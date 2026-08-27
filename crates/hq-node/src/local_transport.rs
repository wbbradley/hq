//! Private Unix listener, same-user peer validation, and atomic readiness ownership.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
};

use hq_domain::InstallationId;
use hq_local_api::protocol::v1::{BuildMetadata, Id32, LifecycleState, MAX_BUILD_FIELD_BYTES};
use serde::{Deserialize, Serialize};

use crate::RuntimePaths;

/// Maximum complete encoded readiness record accepted before JSON parsing.
pub const MAX_READINESS_BYTES: usize = 4_096;

const READINESS_VERSION: u16 = 1;
const MAX_BIND_ATTEMPTS: usize = 3;

/// Stable redacted local runtime-artifact failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeArtifactErrorClass {
    /// A reserved artifact is a symbolic link.
    SymbolicLink,
    /// A reserved path contains an ordinary file, directory, or other unsafe type.
    UnsafeArtifact,
    /// A runtime artifact is accessible beyond its owner.
    UnsafePermissions,
    /// A responding listener already owns the socket path.
    LiveListener,
    /// Kernel peer credentials were unavailable.
    PeerCredentials,
    /// The accepted peer does not match the process effective user.
    PeerMismatch,
    /// No connection is currently waiting on the nonblocking listener.
    WouldBlock,
    /// The listener was already bound by this owner.
    AlreadyBound,
    /// A listener-dependent operation was requested before binding.
    NotBound,
    /// Readiness bytes or fields are malformed or noncanonical.
    ReadinessInvalid,
    /// Readiness input exceeds the fixed pre-parse bound.
    ReadinessTooLarge,
    /// A path changed identity while an owned operation was in progress.
    ArtifactChanged,
    /// A filesystem or socket operation failed without retaining platform prose.
    OperatingSystem,
}

/// Redacted local runtime-artifact failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeArtifactError {
    class: RuntimeArtifactErrorClass,
}

impl RuntimeArtifactError {
    const fn new(class: RuntimeArtifactErrorClass) -> Self {
        Self { class }
    }

    /// Returns the stable failure class.
    pub const fn class(self) -> RuntimeArtifactErrorClass {
        self.class
    }

    pub(crate) const fn from_shutdown_state() -> Self {
        Self::new(RuntimeArtifactErrorClass::NotBound)
    }

    pub(crate) const fn from_nonready_state() -> Self {
        Self::new(RuntimeArtifactErrorClass::ReadinessInvalid)
    }
}

impl fmt::Display for RuntimeArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.class {
            RuntimeArtifactErrorClass::SymbolicLink => {
                "local runtime artifact must not be a symbolic link"
            }
            RuntimeArtifactErrorClass::UnsafeArtifact => "local runtime artifact type is unsafe",
            RuntimeArtifactErrorClass::UnsafePermissions => {
                "local runtime artifact permissions are unsafe"
            }
            RuntimeArtifactErrorClass::LiveListener => "local listener is already running",
            RuntimeArtifactErrorClass::PeerCredentials => "local peer credentials are unavailable",
            RuntimeArtifactErrorClass::PeerMismatch => "local peer user does not match",
            RuntimeArtifactErrorClass::WouldBlock => "no local connection is ready",
            RuntimeArtifactErrorClass::AlreadyBound => "local listener is already bound",
            RuntimeArtifactErrorClass::NotBound => "local listener is not bound",
            RuntimeArtifactErrorClass::ReadinessInvalid => "readiness metadata is invalid",
            RuntimeArtifactErrorClass::ReadinessTooLarge => "readiness metadata is too large",
            RuntimeArtifactErrorClass::ArtifactChanged => "local runtime artifact changed identity",
            RuntimeArtifactErrorClass::OperatingSystem => "local runtime operation failed",
        })
    }
}

impl Error for RuntimeArtifactError {}

