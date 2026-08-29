//! Installation-root envelope adapter for owned relay sessions.

use sha2::{Digest, Sha256};

use crate::{
    AuthInput, EnvelopeCodec, FailureClass, LogicalEnvelopeId, OpenedRelayEnvelope, OutboundIntent,
    PreparedOutbound, PreparedRelayAuthentication, RandomSource, RejectedRelayEnvelope,
    RelayEnvelopePort, RelayOpenOutcome, RelayPortError, RelayUrl, SystemRandom,
    envelope::verified_transport_event_id,
};

impl RelayEnvelopePort for EnvelopeCodec {
    fn local_public_key(&self) -> [u8; 32] {
        self.public_key()
    }

    fn prepare(
        &self,
        intent: &OutboundIntent,
        recipient_public_key: [u8; 32],
        now_seconds: u64,
    ) -> Result<PreparedOutbound, RelayPortError> {
        let semantic = decode_canonical(&intent.exact_canonical_bytes)?;
        if semantic.fact().id() != intent.key.fact_id {
            return Err(RelayPortError::Corrupt);
        }
        let mut random = SystemRandom;
        let envelope = EnvelopeCodec::prepare(
            self,
            &semantic,
            recipient_public_key,
            now_seconds,
            &mut random,
        )
        .map_err(map_local_envelope_error)?
        .into_durable();
        Ok(PreparedOutbound {
            key: intent.key,
            envelope,
        })
    }

    fn open(&self, exact_outer: &[u8]) -> Result<RelayOpenOutcome, RelayPortError> {
        match EnvelopeCodec::open(self, exact_outer) {
            Ok(opened) => Ok(RelayOpenOutcome::Opened(OpenedRelayEnvelope {
                canonical_sha256: Sha256::digest(&opened.canonical_event).into(),
                exact_canonical_bytes: opened.canonical_event.into_vec(),
                wrapper_id: opened.metadata.wrapper_id,
                wrapper_created_at: opened.metadata.wrapper_created_at,
                logical_id: LogicalEnvelopeId {
                    origin_installation_id: opened.metadata.origin_installation_id,
                    canonical_event_id: opened.metadata.canonical_event_id,
                },
            })),
            Err(error) => Ok(RelayOpenOutcome::Rejected(RejectedRelayEnvelope {
                failure: error.class(),
                wrapper_id: None,
            })),
        }
    }

    fn authenticate(
        &self,
        url: &RelayUrl,
        challenge: &str,
        now_seconds: u64,
    ) -> Result<PreparedRelayAuthentication, RelayPortError> {
        let mut auxiliary_randomness = [0; 32];
        SystemRandom
            .fill(&mut auxiliary_randomness)
            .map_err(map_local_envelope_error)?;
        let exact_event = self
            .authentication_event(
                AuthInput {
                    relay_url: url.as_str().to_owned(),
                    challenge: challenge.to_owned(),
                    created_at: now_seconds,
                },
                auxiliary_randomness,
            )
            .map_err(map_local_envelope_error)?;
        let event_id =
            verified_transport_event_id(&exact_event).map_err(map_local_envelope_error)?;
        Ok(PreparedRelayAuthentication {
            event_id,
            exact_event,
        })
    }
}

fn decode_canonical(exact: &[u8]) -> Result<hq_protocol::VerifiedSemanticFact, RelayPortError> {
    hq_protocol::decode_semantic_event(exact.to_vec())
        .map_err(|_| RelayPortError::Corrupt)?
        .ok_or(RelayPortError::Corrupt)
}

const fn map_local_envelope_error(error: crate::EnvelopeError) -> RelayPortError {
    if matches!(error.class(), FailureClass::Cryptography) {
        RelayPortError::Unavailable
    } else {
        RelayPortError::Corrupt
    }
}
