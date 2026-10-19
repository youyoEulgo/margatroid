use async_runtime_plugin::AsyncTaskError;
use core_plugin::World;
use margatroid_types::agent_driver_events::{AgentDriverReadOutput, LoadAgentDriver};

use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use margatroid_types::discovery::{
    AgentDriverCatalogResult, AgentDriverCatalogSnapshot, AgentDriverDiscoverySnapshot,
    QueryAgentDriverCatalog, RefreshAgentDriverCatalog,
};
use margatroid_types::workspace_catalog::DriverCatalog;
use std::sync::atomic::{AtomicU64, Ordering};

static SCAN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct CatalogScanOutput {
    query_id: u64,
    id: String,
    result: Result<AgentDriverCatalogSnapshot, String>,
}
impl core_plugin::Event for CatalogScanOutput {}

#[derive(Default)]
struct CatalogPublication(u64);
impl core_plugin::Resource for CatalogPublication {}

pub(crate) fn start_catalog_scan(world: &mut World, id: String) {
    let config = world
        .get_resource::<crate::types::AgentDriverLoadConfig>()
        .expect("driver plugin is installed")
        .clone();
    let query_id = SCAN_ID.fetch_add(1, Ordering::Relaxed);
    world.send_async_closure(
        app_runtime_plugin::RuntimePlugin::PRE_UPDATE,
        move || async move {
            let result = crate::catalog::scan_validated(&config.root, &config.limits)
                .await
                .map_err(|e| e.to_string());
            Ok::<_, AsyncTaskError>(CatalogScanOutput {
                query_id,
                id,
                result,
            })
        },
    );
}

pub(crate) fn driver_catalog_system(world: &mut World) {
    let outputs = world
        .event_reader::<Result<CatalogScanOutput, AsyncTaskError>>()
        .into_iter()
        .map(|output| output.as_ref().cloned().map_err(ToString::to_string))
        .collect::<Vec<_>>();
    for output in outputs {
        match output {
            Ok(output) => {
                if let Ok(snapshot) = &output.result {
                    let previous = world
                        .get_resource::<CatalogPublication>()
                        .map_or(0, |s| s.0);
                    if output.query_id > previous {
                        world.insert_resource(DriverCatalog(snapshot.drivers.clone()));
                        world.insert_resource(AgentDriverDiscoverySnapshot(snapshot.clone()));
                        world.insert_resource(CatalogPublication(output.query_id));
                    }
                }
                world.send_event(AgentDriverCatalogResult {
                    id: output.id,
                    result: output.result,
                });
            }
            Err(error) => tracing::error!(%error, "driver catalog worker stopped"),
        }
    }
    let queries = world
        .event_reader::<QueryAgentDriverCatalog>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for query in queries {
        let snapshot = world
            .get_resource::<AgentDriverDiscoverySnapshot>()
            .expect("driver discovery snapshot is installed")
            .0
            .clone();
        world.send_event(AgentDriverCatalogResult {
            id: query.id,
            result: Ok(snapshot),
        });
    }
    let requests = world
        .event_reader::<RefreshAgentDriverCatalog>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        start_catalog_scan(world, request.id);
    }
}

pub(crate) fn load_agent_driver_system(world: &mut World) {
    let requests = world
        .event_reader::<LoadAgentDriver>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for request in requests {
        crate::handler::handle_load_agent_driver(world, request);
    }
    let outputs = world
        .event_reader::<Result<AgentDriverReadOutput, AsyncTaskError>>()
        .into_iter()
        .filter_map(|output| match output {
            Ok(output) => Some(output.clone()),
            Err(error) => {
                tracing::error!(%error, "driver async task stopped");
                None
            }
        })
        .collect::<Vec<_>>();
    for output in outputs {
        crate::handler::handle_mount_agent_driver(world, output);
    }
}
