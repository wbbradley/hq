//! Deterministic causal reduction without runtime or adapter dependencies.

use std::collections::BTreeSet;

use hq_domain::{Fact, FactId};

/// Minimal deterministic projection proving the pure reducer boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FactSummary {
    ordered_fact_ids: Vec<FactId>,
}

impl FactSummary {
    /// Returns the number of unique fact identities in the input.
    pub fn unique_fact_count(&self) -> usize {
        self.ordered_fact_ids.len()
    }

    /// Returns the skeleton fact identities in deterministic order.
    pub fn ordered_fact_ids(&self) -> &[FactId] {
        &self.ordered_fact_ids
    }
}

/// Builds the in-memory skeleton projection without I/O or ambient inputs.
pub fn summarize(facts: &[Fact]) -> FactSummary {
    let ordered_fact_ids = facts
        .iter()
        .map(Fact::id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    FactSummary { ordered_fact_ids }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use hq_domain::{
        BoundedSet, CausalReferences, EncryptionPublicKey, Fact, FactId, FactScope,
        InstallationAddress, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS,
        SemanticPayload, ShortText, SigningPublicKey, Timestamp,
    };

    use super::summarize;

    #[test]
    fn summary_is_deterministic_and_deduplicated() -> Result<(), Box<dyn Error>> {
        let fact_id = |value| {
            let mut bytes = [0; 32];
            bytes[31] = value;
            FactId::from_bytes(bytes)
        };
        let fact = |value, label| -> Result<Fact, Box<dyn Error>> {
            let id = fact_id(value);
            let installation_id = InstallationId::from_bytes(*id.as_bytes());
            let signing_key = SigningPublicKey::from_bytes(*id.as_bytes());
            Ok(Fact::new(
                id,
                InstallationAddress::new(installation_id, signing_key),
                Timestamp::from_unix_millis(i64::from(value)),
                FactScope::InstallationPrivate(installation_id),
                CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(
                    BoundedSet::new([])?,
                    [],
                )?,
                SemanticPayload::InstallationDeclared {
                    installation_id,
                    signing_key,
                    encryption_key: EncryptionPublicKey::from_bytes(*id.as_bytes()),
                    label: Some(ShortText::new(label)?),
                },
            )?)
        };
        let facts = [fact(2, "second")?, fact(1, "first")?, fact(2, "second")?];

        assert_eq!(
            summarize(&facts).ordered_fact_ids(),
            [fact_id(1), fact_id(2)]
        );
        Ok(())
    }
}
