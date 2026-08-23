use async_runtime_plugin::AsyncContext;
use core_plugin::World;
use futures_util::FutureExt;

use crate::events::{
    CancelInferenceRequest, CapturedInferenceRequest, ContextCompactionInferenceRequest,
    InferenceOutputKind, InferenceTaskError, InferenceTaskOutput, ReloadModelRoutes,
};
use crate::handler::{
    handle_cancel_inference, handle_inference_task_output, handle_prepare_inference,
    handle_reload_model_routes, run_provider, InferenceCommand,
};
use crate::{InferenceError, InferenceErrorKind};
use margatroid_types::InferenceRequestEvent;

pub(crate) fn reload_model_routes_system(world: &mut World) {
    let requests = world
        .event_reader::<ReloadModelRoutes>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        handle_reload_model_routes(world, request);
    }
}

pub(crate) fn prepare_inference_system(world: &mut World) {
    let mut commands = world
        .event_reader::<InferenceRequestEvent>()
        .into_iter()
        .cloned()
        .map(|command| InferenceCommand {
            id: command.id,
            agent: command.agent,
            agent_id: command.agent_id,
            messages: command.messages,
            tools: command.tools,
            output: InferenceOutputKind::AgentMessage,
        })
        .collect::<Vec<_>>();
    commands.extend(
        world
            .event_reader::<ContextCompactionInferenceRequest>()
            .into_iter()
            .cloned()
            .map(|command| InferenceCommand {
                id: command.id,
                agent: command.agent,
                agent_id: command.agent_id,
                messages: command.messages,
                tools: Vec::new(),
                output: InferenceOutputKind::ContextCompaction,
            }),
    );
    commands.extend(
        world
            .event_reader::<CapturedInferenceRequest>()
            .into_iter()
            .cloned()
            .map(|command| InferenceCommand {
                id: command.id,
                agent: command.agent,
                agent_id: command.agent_id,
                messages: command.messages,
                tools: Vec::new(),
                output: InferenceOutputKind::Captured,
            }),
    );
    for command in commands {
        handle_prepare_inference(world, command);
    }
}

pub(crate) fn cancel_inference_system(world: &mut World) {
    let cancellations = world
        .event_reader::<CancelInferenceRequest>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for cancellation in cancellations {
        handle_cancel_inference(world, cancellation);
    }
}

pub(crate) async fn execute_prepared_inference(
    prepared: crate::events::PreparedInference,
    _context: AsyncContext,
) -> Result<InferenceTaskOutput, InferenceTaskError> {
    let route = prepared.route.clone();
    let mut cancellation = prepared.cancellation.clone();
    let provider = std::panic::AssertUnwindSafe(run_provider(prepared)).catch_unwind();
    let result = tokio::select! {
        biased;
        _ = cancellation.changed() => crate::events::InferenceTaskResult::Cancelled,
        result = provider => crate::events::InferenceTaskResult::Completed(result.unwrap_or_else(|_| {
            Err(InferenceError::new(
                InferenceErrorKind::TaskPanicked,
                "inference provider task panicked",
            ))
        })),
    };
    Ok(InferenceTaskOutput { route, result })
}

pub(crate) fn publish_inference_output_system(world: &mut World) {
    let mut outputs = Vec::new();
    for output in world.event_reader::<Result<InferenceTaskOutput, InferenceTaskError>>() {
        match output {
            Ok(output) => outputs.push(output.clone()),
            Err(error) => tracing::warn!(error = %error.source, "inference task was cancelled"),
        }
    }
    for output in outputs {
        handle_inference_task_output(world, output);
    }
}
