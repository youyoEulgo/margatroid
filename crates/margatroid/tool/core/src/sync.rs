use crate::{ToolError, ToolTaskSnapshot};
use client_core::{ClientFrame, ClientPrepareError, ClientSelector, Clients, SyncContext, SyncKey};
use core_plugin::Entity;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug, serde::Serialize)]
pub struct AgentTaskSnapshot {
    pub address: String,
    pub tasks: Vec<ToolTaskSnapshot>,
}
type TaskEncoder =
    Arc<dyn Fn(AgentTaskSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync>;
#[derive(Clone)]
pub struct TaskSyncBinding {
    pub clients: Clients,
    targets: Arc<[ClientSelector]>,
    encode: TaskEncoder,
}
impl std::fmt::Debug for TaskSyncBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskSyncBinding").finish_non_exhaustive()
    }
}
impl TaskSyncBinding {
    pub fn new(
        clients: Clients,
        targets: Arc<[ClientSelector]>,
        encode: impl Fn(AgentTaskSnapshot) -> Result<Arc<ClientFrame>, ClientPrepareError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            clients,
            targets,
            encode: Arc::new(encode),
        }
    }
    pub(crate) fn bind(
        &self,
        owner: Entity,
        address: String,
        state: Weak<crate::components::TaskSetShared>,
    ) -> TaskUpdateContext {
        let shared = state.upgrade().expect("new task collection missing");
        let gate = shared.gate.clone();
        let revision = shared.revision.clone();
        drop(shared);
        let encode = self.encode.clone();
        let public_address = address.clone();
        let sync = SyncContext::new(
            self.clients.clone(),
            SyncKey {
                topic: "agent.task.sync".into(),
                object: address,
            },
            self.targets.clone(),
            move || {
                let state = state.upgrade().ok_or(ClientPrepareError::SourceClosed)?;
                let _gate = crate::types::lock(&state.gate);
                let tasks = crate::types::lock(&state.entries)
                    .iter()
                    .filter_map(|(id, handle)| handle.snapshot(id))
                    .collect();
                Ok((
                    state.revision.load(Ordering::Acquire),
                    AgentTaskSnapshot {
                        address: public_address.clone(),
                        tasks,
                    },
                ))
            },
            move |snapshot| encode(snapshot),
        );
        TaskUpdateContext {
            owner,
            gate,
            revision,
            sync,
        }
    }
}
#[derive(Clone)]
pub struct TaskUpdateContext {
    pub owner: Entity,
    pub(crate) gate: Arc<Mutex<()>>,
    pub(crate) revision: Arc<AtomicU64>,
    sync: SyncContext<AgentTaskSnapshot>,
}
impl std::fmt::Debug for TaskUpdateContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskUpdateContext")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}
impl TaskUpdateContext {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
    }
    pub(crate) fn commit(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub(crate) fn publish(&self, revision: u64) {
        self.sync.publish(revision);
    }
    pub fn close(&self) {
        self.sync.close();
    }
}

pub(crate) fn update_task<R>(
    context: &TaskUpdateContext,
    change: impl FnOnce() -> Result<(R, bool), ToolError>,
) -> Result<R, ToolError> {
    let gate = crate::types::lock(&context.gate);
    let (result, changed) = change()?;
    let revision = changed.then(|| context.commit());
    drop(gate);
    if let Some(revision) = revision {
        context.publish(revision);
    }
    Ok(result)
}
