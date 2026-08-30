//! Foreground-only Codex registration and provider-private launch policy.

use std::{path::PathBuf, sync::Arc, time::Duration};

use hq_application::QueryDomain;
use hq_codex::{
    CODEX_PROVIDER_ID, CodexDiagnosticSink, CodexFactory, CodexFactoryConfig, CodexLaunch,
    CodexLaunchResolver, CodexProcessStarter,
};
use hq_domain::{ProviderId, ResourceScheme};
use hq_harness::{HarnessError, HarnessErrorClass, HarnessInstanceRequest, HarnessRegistry};
use hq_reducer::{AgentLifecycle, AgentProjection, AgentProjectionKey};

/// Passive foreground Codex process and provider-policy configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForegroundCodexConfig {
    /// Executable resolved by the caller environment copied at launch time.
    pub executable: PathBuf,
    /// Optional provider-private model override.
    pub model: Option<String>,
    /// Whether Codex receives explicit permissive approval and sandbox settings.
    pub permissive: bool,
    /// Maximum wait for one correlated app-server response.
    pub call_timeout: Duration,
    /// Grace period before child termination escalates.
    pub process_grace: Duration,
    /// Maximum decoded app-server frames awaiting the session owner.
    pub frame_capacity: usize,
}

impl Default for ForegroundCodexConfig {
    fn default() -> Self {
        Self {
            executable: std::env::var_os("CODEX_BIN")
                .map_or_else(|| PathBuf::from("codex"), PathBuf::from),
            model: None,
            permissive: false,
            call_timeout: Duration::from_secs(30),
            process_grace: Duration::from_secs(2),
            frame_capacity: 256,
        }
    }
}

struct ForegroundCodexLaunchResolver<P> {
    query: P,
    executable: PathBuf,
    model: Option<String>,
    permissive: bool,
}

impl<P: QueryDomain + Send + Sync> CodexLaunchResolver for ForegroundCodexLaunchResolver<P> {
    fn resolve(&self, request: &HarnessInstanceRequest) -> Result<CodexLaunch, HarnessError> {
        let directory = request
            .launch_directory
            .as_ref()
            .ok_or_else(invalid_input)?;
        if directory.scheme() != ResourceScheme::WorkingTree {
            return Err(invalid_input());
        }
        let working_directory = PathBuf::from(directory.value());
        if !working_directory.is_absolute() || !working_directory.is_dir() {
            return Err(invalid_input());
        }
        let snapshot = self
            .query
            .authoritative_snapshot()
            .map_err(|_| HarnessError::new(HarnessErrorClass::Unavailable))?;
        let Some(AgentProjection::Agent(agent)) = snapshot
            .domain()
            .agent()
            .projection(AgentProjectionKey::Agent(request.agent_id))
        else {
            return Err(invalid_input());
        };
        let mut names = agent.names.iter();
        let Some(name) = names.next() else {
            return Err(invalid_input());
        };
        if names.next().is_some() || agent.lifecycle != AgentLifecycle::Active {
            return Err(invalid_input());
        }
        Ok(CodexLaunch {
            executable: self.executable.clone(),
            working_directory,
            developer_instructions: developer_instructions(name.as_str()),
            model: self.model.clone(),
            permissive: self.permissive,
        })
    }
}

/// Builds the only production registry containing the private Codex adapter.
pub fn compose_codex_registry<P>(
    query: P,
    config: ForegroundCodexConfig,
    starter: Arc<dyn CodexProcessStarter>,
    diagnostics: Arc<dyn CodexDiagnosticSink>,
) -> Result<HarnessRegistry, HarnessError>
where
    P: QueryDomain + Send + Sync + 'static,
{
    let resolver = Arc::new(ForegroundCodexLaunchResolver {
        query,
        executable: config.executable,
        model: config.model,
        permissive: config.permissive,
    });
    let factory = Arc::new(CodexFactory::new(CodexFactoryConfig {
        starter,
        resolver,
        diagnostics,
        call_timeout: config.call_timeout,
        process_grace: config.process_grace,
        frame_capacity: config.frame_capacity,
    })?);
    let mut registry = HarnessRegistry::new();
    registry.register_named(
        ProviderId::new(CODEX_PROVIDER_ID).map_err(|_| invalid_input())?,
        hq_domain::ShortText::new("Codex").map_err(|_| invalid_input())?,
        CodexFactory::capabilities(),
        factory,
    )?;
    Ok(registry)
}

fn developer_instructions(name: &str) -> String {
    format!(
        "You are the durable HQ named agent `{name}`. Use HQ messaging for durable human and agent communication. Treat structured Codex questions and approvals as authority-bearing requests; never invent an answer or expose secrets in persistent output."
    )
}

