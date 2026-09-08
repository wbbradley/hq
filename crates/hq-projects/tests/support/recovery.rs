//! Workflow tests use the production recovery store to exercise its transition fences.
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone)]
pub struct RecoveryDatabase(Arc<Database>);
struct Database {
    store: Option<hq_store::Store>,
    directory: PathBuf,
}
impl Default for RecoveryDatabase {
    fn default() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "hq-workflow-recovery-{}-{}",
            std::process::id(),
            NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("recovery directory");
        let store = hq_store::Store::open(
            directory.join("state").join("recovery.sqlite3"),
            NonZeroUsize::MIN,
        )
        .expect("recovery store");
        Self(Arc::new(Database {
            store: Some(store),
            directory,
        }))
    }
}
impl RecoveryDatabase {
    pub fn handle(&self) -> hq_store::ProjectRecoveryStateHandle {
        self.0
            .store
            .as_ref()
            .expect("open store")
            .project_recovery_state_handle()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            let _ = store.close();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
pub fn map_store_error(error: hq_store::StoreError) -> hq_projects::SagaStoreError {
    match error.class() {
        hq_store::StoreErrorClass::ProjectSagaConflict => hq_projects::SagaStoreError::Conflict,
        _ => hq_projects::SagaStoreError::Unavailable,
    }
}
macro_rules! recovery_store {
    ($store:ty) => {
        impl hq_projects::ProjectRecoveryStore for $store {
            fn recovery_find(
                &self,
                operation: hq_domain::OperationId,
            ) -> Result<Option<hq_application::ProjectRecoveryRecord>, SagaStoreError> {
                self.1
                    .handle()
                    .find(operation)
                    .map_err(recovery::map_store_error)
            }
            fn recovery_active(
                &self,
                project: hq_domain::ProjectId,
            ) -> Result<Option<hq_application::ProjectRecoveryRecord>, SagaStoreError> {
                self.1
                    .handle()
                    .active(project)
                    .map_err(recovery::map_store_error)
            }
            fn recovery_compare_exchange(
                &self,
                expected: Option<u64>,
                record: hq_application::ProjectRecoveryRecord,
                now: u64,
            ) -> Result<hq_application::ProjectRecoveryWriteOutcome, SagaStoreError> {
                self.1
                    .handle()
                    .compare_exchange(expected, record, now)
                    .map_err(recovery::map_store_error)
            }
            fn recovery_due(
                &self,
                now: u64,
                limit: usize,
            ) -> Result<Vec<hq_application::ProjectRecoveryRecord>, SagaStoreError> {
                self.1
                    .handle()
                    .due(now, limit)
                    .map_err(recovery::map_store_error)
            }
            fn recovery_deadline(&self) -> Result<Option<u64>, SagaStoreError> {
                self.1
                    .handle()
                    .next_deadline()
                    .map_err(recovery::map_store_error)
            }
            fn recovery_retry(
                &self,
                request: hq_application::ProjectRecoveryRetryRequest,
                now: u64,
            ) -> Result<hq_application::ProjectRecoveryWriteOutcome, SagaStoreError> {
                self.1
                    .handle()
                    .retry(request, now)
                    .map_err(recovery::map_store_error)
            }
        }
    };
}
pub(crate) use recovery_store;
