use crate::{WorkspaceError, WorkspaceErrorKind as K, WorkspaceExecutor};
use core_plugin::Entity;
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    fmt,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle, ThreadId},
};
use tokio::{
    sync::mpsc,
    task::{JoinSet, LocalSet},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceState {
    Starting,
    Running,
    Restarting,
    Stopping,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDefinition {
    pub name: String,
    pub project_root: PathBuf,
    #[serde(default)]
    pub members: Vec<WorkspaceMemberDefinition>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemberDefinition {
    pub name: String,
    pub driver: String,
    pub home_root: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberDiagnostic {
    pub name: String,
    pub error: WorkspaceError,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceOperationOutput {
    pub workspace: Entity,
    pub members: Vec<Entity>,
    pub member_errors: Vec<MemberDiagnostic>,
}

pub type WorkspaceLocalTask = Pin<Box<dyn Future<Output = ()> + 'static>>;
pub type LocalFuture = WorkspaceLocalTask;
pub type WorkspaceExecutionTask = Box<dyn FnOnce() -> WorkspaceLocalTask + Send + 'static>;
enum Command {
    Spawn(WorkspaceExecutionTask),
    Close,
}
struct Admission {
    closed: bool,
    close_sent: bool,
    sender: mpsc::UnboundedSender<Command>,
}
#[derive(Clone)]
pub struct WorkspaceExecutionEntry {
    admission: Arc<Mutex<Admission>>,
}
impl fmt::Debug for WorkspaceExecutionEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceExecutionEntry")
            .field("closed", &self.is_closed())
            .finish()
    }
}
impl WorkspaceExecutionEntry {
    pub fn submit(
        &self,
        task: impl FnOnce() -> WorkspaceLocalTask + Send + 'static,
    ) -> Result<(), WorkspaceError> {
        let mut admission = lock(&self.admission);
        if admission.closed {
            return Err(closed());
        }
        if admission
            .sender
            .send(Command::Spawn(Box::new(task)))
            .is_err()
        {
            admission.closed = true;
            return Err(closed());
        }
        Ok(())
    }
    pub fn close_admission(&self) {
        lock(&self.admission).closed = true;
    }
    pub fn is_closed(&self) -> bool {
        let admission = lock(&self.admission);
        admission.closed || admission.sender.is_closed()
    }
}
struct JoinState {
    handle: Option<JoinHandle<Result<(), WorkspaceError>>>,
    result: Option<Result<(), WorkspaceError>>,
}
struct SharedJoin {
    state: Mutex<JoinState>,
    completed: Condvar,
    thread: ThreadId,
}
#[derive(Clone)]
pub struct WorkspaceExecutionClose {
    entry: WorkspaceExecutionEntry,
    join: Arc<SharedJoin>,
}
impl fmt::Debug for WorkspaceExecutionClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceExecutionClose")
            .field("entry", &self.entry)
            .field("finished", &lock(&self.join.state).result.is_some())
            .finish()
    }
}
impl WorkspaceExecutionClose {
    pub async fn close_and_join(&self) -> Result<(), WorkspaceError> {
        if thread::current().id() == self.join.thread {
            return Err(WorkspaceError::new(
                K::ThreadCloseFailed,
                "cannot join workspace execution from its own thread",
            ));
        }
        {
            let mut admission = lock(&self.entry.admission);
            admission.closed = true;
            if !admission.close_sent {
                admission.close_sent = true;
                let _ = admission.sender.send(Command::Close);
            }
        }
        let join = self.join.clone();
        tokio::task::spawn_blocking(move || join.wait())
            .await
            .map_err(|e| WorkspaceError::new(K::ThreadCloseFailed, e))?
    }
    pub fn is_joined(&self) -> bool {
        lock(&self.join.state).result.is_some()
    }
}
impl SharedJoin {
    fn wait(&self) -> Result<(), WorkspaceError> {
        let mut state = lock(&self.state);
        loop {
            if let Some(result) = &state.result {
                return result.clone();
            }
            if let Some(handle) = state.handle.take() {
                drop(state);
                let result = handle.join().unwrap_or_else(|panic| {
                    Err(WorkspaceError::new(
                        K::ThreadCloseFailed,
                        format!("workspace thread panicked: {}", panic_message(panic)),
                    ))
                });
                state = lock(&self.state);
                state.result = Some(result.clone());
                self.completed.notify_all();
                return result;
            }
            state = self
                .completed
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
fn closed() -> WorkspaceError {
    WorkspaceError::new(K::StateRejected, "workspace task admission is closed")
}
fn panic_message(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).into()
    } else {
        "non-string panic payload".into()
    }
}
pub fn spawn_workspace_executor() -> Result<WorkspaceExecutor, WorkspaceError> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    let handle = thread::Builder::new()
        .name("workspace-execution".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let error = WorkspaceError::new(K::ThreadStartFailed, error);
                    let _ = ready_sender.send(Err(error.clone()));
                    return Err(error);
                }
            };
            let local = LocalSet::new();
            if ready_sender.send(Ok(())).is_err() {
                return Ok(());
            }
            runtime.block_on(local.run_until(run(receiver)))
        })
        .map_err(|e| WorkspaceError::new(K::ThreadStartFailed, e))?;
    match ready_receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = handle.join();
            return Err(error);
        }
        Err(error) => {
            let _ = handle.join();
            return Err(WorkspaceError::new(K::ThreadStartFailed, error));
        }
    }
    let thread = handle.thread().id();
    let entry = WorkspaceExecutionEntry {
        admission: Arc::new(Mutex::new(Admission {
            closed: false,
            close_sent: false,
            sender,
        })),
    };
    let close = WorkspaceExecutionClose {
        entry: entry.clone(),
        join: Arc::new(SharedJoin {
            state: Mutex::new(JoinState {
                handle: Some(handle),
                result: None,
            }),
            completed: Condvar::new(),
            thread,
        }),
    };
    Ok(WorkspaceExecutor { entry, close })
}
async fn run(mut receiver: mpsc::UnboundedReceiver<Command>) -> Result<(), WorkspaceError> {
    let mut tasks = JoinSet::new();
    let mut failure = None;
    loop {
        tokio::select! {
            command=receiver.recv()=>match command {
                Some(Command::Spawn(factory))=>{tasks.spawn_local(async move {factory().await;});},
                Some(Command::Close)|None=>{receiver.close();break;},
            },
            result=tasks.join_next(),if !tasks.is_empty()=>{record_result(result,&mut failure);}
        }
    }
    while let Some(result) = tasks.join_next().await {
        record_result(Some(result), &mut failure);
    }
    failure.map_or(Ok(()), Err)
}
fn record_result(
    result: Option<Result<(), tokio::task::JoinError>>,
    failure: &mut Option<WorkspaceError>,
) {
    if let Some(Err(error)) = result {
        if failure.is_none() {
            *failure = Some(if error.is_panic() {
                WorkspaceError::new(
                    K::ThreadCloseFailed,
                    format!(
                        "workspace task panicked: {}",
                        panic_message(error.into_panic())
                    ),
                )
            } else {
                WorkspaceError::new(K::ThreadCloseFailed, error)
            });
        }
    }
}