/// Versioned diagnostic readiness metadata; no field grants ownership or domain authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessRecord {
    /// Readiness metadata version, independently versioned from local protocol v1.
    pub version: u16,
    /// A readiness artifact represents only acknowledged `Ready` state.
    pub state: LifecycleState,
    /// Diagnostic process identity, never a liveness or ownership oracle.
    pub process_id: u32,
    /// Safe executable build identity.
    pub build: BuildMetadata,
    /// Installation whose private runtime namespace contains this record.
    pub installation_id: Id32,
    /// Authoritative store revision acknowledged at readiness.
    pub revision: u64,
    /// Fresh non-authoritative identity distinguishing one boot publication.
    pub boot_nonce: Id32,
}

impl ReadinessRecord {
    /// Constructs and validates one explicit readiness record.
    pub fn new(
        state: LifecycleState,
        process_id: u32,
        build: BuildMetadata,
        installation_id: Id32,
        revision: u64,
        boot_nonce: Id32,
    ) -> Result<Self, RuntimeArtifactError> {
        let record = Self {
            version: READINESS_VERSION,
            state,
            process_id,
            build,
            installation_id,
            revision,
            boot_nonce,
        };
        record.validate()?;
        Ok(record)
    }

    /// Strictly decodes one bounded canonical JSON readiness record.
    pub fn decode(bytes: &[u8]) -> Result<Self, RuntimeArtifactError> {
        if bytes.len() > MAX_READINESS_BYTES {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessTooLarge,
            ));
        }
        let record = serde_json::from_slice::<Self>(bytes)
            .map_err(|_| RuntimeArtifactError::new(RuntimeArtifactErrorClass::ReadinessInvalid))?;
        record.validate()?;
        if record.encode()? != bytes {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessInvalid,
            ));
        }
        Ok(record)
    }

    /// Reads and strictly decodes one private readiness file with a pre-allocation size check.
    pub fn read_from(path: &Path) -> Result<Self, RuntimeArtifactError> {
        let metadata = checked_metadata(path, ArtifactKind::RegularFile)?;
        if metadata.len() > MAX_READINESS_BYTES as u64 {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessTooLarge,
            ));
        }
        let expected = ArtifactIdentity::from_metadata(&metadata);
        let file = File::open(path).map_err(operating_system)?;
        let opened = file.metadata().map_err(operating_system)?;
        if ArtifactIdentity::from_metadata(&opened) != expected {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ArtifactChanged,
            ));
        }
        let capacity = usize::try_from(metadata.len()).map_err(operating_system)?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take((MAX_READINESS_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(operating_system)?;
        Self::decode(&bytes)
    }

    fn encode(&self) -> Result<Vec<u8>, RuntimeArtifactError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(operating_system)?;
        if bytes.len() > MAX_READINESS_BYTES {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessTooLarge,
            ));
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<(), RuntimeArtifactError> {
        let valid_build_field = |field: &str| {
            !field.is_empty()
                && field.len() <= MAX_BUILD_FIELD_BYTES
                && !field.chars().any(char::is_control)
        };
        if self.version != READINESS_VERSION
            || self.state != LifecycleState::Ready
            || self.process_id == 0
            || self.installation_id == Id32::new([0; 32])
            || self.boot_nonce == Id32::new([0; 32])
            || !valid_build_field(self.build.name())
            || !valid_build_field(self.build.version())
            || self
                .build
                .commit()
                .is_some_and(|value| !valid_build_field(value))
        {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessInvalid,
            ));
        }
        Ok(())
    }
}

fn system_effective_user_id() -> u32 {
    nix::unistd::geteuid().as_raw()
}

