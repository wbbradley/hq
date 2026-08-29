//! Durable installation-local mailbox draft contracts.

#![allow(clippy::expect_used, clippy::panic)]

use hq_application::{
    MailboxDraft, MailboxDraftDeleteOutcome, MailboxDraftDeleteRequest, MailboxDraftSaveOutcome,
    MailboxDraftSaveRequest, MailboxDraftTarget,
};
use hq_domain::{InstallationId, MailboxAddress, MailboxId, MessageId, OperationId};

mod support;

use support::{TestDirectory, open_store};

#[test]
fn drafts_autosave_conflict_delete_and_survive_restart() {
    let directory = TestDirectory::new();
    let database = directory.database_path();
    let draft_id = OperationId::from_bytes([0x31; 32]);
    let reply = MailboxDraftTarget::Reply {
        message_id: MessageId::from_bytes([0x41; 32]),
    };
    let store = open_store(&database);
    let drafts = store.application_state_handle();

    assert_eq!(
        drafts.save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: reply.clone(),
            content: String::new(),
            expected_version: None,
        }),
        Ok(MailboxDraftSaveOutcome::Saved(MailboxDraft {
            draft_id,
            target: reply.clone(),
            content: String::new(),
            version: 1,
        }))
    );
    assert_eq!(
        drafts.save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: reply.clone(),
            content: "first answer".to_owned(),
            expected_version: Some(1),
        }),
        Ok(MailboxDraftSaveOutcome::Saved(MailboxDraft {
            draft_id,
            target: reply.clone(),
            content: "first answer".to_owned(),
            version: 2,
        }))
    );
    assert_eq!(
        drafts.save_mailbox_draft(MailboxDraftSaveRequest {
            draft_id,
            target: MailboxDraftTarget::SelfNote,
            content: "stale overwrite".to_owned(),
            expected_version: Some(1),
        }),
        Ok(MailboxDraftSaveOutcome::Conflict(MailboxDraft {
            draft_id,
            target: reply.clone(),
            content: "first answer".to_owned(),
            version: 2,
        }))
    );
    store.close().expect("store closes");

    let reopened = open_store(&database);
    let drafts = reopened.application_state_handle();
    assert_eq!(
        drafts.load_mailbox_drafts(),
        Ok(vec![MailboxDraft {
            draft_id,
            target: reply.clone(),
            content: "first answer".to_owned(),
            version: 2,
        }])
    );
    assert_eq!(
        drafts.delete_mailbox_draft(MailboxDraftDeleteRequest {
            draft_id,
            expected_version: 1,
        }),
        Ok(MailboxDraftDeleteOutcome::Conflict(MailboxDraft {
            draft_id,
            target: reply,
            content: "first answer".to_owned(),
            version: 2,
        }))
    );
    assert_eq!(
        drafts.delete_mailbox_draft(MailboxDraftDeleteRequest {
            draft_id,
            expected_version: 2,
        }),
        Ok(MailboxDraftDeleteOutcome::Deleted)
    );
    assert_eq!(
        drafts.delete_mailbox_draft(MailboxDraftDeleteRequest {
            draft_id,
            expected_version: 2,
        }),
        Ok(MailboxDraftDeleteOutcome::NotFound)
    );
}

#[test]
fn every_explicit_target_round_trips_without_a_canonical_foreign_key() {
    let directory = TestDirectory::new();
    let store = open_store(&directory.database_path());
    let drafts = store.application_state_handle();
    let targets = [
        MailboxDraftTarget::Reply {
            message_id: MessageId::from_bytes([0x51; 32]),
        },
        MailboxDraftTarget::Direct {
            recipient: MailboxAddress::new(
                InstallationId::from_bytes([0x61; 32]),
                MailboxId::from_bytes([0x62; 32]),
            ),
        },
        MailboxDraftTarget::SelfNote,
    ];
    for (index, target) in targets.into_iter().enumerate() {
        let mut id = [0; 32];
        id[31] = u8::try_from(index + 1).expect("small index");
        drafts
            .save_mailbox_draft(MailboxDraftSaveRequest {
                draft_id: OperationId::from_bytes(id),
                target,
                content: "recover me".to_owned(),
                expected_version: None,
            })
            .expect("draft saves");
    }
    let loaded = drafts.load_mailbox_drafts().expect("drafts load");
    assert_eq!(loaded.len(), 3);
    assert!(loaded.iter().all(|draft| draft.content == "recover me"));
}
