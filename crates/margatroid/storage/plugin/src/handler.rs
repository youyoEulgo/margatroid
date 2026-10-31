use crate::{
    types::{Reply, StorageContext, StorageOutput, StorageTaskError},
    StorageInstalled,
};
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc, Mutex},
};
use storage_core::{
    execute_storage, AgentStorage, StorageError, StorageErrorKind as K, StorageRequest,
    StorageValue,
};

fn deliver(reply: &Reply, result: Result<StorageValue, StorageError>) {
    if let Some(sender) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = sender.send(result);
    }
}
pub(crate) fn handle_storage_request(world: &mut World, request: StorageRequest) {
    let Some(sender) = request
        .reply
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    let reply = Arc::new(Mutex::new(Some(sender)));
    let Some(storage) = world.get_component::<AgentStorage>(request.owner).cloned() else {
        deliver(
            &reply,
            Err(StorageError::new(
                K::OwnerMissing,
                "storage owner is missing",
            )),
        );
        return;
    };
    if request.cancel.is_cancelled() {
        deliver(
            &reply,
            Err(StorageError::new(
                K::Cancelled,
                "request cancelled before worker submission",
            )),
        );
        return;
    }
    let context = StorageContext {
        owner: request.owner,
        storage,
        operation: request.operation,
        cancel: request.cancel,
        reply: reply.clone(),
        pending: crate::types::PendingReply {
            reply: reply.clone(),
            active: true,
        },
    };
    let schedule = world
        .get_resource::<StorageInstalled>()
        .unwrap()
        .schedule
        .clone();
    let submitted = catch_unwind(AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || storage_task(context))
    }));
    if submitted.is_err() {
        deliver(
            &reply,
            Err(StorageError::new(
                K::WorkerFailed,
                "storage task submission failed",
            )),
        );
    }
}
pub(crate) fn handle_storage_result(
    world: &mut World,
    result: Result<StorageOutput, StorageTaskError>,
) {
    match result {
        Ok(output) => {
            let _owner_present = world.is_alive(output.owner);
            deliver(&output.reply, Ok(output.value.clone()));
        }
        Err(error) => {
            let _owner_present = error.owner.is_some_and(|owner| world.is_alive(owner));
            if let Some(reply) = &error.reply {
                deliver(reply, Err(error.error.clone()));
            }
        }
    }
}
struct WorkerGuard {
    completion: mpsc::Receiver<Result<StorageValue, StorageError>>,
    reply: Reply,
    active: bool,
    worker: Option<tokio::task::JoinHandle<()>>,
}
struct ThreadWake(std::thread::Thread);
impl std::task::Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if self.active {
            if let Some(worker) = self.worker.as_mut() {
                let waker = std::task::Waker::from(Arc::new(ThreadWake(std::thread::current())));
                let mut context = std::task::Context::from_waker(&waker);
                while std::future::Future::poll(std::pin::Pin::new(&mut *worker), &mut context)
                    .is_pending()
                {
                    std::thread::park();
                }
            }
            let result = self.completion.recv().unwrap_or_else(|_| {
                Err(StorageError::new(
                    K::CommitUnknown,
                    "storage worker exited without a result",
                ))
            });
            deliver(&self.reply, result);
        }
    }
}
async fn storage_task(context: StorageContext) -> Result<StorageOutput, StorageTaskError> {
    let StorageContext {
        owner,
        storage,
        operation,
        cancel,
        reply,
        mut pending,
    } = context;
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut guard = WorkerGuard {
        completion: receiver,
        reply: reply.clone(),
        active: true,
        worker: None,
    };
    pending.active = false;
    guard.worker = Some(tokio::task::spawn_blocking(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            if cancel.is_cancelled() {
                Err(StorageError::new(
                    K::Cancelled,
                    "request cancelled before execution",
                ))
            } else {
                execute_storage(&storage, operation)
            }
        }))
        .unwrap_or_else(|_| {
            Err(StorageError::new(
                K::CommitUnknown,
                "storage worker panicked; commit fact is unknown",
            ))
        });
        let _ = sender.send(result);
    }));
    let joined = guard.worker.as_mut().unwrap().await;
    let result = guard.completion.recv().unwrap_or_else(|_| {
        Err(StorageError::new(
            K::CommitUnknown,
            "storage worker exited without a result",
        ))
    });
    guard.active = false;
    if let Err(error) = joined {
        return Err(StorageTaskError {
            owner: Some(owner),
            reply: Some(reply),
            error: StorageError::new(K::CommitUnknown, error),
        });
    }
    match result {
        Ok(value) => Ok(StorageOutput {
            owner,
            reply,
            value,
        }),
        Err(error) => Err(StorageTaskError {
            owner: Some(owner),
            reply: Some(reply),
            error,
        }),
    }
}
