//! Independently locked exact-owner control handles, separate from mutable worker I/O.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use hq_domain::AgentId;

use crate::{HarnessError, HarnessErrorClass, HarnessOperationControl, HarnessReadyWorker};

#[derive(Clone)]
pub(super) struct RegisteredControl {
    pub(super) worker: HarnessReadyWorker,
    pub(super) control: Arc<dyn HarnessOperationControl>,
}

#[derive(Default)]
pub(super) struct WorkerControls(Mutex<BTreeMap<AgentId, RegisteredControl>>);

impl WorkerControls {
    pub(super) fn insert(&self, control: RegisteredControl) -> Result<(), HarnessError> {
        self.0
            .lock()
            .map_err(|_| unavailable())?
            .insert(control.worker.agent_id, control);
        Ok(())
    }

    pub(super) fn get(&self, agent: AgentId) -> Result<Option<RegisteredControl>, HarnessError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| unavailable())?
            .get(&agent)
            .cloned())
    }

    pub(super) fn remove(&self, agent: AgentId) {
        if let Ok(mut controls) = self.0.lock() {
            controls.remove(&agent);
        }
    }

    pub(super) fn clear(&self) {
        if let Ok(mut controls) = self.0.lock() {
            controls.clear();
        }
    }
}

fn unavailable() -> HarnessError {
    HarnessError::new(HarnessErrorClass::Unavailable)
}
