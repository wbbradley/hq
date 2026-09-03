//! Bounded one-shot lifecycle client over the private local Unix socket.

use std::{error::Error, fmt, os::unix::net::UnixStream, time::Duration};

use hq_local_api::protocol::v1::{
    BuildMetadata, LifecycleRequest, LifecycleStatus, Request, RequestEnvelope, RequestId,
    Response, ResponseResult, V1, VersionRange, WireMessage,
};

use crate::{ReadinessRecord, RuntimeArtifactErrorClass, RuntimePaths, unix_frame};

/// Explicit inputs for one bounded lifecycle request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleClientConfig {
    /// Exact private runtime namespace to probe.
    pub runtime: RuntimePaths,
    /// Safe client build metadata sent during negotiation.
    pub build: BuildMetadata,
    /// Positive read and write deadline applied to the connection.
    pub io_timeout: Duration,
}

/// Successful protocol lifecycle observation plus optional post-authentication diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleObservation {
    /// Typed response returned by the negotiated node.
    pub status: LifecycleStatus,
    /// Strict readiness metadata read only after the socket protocol authenticated liveness.
    pub readiness: Option<ReadinessRecord>,
}

/// Stable one-shot lifecycle probe failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleClientError {
    /// I/O timeout must be positive.
    InvalidTimeout,
    /// No local listener accepted the connection.
    Absent,
    /// Socket I/O failed before a complete response was available.
    Transport,
    /// The request was written but its response was lost.
    ResponseLost,
    /// The server supports no compatible local protocol version.
    Incompatible,
    /// Negotiation or the correlated response violated protocol order.
    Protocol,
    /// Existing readiness metadata failed strict validation after protocol liveness succeeded.
    Readiness(RuntimeArtifactErrorClass),
    /// The live peer did not match the boot generation in its readiness artifact.
    StaleReadiness,
}

impl fmt::Display for LifecycleClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local lifecycle request failed: {self:?}")
    }
}

impl Error for LifecycleClientError {}

/// Reusable configuration owner for independent one-shot lifecycle connections.
#[derive(Clone, Debug)]
pub struct LifecycleClient {
    config: LifecycleClientConfig,
}

impl LifecycleClient {
    /// Validates and retains explicit lifecycle probe configuration.
    pub fn new(config: LifecycleClientConfig) -> Result<Self, LifecycleClientError> {
        if config.io_timeout.is_zero() {
            return Err(LifecycleClientError::InvalidTimeout);
        }
        Ok(Self { config })
    }

    /// Negotiates a fresh connection and executes exactly one lifecycle request.
    pub fn request(
        &mut self,
        request: LifecycleRequest,
    ) -> Result<LifecycleObservation, LifecycleClientError> {
        let mut stream =
            UnixStream::connect(self.config.runtime.socket_file()).map_err(|error| {
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) {
                    LifecycleClientError::Absent
                } else {
                    LifecycleClientError::Transport
                }
            })?;
        stream
            .set_read_timeout(Some(self.config.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.config.io_timeout)))
            .map_err(|_| LifecycleClientError::Transport)?;

        let hello = WireMessage::ClientHello(hq_local_api::protocol::v1::ClientHello::new(
            VersionRange::new(V1, V1).map_err(|_| LifecycleClientError::Protocol)?,
            self.config.build.clone(),
        ));
        write_message(&mut stream, &hello, false)?;
        match read_message(&mut stream, false)? {
            WireMessage::ServerHello(hello) if hello.selected_version == V1 => {}
            WireMessage::VersionRejected(_) => return Err(LifecycleClientError::Incompatible),
            _ => return Err(LifecycleClientError::Protocol),
        }

        let readiness = read_readiness_after_protocol(&self.config.runtime)?;
        let request_id = RequestId::new(1).map_err(|_| LifecycleClientError::Protocol)?;
        let message = WireMessage::Request(RequestEnvelope::new(
            request_id,
            Request::Lifecycle(request),
        ));
        write_message(&mut stream, &message, true)?;
        let response = read_message(&mut stream, true)?;
        let WireMessage::Response(response) = response else {
            return Err(LifecycleClientError::Protocol);
        };
        if response.id != request_id {
            return Err(LifecycleClientError::Protocol);
        }
        let Response::Success(ResponseResult::Lifecycle(status)) = response.response else {
            return Err(LifecycleClientError::Protocol);
        };
        validate_readiness_generation(&status, readiness.as_ref())?;
        Ok(LifecycleObservation { status, readiness })
    }
}

