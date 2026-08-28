//! Named-agent authoring contract tests.

use std::collections::BTreeSet;

use hq_application::{
    AgentNameClaimRequest, AgentRetirementPlanRequest, AgentSessionRenameRequest,
    AgentSessionSelectionRequest, LocalFactInputs, LocalInstallationAuthority,
    plan_agent_mailbox_creation, plan_agent_name_claim, plan_agent_retirement,
    plan_agent_session_rename, plan_agent_session_selection,
};
use hq_domain::{
    AgentId, AuthorityRole, BoundedText, FactId, FactScope, InstallationId, MailboxAddress,
    MailboxId, MailboxKind, ProviderId, ProviderSessionId, RepositoryContext, ResourceLocator,
    ResourceScheme, SemanticPayload, ShortText, SigningPublicKey, Timestamp,
};

fn id(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn authority() -> LocalInstallationAuthority {
    LocalInstallationAuthority {
        installation_id: InstallationId::from_bytes(id(1)),
        signing_key: SigningPublicKey::from_bytes(id(2)),
        root_fact: FactId::from_bytes(id(3)),
    }
}

fn inputs() -> LocalFactInputs {
    LocalFactInputs {
        authored_at: Timestamp::from_unix_millis(4),
        auxiliary_randomness: id(5),
    }
}

fn context() -> Result<RepositoryContext, Box<dyn std::error::Error>> {
    Ok(RepositoryContext {
        directory: ResourceLocator::new(
            ResourceScheme::WorkingTree,
            BoundedText::new("/work/repo")?,
        ),
        repository: None,
        worktree: None,
        branch: Some(ShortText::new("main")?),
    })
}

#[test]
fn mailbox_and_name_plans_bind_local_authority_and_exact_mailbox_root()
-> Result<(), Box<dyn std::error::Error>> {
    let authority = authority();
    let mailbox_id = MailboxId::from_bytes(id(6));
    let mailbox = plan_agent_mailbox_creation(authority, inputs(), mailbox_id, None)?;
    assert_eq!(
        mailbox.scope(),
        &FactScope::InstallationPrivate(authority.installation_id)
    );
    assert_eq!(
        mailbox.payload(),
        &SemanticPayload::MailboxCreated {
            mailbox_id,
            kind: MailboxKind::Agent,
            label: None,
        }
    );
    assert!(mailbox.causal().parents().contains(&authority.root_fact));

    let mailbox_root = FactId::from_bytes(id(7));
    let agent_id = AgentId::from_bytes(id(8));
    let name = ShortText::new("build-agent")?;
    let claim = plan_agent_name_claim(
        authority,
        inputs(),
        AgentNameClaimRequest {
            agent_id,
            mailbox: MailboxAddress::new(authority.installation_id, mailbox_id),
            mailbox_root,
            name: name.clone(),
        },
    )?;
    assert_eq!(
        claim.payload(),
        &SemanticPayload::AgentNameClaimed {
            agent_id,
            mailbox_id,
            name,
        }
    );
    assert!(claim.causal().parents().contains(&authority.root_fact));
    assert!(claim.causal().parents().contains(&mailbox_root));
    assert_eq!(
        claim.causal().authority(AuthorityRole::LocalInstallation),
        Some(authority.root_fact)
    );
    Ok(())
}

#[test]
fn selection_and_rename_plans_include_exact_support_and_complete_register_frontiers()
-> Result<(), Box<dyn std::error::Error>> {
    let authority = authority();
    let mailbox_id = MailboxId::from_bytes(id(6));
    let mailbox = MailboxAddress::new(authority.installation_id, mailbox_id);
    let agent_id = AgentId::from_bytes(id(8));
    let provider = ProviderId::new("provider")?;
    let session = ProviderSessionId::new("session-1")?;
    let claim_fact = FactId::from_bytes(id(9));
    let binding_fact = FactId::from_bytes(id(10));
    let context_fact = FactId::from_bytes(id(11));
    let selection_frontier =
        BTreeSet::from([FactId::from_bytes(id(12)), FactId::from_bytes(id(13))]);
    let selected_context = context()?;
    let selection = plan_agent_session_selection(
        authority,
        inputs(),
        AgentSessionSelectionRequest {
            agent_id,
            mailbox,
            claim_fact,
            provider: provider.clone(),
            session: session.clone(),
            binding_fact,
            context_fact,
            context: selected_context.clone(),
            selection_frontier: selection_frontier.clone(),
        },
    )?;
    assert_eq!(
        selection.payload(),
        &SemanticPayload::ProviderSessionSelected {
            agent_id,
            mailbox_id,
            provider: provider.clone(),
            session: session.clone(),
            context: selected_context,
        }
    );
    for parent in [authority.root_fact, claim_fact, binding_fact, context_fact]
        .into_iter()
        .chain(selection_frontier)
    {
        assert!(selection.causal().parents().contains(&parent));
    }

    let rename_frontier = BTreeSet::from([FactId::from_bytes(id(14)), FactId::from_bytes(id(15))]);
    let display_name = Some(ShortText::new("review")?);
    let rename = plan_agent_session_rename(
        authority,
        inputs(),
        AgentSessionRenameRequest {
            agent_id,
            mailbox,
            claim_fact,
            provider: provider.clone(),
            session: session.clone(),
            binding_fact,
            display_name: display_name.clone(),
            rename_frontier: rename_frontier.clone(),
        },
    )?;
    assert_eq!(
        rename.payload(),
        &SemanticPayload::ProviderSessionRenamed {
            agent_id,
            provider,
            session,
            display_name,
        }
    );
    for parent in [authority.root_fact, claim_fact, binding_fact]
        .into_iter()
        .chain(rename_frontier)
    {
        assert!(rename.causal().parents().contains(&parent));
    }
    Ok(())
}

#[test]
fn agent_planners_reject_nonlocal_mailboxes() -> Result<(), Box<dyn std::error::Error>> {
    let authority = authority();
    let result = plan_agent_name_claim(
        authority,
        inputs(),
        AgentNameClaimRequest {
            agent_id: AgentId::from_bytes(id(8)),
            mailbox: MailboxAddress::new(
                InstallationId::from_bytes(id(99)),
                MailboxId::from_bytes(id(6)),
            ),
            mailbox_root: FactId::from_bytes(id(7)),
            name: ShortText::new("agent")?,
        },
    );
    let Err(error) = result else {
        return Err("a local name adopted a remote mailbox".into());
    };
    assert_eq!(
        error.code(),
        hq_application::ApplicationErrorCode::InvalidRequest
    );
    Ok(())
}

#[test]
fn retirement_plan_binds_the_exact_claim_and_complete_agent_frontier()
-> Result<(), Box<dyn std::error::Error>> {
    let authority = authority();
    let agent_id = AgentId::from_bytes(id(8));
    let mailbox_id = MailboxId::from_bytes(id(6));
    let claim_fact = FactId::from_bytes(id(9));
    let agent_frontier = BTreeSet::from([FactId::from_bytes(id(12)), FactId::from_bytes(id(13))]);
    let plan = plan_agent_retirement(
        authority,
        inputs(),
        AgentRetirementPlanRequest {
            agent_id,
            mailbox: MailboxAddress::new(authority.installation_id, mailbox_id),
            claim_fact,
            agent_frontier: agent_frontier.clone(),
        },
    )?;

    assert_eq!(
        plan.payload(),
        &SemanticPayload::AgentRetired {
            agent_id,
            mailbox_id,
        }
    );
    for parent in [authority.root_fact, claim_fact]
        .into_iter()
        .chain(agent_frontier)
    {
        assert!(plan.causal().parents().contains(&parent));
    }
    Ok(())
}
