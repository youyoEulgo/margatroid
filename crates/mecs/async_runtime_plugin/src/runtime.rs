use std::sync::mpsc::sync_channel;
use std::thread;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

use crate::resource::{AsyncExecutorHandle, ErasedExecutionTask};
use crate::AsyncRuntimeError;

pub(crate) fn start_executor() -> AsyncExecutorHandle {
    let (task_sender, task_receiver) = unbounded_channel();
    let (startup_sender, startup_receiver) = sync_channel(1);
    let thread = thread::Builder::new()
        .name("mecs-async-runtime".into())
        .spawn(move || run_executor(task_receiver, startup_sender))
        .unwrap_or_else(|source| AsyncRuntimeError::ExecutorThreadStartFailed { source }.panic());

    match startup_receiver
        .recv()
        .expect("async executor startup confirmation channel disconnected")
    {
        Ok(()) => AsyncExecutorHandle::new(task_sender, thread),
        Err(source) => {
            thread
                .join()
                .expect("async executor thread panicked during startup");
            AsyncRuntimeError::ExecutorRuntimeBuildFailed { source }.panic()
        }
    }
}

fn run_executor(
    mut task_receiver: UnboundedReceiver<ErasedExecutionTask>,
    startup_sender: std::sync::mpsc::SyncSender<Result<(), std::io::Error>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = startup_sender.send(Err(error));
            return;
        }
    };
    if startup_sender.send(Ok(())).is_err() {
        return;
    }
    runtime.block_on(async move {
        while let Some(task) = task_receiver.recv().await {
            tokio::spawn(task);
        }
    });
}
