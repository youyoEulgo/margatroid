use std::fs;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Component;
use std::sync::{Arc, Mutex};

use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use tool_resource_core::{
    read_tool, ImportTool, ToolEntry, ToolResourceError, ToolResourceErrorKind as Kind, Tools,
};

use crate::types::{ImportConfig, ImportContext, ImportError, ImportOutput, ImportTaskError};

pub(crate) fn handle_import(world: &mut World, request: ImportTool) {
    let Some(reply) = request
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let invalid = if !world.is_alive(request.owner) {
        Some(ToolResourceError::new(
            Kind::OwnerMissing,
            "owner entity is no longer alive",
        ))
    } else if !valid_alias(&request.alias) {
        Some(ToolResourceError::new(Kind::InvalidAlias, "alias must start with an ASCII letter or underscore and contain only letters, digits, underscores or hyphens"))
    } else if request.cancel.is_cancelled() {
        Some(ToolResourceError::new(Kind::Cancelled, "import cancelled"))
    } else {
        None
    };
    if let Some(error) = invalid {
        let _ = reply.send(Err(error));
        return;
    }
    let context = ImportContext {
        owner: request.owner,
        alias: request.alias,
        path: request.path,
        cancel: request.cancel,
        limits: world
            .get_resource::<ImportConfig>()
            .expect("resource import configuration must exist")
            .limits,
        reply: Arc::new(Mutex::new(Some(reply))),
    };
    let fallback = context.clone();
    let schedule = world
        .get_resource::<ImportConfig>()
        .expect("import configuration")
        .schedule
        .clone();
    let guard = ImportReplyGuard(Some(context.clone()));
    let submission = catch_unwind(AssertUnwindSafe(|| {
        world.send_async_closure(&schedule, move || async move {
            let mut guard = guard;
            let result = validate_import(context)
                .await
                .map_err(ImportTaskError::Validation);
            guard.0 = None;
            result
        });
    }));
    if submission.is_err() {
        let error =
            ToolResourceError::new(Kind::WorkerFailed, "failed to submit resource validation")
                .with_path(&fallback.path);
        respond(&fallback, Err(error));
    }
}

pub(crate) fn handle_import_result(world: &mut World, result: Result<ImportOutput, ImportError>) {
    let (context, validated) = match result {
        Ok(output) => (
            output.context,
            Ok(ToolEntry {
                path: output.path,
                description: output.description,
                input_schema: output.input_schema,
                owner_task_id: None,
                binding: None,
            }),
        ),
        Err(error) => (error.context, Err(error.error)),
    };
    let Some(reply) = context
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let result = if !world.is_alive(context.owner) {
        Err(ToolResourceError::new(
            Kind::OwnerMissing,
            "owner entity is no longer alive",
        ))
    } else if context.cancel.is_cancelled() {
        Err(ToolResourceError::new(
            Kind::Cancelled,
            "import cancelled before publication",
        ))
    } else {
        validated.and_then(|entry| {
            if !world.contains_component::<Tools>(context.owner) {
                world.insert_component(context.owner, Tools::default());
            }
            world
                .get_component_mut::<Tools>(context.owner)
                .expect("owner collection must exist")
                .insert(context.alias.clone(), entry)
        })
    };
    let result = result.map_err(|mut error| {
        error.message = format!("alias {}: {}", context.alias, error.message);
        error
    });
    let _ = reply.send(result);
}

async fn validate_import(context: ImportContext) -> Result<ImportOutput, ImportError> {
    let path = context.path.clone();
    let cancel = context.cancel.clone();
    let limits = context.limits;
    let worker = tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(
                ToolResourceError::new(Kind::Cancelled, "import cancelled").with_path(&path)
            );
        }
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(ToolResourceError::new(
                Kind::InvalidPath,
                "entry path must be absolute without parent traversal",
            )
            .with_path(&path));
        }
        let canonical = fs::canonicalize(&path).map_err(|error| {
            let kind = if error.kind() == io::ErrorKind::NotFound {
                Kind::FileMissing
            } else {
                Kind::ReadFailed
            };
            ToolResourceError::new(kind, error.to_string()).with_path(&path)
        })?;
        let input = read_tool(&canonical, &limits)?;
        Ok((canonical, input.description, input.input_schema))
    });
    let result = match worker.await {
        Ok(result) => result,
        Err(_) => Err(ToolResourceError::new(
            Kind::WorkerFailed,
            "resource validation worker failed",
        )
        .with_path(&context.path)),
    };
    match result {
        Ok((path, description, input_schema)) => Ok(ImportOutput {
            context,
            path,
            description,
            input_schema,
        }),
        Err(error) => Err(ImportError { context, error }),
    }
}

fn valid_alias(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn respond(context: &ImportContext, result: Result<(), ToolResourceError>) {
    if let Some(reply) = context
        .reply
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    {
        let _ = reply.send(result);
    }
}

struct ImportReplyGuard(Option<ImportContext>);
impl Drop for ImportReplyGuard {
    fn drop(&mut self) {
        if let Some(context) = self.0.take() {
            respond(
                &context,
                Err(ToolResourceError::new(
                    Kind::WorkerFailed,
                    "resource validation task failed before returning its result",
                )
                .with_path(&context.path)),
            );
        }
    }
}
