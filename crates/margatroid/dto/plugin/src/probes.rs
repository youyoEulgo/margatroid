use crate::projection;
use agent_core::{AgentUpdateContext, AgentUpdateKind};
use client_core::{ClientFrame, ClientPrepareError, ClientSelector, Clients, SyncContext, SyncKey};
use core_plugin::{Entity, World};
use margatroid_protocol::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

type View<T> = Arc<Mutex<(u64, T)>>;
pub(crate) fn state_source<T: Clone + Send + 'static>(
    clients: Clients,
    targets: Arc<[ClientSelector]>,
    key: SyncKey,
    view: &View<T>,
    encode: impl Fn(T) -> Result<Arc<ClientFrame>, ClientPrepareError> + Send + Sync + 'static,
) -> SyncContext<T> {
    let weak = Arc::downgrade(view);
    SyncContext::new(
        clients,
        key,
        targets,
        move || {
            let view = weak.upgrade().ok_or(ClientPrepareError::SourceClosed)?;
            let state = view.lock().unwrap_or_else(|e| e.into_inner());
            Ok((state.0, state.1.clone()))
        },
        encode,
    )
}
fn encoded(message: ServerMessage) -> Result<Arc<ClientFrame>, ClientPrepareError> {
    encode_server_message(&message)
        .map(|text| Arc::new(ClientFrame::Text(text)))
        .map_err(|_| ClientPrepareError::TooLarge)
}
pub(crate) fn update<T: PartialEq + Send + 'static>(
    view: &View<T>,
    context: &SyncContext<T>,
    value: T,
) {
    let mut state = view.lock().unwrap_or_else(|e| e.into_inner());
    if state.1 == value {
        return;
    }
    state.0 += 1;
    state.1 = value;
    let revision = state.0;
    drop(state);
    context.publish(revision);
}
struct TurnView {
    view: View<AgentTurnStateDto>,
    sync: SyncContext<AgentTurnStateDto>,
}
struct AgentViews {
    clients: Clients,
    targets: Arc<[ClientSelector]>,
    view: View<Vec<AgentStateDto>>,
    sync: SyncContext<Vec<AgentStateDto>>,
    turns: Mutex<BTreeMap<Entity, TurnView>>,
    addresses: Mutex<BTreeMap<Entity, String>>,
}
impl Drop for AgentViews {
    fn drop(&mut self) {
        self.sync.close();
        for value in self
            .turns
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            value.sync.close();
        }
    }
}

pub fn agent_sync_context(
    clients: Clients,
    targets: &[config_core::OutboundTarget],
) -> AgentUpdateContext {
    let targets = crate::sync_selectors(targets);
    let view = Arc::new(Mutex::new((0, Vec::<AgentStateDto>::new())));
    let sync = state_source(
        clients.clone(),
        targets.clone(),
        SyncKey {
            topic: "agent.sync".into(),
            object: "all".into(),
        },
        &view,
        |agents| encoded(ServerMessage::AgentSync { agents }),
    );
    let state = Arc::new(AgentViews {
        clients,
        targets,
        view,
        sync,
        turns: Mutex::default(),
        addresses: Mutex::default(),
    });
    let public = state.clone();
    AgentUpdateContext::new(
        move |world, owner, kind| {
            if let Some(address) = crate::projection::address(world, owner) {
                state.addresses.lock().unwrap().insert(owner, address);
            } else {
                state.addresses.lock().unwrap().remove(&owner);
            }
            update(&state.view, &state.sync, projection::agents(world));
            {
                let mut turns = state.turns.lock().unwrap();
                if let Some(turn) = projection::turn(world, owner) {
                    if let Some(current) = turns.get(&owner) {
                        update(&current.view, &current.sync, turn);
                    } else {
                        let key = SyncKey {
                            topic: "agent.turn.sync".into(),
                            object: turn.address.clone(),
                        };
                        let view = Arc::new(Mutex::new((0, turn)));
                        let sync = state_source(
                            state.clients.clone(),
                            state.targets.clone(),
                            key,
                            &view,
                            |value: AgentTurnStateDto| {
                                encoded(ServerMessage::AgentTurnSync {
                                    address: value.address,
                                    turn: value.turn,
                                })
                            },
                        );
                        turns.insert(owner, TurnView { view, sync });
                    }
                } else if let Some(old) = turns.remove(&owner) {
                    old.sync.close();
                }
            }
            match kind {
                AgentUpdateKind::Changed => vec![],
                AgentUpdateKind::RuntimeInstalled => {
                    let mut waits = crate::ui::refresh_ui_watch(world, owner);
                    waits.extend(crate::records::refresh_record_watch(world, owner));
                    waits
                }
                AgentUpdateKind::RuntimeStopped | AgentUpdateKind::Removed => {
                    if let Some(tasks) = world.get_component::<tool_core::ToolTasks>(owner) {
                        tasks.close_sync();
                    }
                    let mut waits = crate::ui::stop_ui_watch(world, owner);
                    waits.extend(crate::records::stop_record_watch(world, owner));
                    waits
                }
            }
        },
        move |owner, value| {
            let Some(address) = public.addresses.lock().unwrap().get(&owner).cloned() else {
                return;
            };
            let mut view = public.view.lock().unwrap();
            let Some(row) = view.1.iter_mut().find(|row| row.address == address) else {
                return;
            };
            let before = row.clone();
            match value {
                agent_core::AgentPublicUpdate::Tools(tools) => {
                    row.tools = tools
                        .into_iter()
                        .map(|tool| AgentToolDto {
                            alias: tool.alias,
                            path: tool.path.to_string_lossy().into_owned(),
                            description: tool.description,
                            input_schema: tool.input_schema,
                            owner_task_id: tool.owner_task_id,
                            binding: tool.binding,
                        })
                        .collect()
                }
                agent_core::AgentPublicUpdate::Sandboxes(values) => {
                    row.sandboxes = values
                        .into_iter()
                        .map(|(alias, path)| AgentSandboxDto {
                            alias,
                            path: path.to_string_lossy().into_owned(),
                        })
                        .collect()
                }
                agent_core::AgentPublicUpdate::Sandbox(value) => row.sandbox = value,
                agent_core::AgentPublicUpdate::Model(value) => {
                    row.model = value.map(|model| AgentModelDto {
                        id: model.id,
                        model: model.model,
                        provider: model.provider,
                        api_type: model.api_type,
                        context_window_tokens: model.context_window_tokens,
                        thinking: model.thinking,
                        reasoning_effort: model.reasoning_effort,
                    })
                }
            }
            if *row == before {
                return;
            }
            view.0 += 1;
            let revision = view.0;
            drop(view);
            public.sync.publish(revision);
        },
    )
}
pub(crate) fn initialize_agent_sources(world: &mut World) {
    let owners = world.query_with::<agent_core::Agent>().result();
    for owner in owners {
        let context = world
            .get_component::<agent_core::Agent>(owner)
            .unwrap()
            .updates();
        let waits = context.notify(world, owner, AgentUpdateKind::Changed);
        assert!(waits.is_empty());
        let waits = context.notify(world, owner, AgentUpdateKind::RuntimeInstalled);
        assert!(
            waits.is_empty(),
            "initial observer setup cannot replace an existing source"
        );
    }
}
pub(crate) fn get_or_create_agent_context(world: &mut World) -> AgentUpdateContext {
    if let Some(context) = world.get_resource::<AgentUpdateContext>() {
        return context.clone();
    }
    let clients = world
        .get_resource::<Clients>()
        .expect("Clients required")
        .clone();
    let context = agent_sync_context(
        clients,
        world
            .get_resource::<config_core::MargatroidConfig>()
            .expect("configuration required")
            .backend_state(),
    );
    world.insert_resource(context.clone());
    context
}