fn validate_readiness_generation(
    status: &LifecycleStatus,
    readiness: Option<&ReadinessRecord>,
) -> Result<(), LifecycleClientError> {
    if status.state != hq_local_api::protocol::v1::LifecycleState::Ready {
        return Ok(());
    }
    let Some(generation) = status.generation else {
        return Err(LifecycleClientError::StaleReadiness);
    };
    if readiness.is_some_and(|record| record.boot_nonce == generation) {
        Ok(())
    } else {
        Err(LifecycleClientError::StaleReadiness)
    }
}

fn write_message(
    stream: &mut UnixStream,
    message: &WireMessage,
    request_written: bool,
) -> Result<(), LifecycleClientError> {
    let frame = message
        .encode_frame()
        .map_err(|_| LifecycleClientError::Protocol)?;
    unix_frame::write_frame(stream, &frame).map_err(|_| {
        if request_written {
            LifecycleClientError::ResponseLost
        } else {
            LifecycleClientError::Transport
        }
    })
}

fn read_message(
    stream: &mut UnixStream,
    request_written: bool,
) -> Result<WireMessage, LifecycleClientError> {
    let lost = || {
        if request_written {
            LifecycleClientError::ResponseLost
        } else {
            LifecycleClientError::Transport
        }
    };
    let frame = unix_frame::read_frame(stream).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            LifecycleClientError::Protocol
        } else {
            lost()
        }
    })?;
    WireMessage::decode_frame(&frame).map_err(|_| LifecycleClientError::Protocol)
}

fn read_readiness_after_protocol(
    runtime: &RuntimePaths,
) -> Result<Option<ReadinessRecord>, LifecycleClientError> {
    match std::fs::symlink_metadata(runtime.readiness_file()) {
        Ok(_) => ReadinessRecord::read_from(runtime.readiness_file())
            .map(Some)
            .map_err(|error| LifecycleClientError::Readiness(error.class())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(LifecycleClientError::Readiness(
            RuntimeArtifactErrorClass::OperatingSystem,
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleState, LifecycleStatus};

    use super::{LifecycleClientError, ReadinessRecord, validate_readiness_generation};

    fn build() -> BuildMetadata {
        BuildMetadata::new("hq", "0.1.0", Some("generation-test")).expect("build validates")
    }

    fn readiness(generation: u8) -> ReadinessRecord {
        ReadinessRecord::new(
            LifecycleState::Ready,
            1,
            build(),
            Id32::new([4; 32]),
            7,
            Id32::new([generation; 32]),
        )
        .expect("readiness validates")
    }

    #[test]
    fn ready_response_requires_the_exact_advertised_generation() {
        let status = LifecycleStatus::new(LifecycleState::Ready, build(), Some(7), None)
            .expect("status validates")
            .with_generation(Id32::new([2; 32]));

        assert_eq!(
            validate_readiness_generation(&status, Some(&readiness(1))),
            Err(LifecycleClientError::StaleReadiness)
        );
        assert_eq!(
            validate_readiness_generation(&status, None),
            Err(LifecycleClientError::StaleReadiness)
        );
        assert_eq!(
            validate_readiness_generation(&status, Some(&readiness(2))),
            Ok(())
        );
    }

    #[test]
    fn nonready_response_does_not_require_published_readiness() {
        let status = LifecycleStatus::new(LifecycleState::Starting, build(), None, None)
            .expect("status validates");
        assert_eq!(validate_readiness_generation(&status, None), Ok(()));
    }
}