fn system_peer_user_id(stream: &UnixStream) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .ok()
            .map(|credentials| credentials.uid())
    }
    #[cfg(target_os = "macos")]
    {
        nix::unistd::getpeereid(stream)
            .ok()
            .map(|(user, _)| user.as_raw())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArtifactIdentity {
    device: u64,
    inode: u64,
}

impl ArtifactIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

#[derive(Debug)]
struct OwnedListener {
    listener: UnixListener,
    identity: ArtifactIdentity,
}

/// Foundation-private owner of socket and readiness artifacts.
#[derive(Debug)]
pub(crate) struct LocalTransportOwner {
    paths: RuntimePaths,
    listener: Option<OwnedListener>,
    readiness: Option<ArtifactIdentity>,
    last_boot_nonce: Option<Id32>,
}

impl LocalTransportOwner {
    pub(crate) const fn new(paths: RuntimePaths) -> Self {
        Self {
            paths,
            listener: None,
            readiness: None,
            last_boot_nonce: None,
        }
    }

    pub(crate) fn bind(&mut self) -> Result<(), RuntimeArtifactError> {
        if self.listener.is_some() {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::AlreadyBound,
            ));
        }
        for _ in 0..MAX_BIND_ATTEMPTS {
            match UnixListener::bind(self.paths.socket_file()) {
                Ok(listener) => {
                    let metadata =
                        fs::symlink_metadata(self.paths.socket_file()).map_err(operating_system)?;
                    if !metadata.file_type().is_socket() {
                        return Err(RuntimeArtifactError::new(
                            RuntimeArtifactErrorClass::ArtifactChanged,
                        ));
                    }
                    let owned = OwnedListener {
                        listener,
                        identity: ArtifactIdentity::from_metadata(&metadata),
                    };
                    let configured = (|| {
                        let directory = open_runtime_directory(self.paths.root())?;
                        let socket_name =
                            self.paths.socket_file().file_name().ok_or_else(|| {
                                RuntimeArtifactError::new(
                                    RuntimeArtifactErrorClass::OperatingSystem,
                                )
                            })?;
                        nix::sys::stat::fchmodat(
                            &directory,
                            Path::new(socket_name),
                            private_file_mode(),
                            nix::sys::stat::FchmodatFlags::NoFollowSymlink,
                        )
                        .map_err(operating_system)?;
                        owned
                            .listener
                            .set_nonblocking(true)
                            .map_err(operating_system)?;
                        let configured =
                            checked_metadata(self.paths.socket_file(), ArtifactKind::Socket)?;
                        if ArtifactIdentity::from_metadata(&configured) != owned.identity {
                            return Err(RuntimeArtifactError::new(
                                RuntimeArtifactErrorClass::ArtifactChanged,
                            ));
                        }
                        Ok(())
                    })();
                    if let Err(error) = configured {
                        let identity = owned.identity;
                        drop(owned);
                        let _ =
                            remove_owned(self.paths.socket_file(), identity, ArtifactKind::Socket);
                        return Err(error);
                    }
                    self.listener = Some(owned);
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    self.probe_or_remove_stale()?;
                }
                Err(_) => return Err(operating_system(())),
            }
        }
        Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::ArtifactChanged,
        ))
    }

    pub(crate) fn accept(&self) -> Result<UnixStream, RuntimeArtifactError> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| RuntimeArtifactError::new(RuntimeArtifactErrorClass::NotBound))?;
        let (stream, _) = listener.listener.accept().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                RuntimeArtifactError::new(RuntimeArtifactErrorClass::WouldBlock)
            } else {
                operating_system(error)
            }
        })?;
        validate_same_user(system_effective_user_id(), system_peer_user_id(&stream))?;
        stream.set_nonblocking(true).map_err(operating_system)?;
        Ok(stream)
    }

    pub(crate) fn publish(&mut self, record: &ReadinessRecord) -> Result<(), RuntimeArtifactError> {
        if self.listener.is_none() {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::NotBound,
            ));
        }
        let bytes = record.encode()?;
        if self.last_boot_nonce == Some(record.boot_nonce) {
            return Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::ReadinessInvalid,
            ));
        }
        self.validate_readiness_target()?;
        let temporary = self.paths.root().join(format!(
            ".node-ready.v1.{}.tmp",
            encode_hex(&record.boot_nonce.bytes())
        ));
        let publish = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(operating_system)?;
            nix::sys::stat::fchmod(&file, private_file_mode()).map_err(operating_system)?;
            file.write_all(&bytes).map_err(operating_system)?;
            file.sync_all().map_err(operating_system)?;
            let temporary_metadata = checked_metadata(&temporary, ArtifactKind::RegularFile)?;
            let temporary_identity = ArtifactIdentity::from_metadata(&temporary_metadata);
            fs::rename(&temporary, self.paths.readiness_file()).map_err(operating_system)?;
            self.readiness = Some(temporary_identity);
            self.last_boot_nonce = Some(record.boot_nonce);
            let metadata =
                checked_metadata(self.paths.readiness_file(), ArtifactKind::RegularFile)?;
            if ArtifactIdentity::from_metadata(&metadata) != temporary_identity {
                return Err(RuntimeArtifactError::new(
                    RuntimeArtifactErrorClass::ArtifactChanged,
                ));
            }
            File::open(self.paths.root())
                .and_then(|directory| directory.sync_all())
                .map_err(operating_system)
        })();
        if publish.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        publish
    }

    pub(crate) fn cleanup(&mut self) -> Result<(), RuntimeArtifactError> {
        let listener = self.listener.take();
        let listener_identity = listener.as_ref().map(|owned| owned.identity);
        drop(listener);

        let mut first_error = None;
        if let Some(identity) = listener_identity
            && let Err(error) =
                remove_owned(self.paths.socket_file(), identity, ArtifactKind::Socket)
        {
            first_error = Some(error);
        }
        if let Some(identity) = self.readiness.take()
            && let Err(error) = remove_owned(
                self.paths.readiness_file(),
                identity,
                ArtifactKind::RegularFile,
            )
        {
            first_error.get_or_insert(error);
        }
        if let Err(error) = File::open(self.paths.root()).and_then(|directory| directory.sync_all())
        {
            first_error.get_or_insert_with(|| operating_system(error));
        }
        first_error.map_or(Ok(()), Err)
    }

    fn probe_or_remove_stale(&self) -> Result<(), RuntimeArtifactError> {
        let metadata = checked_metadata(self.paths.socket_file(), ArtifactKind::Socket)?;
        let identity = ArtifactIdentity::from_metadata(&metadata);
        let socket = nix::sys::socket::socket(
            nix::sys::socket::AddressFamily::Unix,
            nix::sys::socket::SockType::Stream,
            nix::sys::socket::SockFlag::empty(),
            None,
        )
        .map_err(operating_system)?;
        nix::fcntl::fcntl(
            &socket,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .map_err(operating_system)?;
        nix::fcntl::fcntl(
            &socket,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
        )
        .map_err(operating_system)?;
        let address =
            nix::sys::socket::UnixAddr::new(self.paths.socket_file()).map_err(operating_system)?;
        match nix::sys::socket::connect(socket.as_raw_fd(), &address) {
            Ok(())
            | Err(
                nix::errno::Errno::EINPROGRESS
                | nix::errno::Errno::EAGAIN
                | nix::errno::Errno::EALREADY
                | nix::errno::Errno::EISCONN,
            ) => Err(RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::LiveListener,
            )),
            Err(nix::errno::Errno::ECONNREFUSED) => {
                remove_owned(self.paths.socket_file(), identity, ArtifactKind::Socket)?;
                Ok(())
            }
            Err(nix::errno::Errno::ENOENT) => Ok(()),
            Err(_) => Err(operating_system(())),
        }
    }

    fn validate_readiness_target(&self) -> Result<(), RuntimeArtifactError> {
        match fs::symlink_metadata(self.paths.readiness_file()) {
            Ok(metadata) => {
                validate_metadata(&metadata, ArtifactKind::RegularFile)?;
                if let Some(expected) = self.readiness
                    && ArtifactIdentity::from_metadata(&metadata) != expected
                {
                    return Err(RuntimeArtifactError::new(
                        RuntimeArtifactErrorClass::ArtifactChanged,
                    ));
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.readiness.is_some() {
                    Err(RuntimeArtifactError::new(
                        RuntimeArtifactErrorClass::ArtifactChanged,
                    ))
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(operating_system(error)),
        }
    }
}

fn validate_same_user(
    effective_user_id: u32,
    peer_user_id: Option<u32>,
) -> Result<(), RuntimeArtifactError> {
    let peer_user_id = peer_user_id
        .ok_or_else(|| RuntimeArtifactError::new(RuntimeArtifactErrorClass::PeerCredentials))?;
    if effective_user_id != peer_user_id {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::PeerMismatch,
        ));
    }
    Ok(())
}

