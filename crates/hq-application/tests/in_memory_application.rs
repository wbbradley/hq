//! In-memory application boundary acceptance tests.

use hq_application::InMemoryApplication;
use std::error::Error;

use hq_domain::{
    BoundedSet, CausalReferences, EncryptionPublicKey, Fact, FactId, FactScope,
    InstallationAddress, InstallationId, MAX_FACT_AUTHORITIES, MAX_FACT_PARENTS, SemanticPayload,
    ShortText, SigningPublicKey, Timestamp,
};

#[test]
fn duplicate_submission_does_not_change_the_projection() -> Result<(), Box<dyn Error>> {
    let id = FactId::from_bytes([7; 32]);
    let installation_id = InstallationId::from_bytes([7; 32]);
    let signing_key = SigningPublicKey::from_bytes([7; 32]);
    let fact = Fact::new(
        id,
        InstallationAddress::new(installation_id, signing_key),
        Timestamp::from_unix_millis(7),
        FactScope::InstallationPrivate(installation_id),
        CausalReferences::<MAX_FACT_PARENTS, MAX_FACT_AUTHORITIES>::new(BoundedSet::new([])?, [])?,
        SemanticPayload::InstallationDeclared {
            installation_id,
            signing_key,
            encryption_key: EncryptionPublicKey::from_bytes([7; 32]),
            label: Some(ShortText::new("hello")?),
        },
    )?;
    let mut application = InMemoryApplication::default();

    application.submit(fact.clone());
    application.submit(fact);

    assert_eq!(application.summary().unique_fact_count(), 1);
    Ok(())
}