pub fn resource_sync_bindings(
    context: AgentUpdateContext,
) -> (
    tool_resource_core::ToolResourceSyncBinding,
    sandbox_resource_core::SandboxSyncBinding,
    sandbox_resource_core::SandboxSelectionSyncBinding,
) {
    let tools = context.clone();
    let sandboxes = context.clone();
    (
        client_core::ProbeFactory::new(move |owner| {
            let context = tools.scoped(owner);
            client_core::UpdateProbe::new(
                move |values: Vec<(String, tool_resource_core::ToolEntry)>| {
                    context.update_public(
                        owner,
                        agent_core::AgentPublicUpdate::Tools(
                            values
                                .into_iter()
                                .map(|(alias, entry)| agent_core::PublicToolInfo {
                                    alias,
                                    path: entry.path,
                                    description: entry.description,
                                    input_schema: entry.input_schema,
                                    owner_task_id: entry.owner_task_id,
                                    binding: entry.binding,
                                })
                                .collect(),
                        ),
                    )
                },
            )
        }),
        client_core::ProbeFactory::new(move |owner| {
            let context = sandboxes.scoped(owner);
            client_core::UpdateProbe::new(move |values| {
                context.update_public(owner, agent_core::AgentPublicUpdate::Sandboxes(values))
            })
        }),
        client_core::ProbeFactory::new(move |owner| {
            let context = context.scoped(owner);
            client_core::UpdateProbe::new(move |value| {
                context.update_public(owner, agent_core::AgentPublicUpdate::Sandbox(value))
            })
        }),
    )
}
pub(crate) fn install_resource_bindings(world: &mut World, context: AgentUpdateContext) {
    if !world.contains_resource::<inference_core::ModelSyncBinding>() {
        world.insert_resource(model_sync_binding(context.clone()));
    }
    let (tools, sandboxes, selection) = resource_sync_bindings(context);
    if !world.contains_resource::<tool_resource_core::ToolResourceSyncBinding>() {
        world.insert_resource(tools);
    }
    if !world.contains_resource::<sandbox_resource_core::SandboxSyncBinding>() {
        world.insert_resource(sandboxes);
    }
    if !world.contains_resource::<sandbox_resource_core::SandboxSelectionSyncBinding>() {
        world.insert_resource(selection);
    }
}

pub fn model_sync_binding(context: AgentUpdateContext) -> inference_core::ModelSyncBinding {
    client_core::ProbeFactory::new(move |owner| {
        let context = context.scoped(owner);
        client_core::UpdateProbe::new(move |value: Option<inference_core::PublicModelInfo>| {
            context.update_public(
                owner,
                agent_core::AgentPublicUpdate::Model(value.map(|model| {
                    agent_core::PublicModelInfo {
                        id: model.id,
                        model: model.model,
                        provider: model.provider,
                        api_type: model.api_type,
                        context_window_tokens: model.context_window_tokens,
                        thinking: model.thinking,
                        reasoning_effort: model.reasoning_effort,
                    }
                })),
            )
        })
    })
}