impl Drop for LocalTransportOwner {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[derive(Clone, Copy)]
enum ArtifactKind {
    Socket,
    RegularFile,
}

fn checked_metadata(path: &Path, kind: ArtifactKind) -> Result<fs::Metadata, RuntimeArtifactError> {
    let metadata = fs::symlink_metadata(path).map_err(operating_system)?;
    validate_metadata(&metadata, kind)?;
    Ok(metadata)
}

fn open_runtime_directory(path: &Path) -> Result<File, RuntimeArtifactError> {
    let metadata = fs::symlink_metadata(path).map_err(operating_system)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::ArtifactChanged,
        ));
    }
    let identity = ArtifactIdentity::from_metadata(&metadata);
    let directory = File::open(path).map_err(operating_system)?;
    let opened = directory.metadata().map_err(operating_system)?;
    if !opened.is_dir() || ArtifactIdentity::from_metadata(&opened) != identity {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::ArtifactChanged,
        ));
    }
    Ok(directory)
}

fn private_file_mode() -> nix::sys::stat::Mode {
    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR
}

fn validate_metadata(
    metadata: &fs::Metadata,
    kind: ArtifactKind,
) -> Result<(), RuntimeArtifactError> {
    if metadata.file_type().is_symlink() {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::SymbolicLink,
        ));
    }
    let expected_type = match kind {
        ArtifactKind::Socket => metadata.file_type().is_socket(),
        ArtifactKind::RegularFile => metadata.file_type().is_file(),
    };
    if !expected_type {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::UnsafeArtifact,
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o600 {
        return Err(RuntimeArtifactError::new(
            RuntimeArtifactErrorClass::UnsafePermissions,
        ));
    }
    Ok(())
}