const fn invalid_input() -> HarnessError {
    HarnessError::new(HarnessErrorClass::InvalidInput)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::{BTreeMap, BTreeSet};

    use hq_application::{
        ApplicationError, ApplicationErrorCode, AuthoritativeSnapshot, ConversationEntry,
        DomainSnapshot, ProjectionSnapshot,
    };
    use hq_codex::CodexProcessPipes;
    use hq_domain::{AgentId, FactId, Page, PageCursor, ResourceLocator, Revision, ShortText};
    use hq_harness::{HarnessCapability, HarnessEnvironment, HarnessInstanceRequest};
    use hq_reducer::{AgentView, ConversationKey};

    use super::*;

    #[derive(Clone)]
    struct Query(AuthoritativeSnapshot);

    impl QueryDomain for Query {
        fn authoritative_snapshot(&self) -> Result<AuthoritativeSnapshot, ApplicationError> {
            Ok(self.0.clone())
        }

        fn conversation_entries(
            &self,
            _key: &ConversationKey,
            _limit: usize,
            _cursor: Option<&PageCursor>,
        ) -> Result<Page<ConversationEntry>, ApplicationError> {
            Err(ApplicationError::new(
                ApplicationErrorCode::AdapterUnavailable,
            ))
        }
    }

    struct Starter;

    impl CodexProcessStarter for Starter {
        fn start(
            &self,
            _launch: &CodexLaunch,
            _environment: &HarnessEnvironment,
        ) -> Result<CodexProcessPipes, HarnessError> {
            Err(HarnessError::new(HarnessErrorClass::Unavailable))
        }
    }

    struct Diagnostics;

    impl CodexDiagnosticSink for Diagnostics {
        fn line(&self, _line: &str) {}
    }

    fn query(agent_id: AgentId, lifecycle: AgentLifecycle) -> Query {
        let agent = AgentView {
            claims: BTreeSet::from([FactId::from_bytes([2; 32])]),
            names: BTreeSet::from([ShortText::new("build-agent").expect("name")]),
            mailboxes: BTreeSet::new(),
            retirements: BTreeSet::new(),
            lifecycle,
            runnable: false,
            selected_session: None,
            name_reserved: true,
        };
        Query(AuthoritativeSnapshot::new(
            Revision::new(1),
            DomainSnapshot::new(
                ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
                ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
                ProjectionSnapshot::new(
                    BTreeMap::new(),
                    BTreeMap::from([(
                        AgentProjectionKey::Agent(agent_id),
                        AgentProjection::Agent(Box::new(agent)),
                    )]),
                    BTreeMap::new(),
                ),
                ProjectionSnapshot::new(BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
            ),
        ))
    }

    #[test]
    fn foreground_resolver_binds_active_name_directory_and_private_options() {
        let agent_id = AgentId::from_bytes([1; 32]);
        let directory = std::env::current_dir().expect("current directory");
        let resolver = ForegroundCodexLaunchResolver {
            query: query(agent_id, AgentLifecycle::Active),
            executable: PathBuf::from("custom-codex"),
            model: Some("model-1".to_owned()),
            permissive: true,
        };
        let launch = resolver
            .resolve(&HarnessInstanceRequest {
                agent_id,
                project_id: None,
                launch_directory: Some(ResourceLocator::new(
                    ResourceScheme::WorkingTree,
                    hq_domain::BoundedText::new(directory.to_str().expect("UTF-8 test directory"))
                        .expect("locator"),
                )),
                environment: HarnessEnvironment::default(),
            })
            .expect("launch resolves");
        assert_eq!(launch.executable, PathBuf::from("custom-codex"));
        assert_eq!(launch.working_directory, directory);
        assert_eq!(launch.model.as_deref(), Some("model-1"));
        assert!(launch.permissive);
        assert!(launch.developer_instructions.contains("build-agent"));
        assert!(!format!("{launch:?}").contains("HQ_TOKEN"));
    }

    #[test]
    fn registry_has_codex_only_and_resolver_rejects_retired_or_unvalidated_paths() {
        let agent_id = AgentId::from_bytes([3; 32]);
        let registry = compose_codex_registry(
            query(agent_id, AgentLifecycle::Active),
            ForegroundCodexConfig::default(),
            Arc::new(Starter),
            Arc::new(Diagnostics),
        )
        .expect("registry composes");
        let capabilities = registry
            .capabilities(&ProviderId::new(CODEX_PROVIDER_ID).expect("provider"))
            .expect("Codex registered");
        assert!(
            capabilities
                .supported
                .contains(&HarnessCapability::ResumeSessions)
        );
        let catalog = registry.provider_catalog();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].provider.as_str(), CODEX_PROVIDER_ID);
        assert_eq!(catalog[0].name.as_str(), "Codex");

        let resolver = ForegroundCodexLaunchResolver {
            query: query(agent_id, AgentLifecycle::Retired),
            executable: PathBuf::from("codex"),
            model: None,
            permissive: false,
        };
        let error = resolver
            .resolve(&HarnessInstanceRequest {
                agent_id,
                project_id: None,
                launch_directory: Some(ResourceLocator::new(
                    ResourceScheme::WorkingTree,
                    hq_domain::BoundedText::new("relative").expect("locator"),
                )),
                environment: HarnessEnvironment::default(),
            })
            .expect_err("invalid path and retired agent reject");
        assert_eq!(error.class, HarnessErrorClass::InvalidInput);
    }
}
