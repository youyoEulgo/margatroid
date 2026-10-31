use std::fs;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Component;
use std::sync::{Arc, Mutex};

use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::World;
use tool_resource_core::{
    read_tool, ImportTool, ToolResourceError, ToolResourceErrorKind as Kind, Tools,
};

use crate::types::{ImportConfig, ImportContext, ImportError, ImportOutput};

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
    let events = world.event_sender();
    let submission = catch_unwind(AssertUnwindSafe(|| {
        world.spawn_async_service(async move {
            events.send_event(validate_import(context).await);
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
        Ok(output) => (output.context, Ok(output.path)),
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
        validated.map(|path| {
            if !world.contains_component::<Tools>(context.owner) {
                world.insert_component(context.owner, Tools::default());
            }
            world
                .get_component_mut::<Tools>(context.owner)
                .expect("owner collection must exist")
                .insert(context.alias.clone(), path);
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
        read_tool(&canonical, &limits)?;
        Ok(canonical)
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
        Ok(path) => Ok(ImportOutput { context, path }),
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