fn remove_owned(
    path: &Path,
    identity: ArtifactIdentity,
    kind: ArtifactKind,
) -> Result<(), RuntimeArtifactError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata_matches_kind(&metadata, kind)
                || ArtifactIdentity::from_metadata(&metadata) != identity
            {
                return Err(RuntimeArtifactError::new(
                    RuntimeArtifactErrorClass::ArtifactChanged,
                ));
            }
            fs::remove_file(path).map_err(operating_system)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(operating_system(error)),
    }
}

fn metadata_matches_kind(metadata: &fs::Metadata, kind: ArtifactKind) -> bool {
    !metadata.file_type().is_symlink()
        && match kind {
            ArtifactKind::Socket => metadata.file_type().is_socket(),
            ArtifactKind::RegularFile => metadata.file_type().is_file(),
        }
}

pub(crate) fn ready_record(
    build: BuildMetadata,
    installation: InstallationId,
    revision: u64,
    boot_nonce: Id32,
) -> Result<ReadinessRecord, RuntimeArtifactError> {
    ReadinessRecord::new(
        LifecycleState::Ready,
        std::process::id(),
        build,
        Id32::new(*installation.as_bytes()),
        revision,
        boot_nonce,
    )
}

fn operating_system<T>(_: T) -> RuntimeArtifactError {
    RuntimeArtifactError::new(RuntimeArtifactErrorClass::OperatingSystem)
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{RuntimeArtifactErrorClass, validate_same_user};

    #[test]
    fn peer_identity_validation_fails_closed() {
        assert_eq!(validate_same_user(1000, Some(1000)), Ok(()));
        assert_eq!(
            validate_same_user(1000, Some(1001)),
            Err(super::RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::PeerMismatch
            ))
        );
        assert_eq!(
            validate_same_user(1000, None),
            Err(super::RuntimeArtifactError::new(
                RuntimeArtifactErrorClass::PeerCredentials
            ))
        );
    }
}
