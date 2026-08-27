//! Public contract tests for validated domain primitives.

use hq_domain::{
    AccountId, AgentId, AuthorityReference, AuthorityRole, BoundedSet, BoundedText, BoundedVec,
    CausalReferences, Command, CommandId, DomainError, EncryptionPublicKey, ErrorCategory,
    ErrorCode, FactId, InstallationAddress, InstallationId, MailboxAddress, MailboxId, MessageId,
    NonEmptyBoundedSet, OperationCorrelation, OperationId, Outcome, Page, PageCursor, ProjectId,
    ProviderId, ProviderSessionId, ReceiptId, ResourceId, ResourceLocator, ResourceScheme,
    Revision, SigningPublicKey, Timestamp, ValidatedValueError, VersionedView,
};

#[test]
fn identities_and_keys_preserve_opaque_bytes_and_order_deterministically() {
    let low = FactId::from_bytes([1; 32]);
    let high = FactId::from_bytes([2; 32]);

    assert!(low < high);
    assert_eq!(low.as_bytes(), &[1; 32]);
    assert_eq!(SigningPublicKey::from_bytes([3; 32]).as_bytes(), &[3; 32]);
    assert_eq!(
        EncryptionPublicKey::from_bytes([4; 32]).as_bytes(),
        &[4; 32]
    );

    let _distinct_types = (
        InstallationId::from_bytes([5; 32]),
        MailboxId::from_bytes([6; 32]),
        AccountId::from_bytes([7; 32]),
        AgentId::from_bytes([8; 32]),
        ProjectId::from_bytes([9; 32]),
        MessageId::from_bytes([10; 32]),
        ResourceId::from_bytes([11; 32]),
        CommandId::from_bytes([12; 32]),
        ReceiptId::from_bytes([13; 32]),
        OperationId::from_bytes([14; 32]),
    );
}

#[test]
fn bounded_text_rejects_empty_and_oversize_utf8() {
    assert_eq!(BoundedText::<4>::new(""), Err(ValidatedValueError::Empty));
    assert_eq!(
        BoundedText::<4>::new("hello"),
        Err(ValidatedValueError::TooLong {
            maximum: 4,
            actual: 5,
        })
    );
    assert_eq!(
        BoundedText::<4>::new("éé").map(BoundedText::into_string),
        Ok("éé".to_owned())
    );
}

#[test]
fn bounded_collections_reject_excess_and_non_empty_sets_reject_duplicates() {
    assert_eq!(
        BoundedVec::<_, 2>::new([1, 2, 3]),
        Err(ValidatedValueError::TooMany {
            maximum: 2,
            actual: 3,
        })
    );
    assert_eq!(
        NonEmptyBoundedSet::<u8, 3>::new([]),
        Err(ValidatedValueError::Empty)
    );
    assert_eq!(
        NonEmptyBoundedSet::<_, 3>::new([1, 1]),
        Err(ValidatedValueError::Duplicate)
    );
}

#[test]
fn addresses_keep_installation_mailbox_and_key_roles_distinct() {
    let installation = InstallationId::from_bytes([1; 32]);
    let mailbox = MailboxId::from_bytes([2; 32]);
    let signing_key = SigningPublicKey::from_bytes([3; 32]);

    assert_eq!(
        InstallationAddress::new(installation, signing_key).installation_id(),
        installation
    );
    assert_eq!(
        MailboxAddress::new(installation, mailbox).mailbox_id(),
        mailbox
    );
}

#[test]
fn authority_references_must_name_declared_parents_and_unique_roles()
-> Result<(), ValidatedValueError> {
    let parent = FactId::from_bytes([1; 32]);
    let unrelated = FactId::from_bytes([2; 32]);
    let parents = BoundedSet::<_, 4>::new([parent])?;

    assert_eq!(
        CausalReferences::<4, 4>::new(
            parents.clone(),
            [AuthorityReference::new(
                AuthorityRole::MailboxGrant,
                unrelated,
            )]
        ),
        Err(ValidatedValueError::AuthorityNotParent)
    );
    assert_eq!(
        CausalReferences::<4, 4>::new(
            parents,
            [
                AuthorityReference::new(AuthorityRole::MailboxGrant, parent),
                AuthorityReference::new(AuthorityRole::MailboxGrant, parent),
            ]
        ),
        Err(ValidatedValueError::DuplicateAuthorityRole)
    );
    let root = CausalReferences::<4, 4>::new(BoundedSet::new([])?, [])?;
    assert_eq!(root.parents().iter().len(), 0);
    Ok(())
}

#[test]
fn authority_roles_use_exact_catalog_vocabulary() {
    assert_eq!(
        AuthorityRole::ALL,
        [
            AuthorityRole::LocalInstallation,
            AuthorityRole::MailboxOwner,
            AuthorityRole::MailboxGrant,
            AuthorityRole::AccountCreator,
            AuthorityRole::DeviceGrant,
            AuthorityRole::AccountMembership,
            AuthorityRole::PreviousState,
            AuthorityRole::ProjectHome,
            AuthorityRole::ActiveHuman,
            AuthorityRole::Assignment,
            AuthorityRole::Dispatch,
            AuthorityRole::Request,
            AuthorityRole::OutputBinding,
        ]
    );
}

#[test]
fn resource_and_correlation_values_keep_namespaces_explicit() -> Result<(), ValidatedValueError> {
    let value = BoundedText::<4096>::new("repo://work/widget")?;
    let repository = ResourceLocator::new(ResourceScheme::GitRepository, value.clone());
    let working_tree = ResourceLocator::new(ResourceScheme::WorkingTree, value);
    assert_ne!(repository, working_tree);

    let provider = ProviderId::new("test-provider")?;
    let session = ProviderSessionId::new("session-1")?;
    let operation = OperationId::from_bytes([7; 32]);
    let correlation = OperationCorrelation::new(provider.clone(), session.clone(), operation);
    assert_eq!(correlation.provider(), &provider);
    assert_eq!(correlation.session(), &session);
    assert_eq!(correlation.operation(), operation);
    Ok(())
}

#[test]
fn commands_outcomes_pages_and_views_are_typed_without_transport_shapes()
-> Result<(), ValidatedValueError> {
    let issued_at = Timestamp::from_unix_millis(42);
    let command = Command::new(CommandId::from_bytes([1; 32]), issued_at, "payload");
    assert_eq!(command.issued_at(), issued_at);
    assert_eq!(command.body(), &"payload");

    let error = DomainError::new(
        ErrorCategory::Conflict,
        ErrorCode::new("same-id-different-input")?,
    );
    let outcome: Outcome<u8> = Outcome::Rejected(error.clone());
    assert_eq!(outcome, Outcome::Rejected(error));

    let cursor = PageCursor::new("next-fact")?;
    let page = Page::new(vec![1, 2], Some(cursor.clone()));
    assert_eq!(page.items(), &[1, 2]);
    assert_eq!(page.next_cursor(), Some(&cursor));

    let view = VersionedView::new(Revision::new(3), page);
    assert_eq!(view.revision(), Revision::new(3));
    assert_eq!(view.value().items(), &[1, 2]);
    Ok(())
}
