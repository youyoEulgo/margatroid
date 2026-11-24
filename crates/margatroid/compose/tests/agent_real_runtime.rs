#[path = "unified_lua.rs"]
mod runtime_support;
use address::WorldAddressExt;
use agent_core::Agent;
use app_runtime_plugin::WorldEventExt;
use runtime_support::{DRIVER, Harness, drive, until};
use serde_json::json;
use std::sync::{Arc, Mutex};
use storage_core::{
    AgentStorage, MessageSelection, MessageSelector, StorageOperation, StorageValue,
};
use tokio::sync::oneshot;
use tool_core::{ToolExecuteRequest, ToolTasks};

fn messages(
    h: &Harness,
    agent: core_plugin::Entity,
    block: &str,
    instance: Option<String>,
) -> Result<StorageValue, storage_core::StorageError> {
    storage_core::execute_storage(
        h.app.world().get_component::<AgentStorage>(agent).unwrap(),
        StorageOperation::ReadMessages {
            selectors: vec![MessageSelector {
                block: block.into(),
                field: "messages".into(),
                instance,
                selection: MessageSelection::All,
            }],
        },
    )
}
#[test]
fn real_driver_reuses_persistent_blocks_but_allocates_fresh_runtime_and_temp_namespace() {
    let mut h = Harness::open();
    let driver = h.driver("storage", DRIVER);
    let first = h.start(
        h.workspace,
        &driver,
        json!({"node_name":"coder","append":"first"}),
    );
    let instance = h
        .app
        .world()
        .get_component::<Agent>(first)
        .unwrap()
        .instance()
        .to_owned();
    let stored = h
        .app
        .world()
        .get_component::<AgentStorage>(first)
        .unwrap()
        .clone();
    let StorageValue::Messages(items) = messages(&h, first, "saved", None).unwrap() else {
        panic!("messages expected")
    };
    assert_eq!(items.len(), 1);
    let StorageValue::Messages(items) =
        messages(&h, first, "transient", Some(instance.clone())).unwrap()
    else {
        panic!("messages expected")
    };
    assert_eq!(items.len(), 1);
    h.stop(first);
    assert!(!stored.is_open());
    let second = h.start(h.workspace, &driver, json!({"node_name":"coder"}));
    let next_instance = h
        .app
        .world()
        .get_component::<Agent>(second)
        .unwrap()
        .instance()
        .to_owned();
    assert_ne!(instance, next_instance);
    assert_ne!(first, second);
    let StorageValue::Messages(items) = messages(&h, second, "saved", None).unwrap() else {
        panic!("messages expected")
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["content"], "first");
    let StorageValue::Messages(items) =
        messages(&h, second, "transient", Some(next_instance)).unwrap()
    else {
        panic!("messages expected")
    };
    assert_eq!(items.len(), 1);
    assert!(messages(&h, second, "transient", Some(instance)).is_err());
    h.close();
}
#[test]
fn cancellation_during_creation_handoff_waits_for_all_agent_connections() {
    let mut h = Harness::open();
    let driver = h.driver("cancel", DRIVER);
    let (tx, mut response) = oneshot::channel();
    h.app.world().send_event(ToolExecuteRequest::Path {
        initiator: h.workspace,
        path: driver,
        arguments: json!({"node_name":"cancelled"}),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    until(&mut h.app, |world| {
        !world.query_with::<Agent>().result().is_empty()
    });
    let agent = h.app.world().entity_by_address("/demo/cancelled").unwrap();
    let storage = h
        .app
        .world()
        .get_component::<AgentStorage>(agent)
        .unwrap()
        .clone();
    let task = h.task(agent);
    h.finish(task.clone());
    assert!(task.is_cleanup_complete());
    assert!(!h.app.world().is_alive(agent));
    assert!(!storage.is_open());
    assert!(
        h.app
            .world()
            .get_component::<ToolTasks>(h.workspace)
            .unwrap()
            .list()
            .is_empty()
    );
    let _ = drive(&mut h.app, &mut response);
    h.close();
}
#[test]
fn stale_task_source_cannot_execute_mcl_against_a_recreated_node() {
    let mut h = Harness::open();
    let driver = h.driver("source", DRIVER);
    let first = h.start(h.workspace, &driver, json!({"node_name":"coder"}));
    let stale = h.task(first).execution_id().to_owned();
    h.stop(first);
    let second = h.start(h.workspace, &driver, json!({"node_name":"coder"}));
    let (tx, mut rx) = oneshot::channel();
    h.app.world().send_event(mcl_core::MclCommand {
        id: "stale-call".into(),
        source: h.workspace,
        source_address: "/demo".into(),
        source_instance: Some(stale),
        target: second,
        instruction: mcl_core::MclInstruction {
            text: "GET saved.messages".into(),
            arguments: vec![],
        },
        cancel: Default::default(),
        reply: Arc::new(Mutex::new(Some(tx))),
    });
    let reply = drive(&mut h.app, &mut rx).unwrap();
    assert!(matches!(reply, mcl_core::MclValue::Error(_)));
    h.close();
}
