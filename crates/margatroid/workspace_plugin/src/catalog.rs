use crate::*;
use app_runtime_plugin::WorldEventExt;
use async_runtime_plugin::WorldAsyncExt;
use core_plugin::{Entity, Event, Resource, World};
use margatroid_types::agent_driver_events::{LoadAgentDriver, LoadAgentDriverResult};
use margatroid_types::agent_events::{
    AgentStartDefinition, StartAgent, StartAgentResult, StopAgent, StopAgentResult,
};
use margatroid_types::workspace_catalog::{
    workspace_resource_id, DriverCatalog, WorkspaceCatalog, WorkspaceCatalogEntry,
    WorkspaceCatalogHandle, WorkspaceDeletion,
};
use resource_id_plugin::ResourceID;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-workspace operation leases plus in-flight request bookkeeping.
///
/// Only the active stage of each operation lives here; Agent lifecycle stays in components.
#[derive(Default)]
pub(crate) struct Operations {
    /// Logical workspace name held by a lease, mapped to the owning request ID.
    leases: BTreeMap<ResourceID, (Option<String>, String)>,
    pending: BTreeMap<String, Operation>,
    /// Registered start request ID mapped to its lifecycle request ID.
    runtime_starts: BTreeMap<String, String>,
    /// Member add that also starts the member on a live workspace: runtime request ID
    /// mapped to the lifecycle request ID.
    member_runs: BTreeMap<String, String>,
    refreshing: bool,
}
impl Resource for Operations {}

#[derive(Clone, Debug)]
enum Action {
    Create,
    Rename {
        rename: RenameWorkspace,
        /// Snapshot the rename must agree with before the directory move.
        snapshot: WorkspaceDefinition,
    },
    Delete,
    Start,
    Add(WorkspaceMemberSpec),
    RenameMember {
        member: ResourceID,
        name: String,
    },
    Remove(ResourceID),
}

#[derive(Clone, Debug)]
enum Stage {
    Loading,
    Saving,
    Starting,
    /// A member add on a live workspace loads its driver, then starts the member.
    AddingMember {
        entity: Entity,
        request: String,
    },
    StartingMember {
        entity: Entity,
        request: String,
    },
    StoppingMember {
        entity: Entity,
        agent: Entity,
        request: String,
    },
    /// Member rename: persist the renamed definition with the revision the drain started from.
    SavingRename,
    /// Member removal: persist the definition without the stopped member.
    SavingRemove,
}

#[derive(Clone, Debug)]
struct Operation {
    workspace: ResourceID,
    action: Action,
    entry: Option<WorkspaceCatalogEntry>,
    stage: Stage,
    stopped: bool,
    restart: bool,
}

#[derive(Clone, Debug)]
enum StorageOutput {
    Refresh(String, Result<Vec<WorkspaceCatalogEntry>, WorkspaceError>),
    Loaded(String, Result<WorkspaceCatalogEntry, WorkspaceError>),
    Saved(String, Result<WorkspaceCatalogEntry, WorkspaceError>),
    Renamed(String, Result<WorkspaceCatalogEntry, WorkspaceError>),
    Deleted(String, Result<WorkspaceDeletion, WorkspaceError>),
}
impl Event for StorageOutput {}

fn err(kind: WorkspaceErrorKind, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::new(kind, message)
}
fn busy() -> WorkspaceError {
    err(
        WorkspaceErrorKind::WorkspaceBusy,
        "workspace catalog operation in progress",
    )
}
fn worker_failed(error: tokio::task::JoinError) -> WorkspaceError {
    err(
        WorkspaceErrorKind::InvalidRequest,
        format!(
            "catalog worker failed: {error}; storage outcome unknown, reconcile before retrying"
        ),
    )
}
fn workspace_id(name: &str) -> Result<ResourceID, WorkspaceError> {
    workspace_resource_id(name)
}
fn member_id(workspace: &ResourceID, name: &str) -> Result<ResourceID, WorkspaceError> {
    if name.is_empty()
        || name.len() > 128
        || matches!(name, "." | "..")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(err(
            WorkspaceErrorKind::InvalidDefinition,
            "invalid member name",
        ));
    }
    ResourceID::new("agent", workspace.name(), name, None)
        .map_err(|error| err(WorkspaceErrorKind::InvalidDefinition, error.to_string()))
}
fn home_root(spec: Option<PathBuf>) -> Option<PathBuf> {
    spec.or_else(|| std::env::var_os("HOME").map(Into::into))
        .filter(|path| path.is_absolute())
}

/// Holds the exclusive lease of a catalog operation.
///
/// The lease is keyed by the workspace address and also by the logical workspace name so a
/// rename to a free name locks a concurrent create or start of that name. `name` is the name
/// the operation will publish.
pub(crate) fn acquire(
    world: &mut World,
    key: &ResourceID,
    name: Option<String>,
    id: &str,
) -> Result<(), WorkspaceError> {
    if !world.contains_resource::<Operations>() {
        return Ok(());
    }
    if id.is_empty() {
        return Err(err(WorkspaceErrorKind::InvalidRequest, "empty request ID"));
    }
    let ops = world.get_resource_mut::<Operations>().unwrap();
    if ops.refreshing
        || ops.leases.contains_key(key)
        || ops.leases.values().any(|(_, owner)| owner == id)
        || (name.is_some() && ops.leases.values().any(|(held, _)| *held == name))
    {
        return Err(busy());
    }
    ops.leases.insert(key.clone(), (name, id.to_owned()));
    if let Ok(entity) = world.workspace_by_id(key) {
        world
            .get_component_mut::<Workspace>(entity)
            .unwrap()
            .set_mutation_request(Some(id.to_owned()));
    }
    Ok(())
}

pub(crate) fn release(world: &mut World, id: &str) {
    for entity in world.workspace_list() {
        world
            .get_component_mut::<Workspace>(entity)
            .unwrap()
            .release_mutation_request(id);
    }
    if let Some(ops) = world.get_resource_mut::<Operations>() {
        ops.leases.retain(|_, (_, owner)| owner != id);
        ops.pending.remove(id);
        ops.runtime_starts.retain(|_, owner| owner != id);
        ops.member_runs.retain(|_, owner| owner != id);
    }
}

fn annotate_runtime_conflict(world: &World, entry: &mut WorkspaceCatalogEntry) {
    if let Ok(entity) = world.workspace_by_id(&entry.id) {
        if entry.definition.as_ref()
            != Some(
                world
                    .get_component::<Workspace>(entity)
                    .unwrap()
                    .definition(),
            )
        {
            entry.error = Some("workspace definition changed on disk while running; explicitly stop before adopting changes".into());
        }
    }
}
fn sort(entries: &mut [WorkspaceCatalogEntry]) {
    entries.sort_by(|a, b| a.id.to_string().cmp(&b.id.to_string()));
}
fn snapshot(world: &mut World, entry: WorkspaceCatalogEntry) {
    let catalog = world.get_resource_mut::<WorkspaceCatalog>().unwrap();
    catalog.0.retain(|item| item.id != entry.id);
    catalog.0.push(entry);
    sort(&mut catalog.0);
}
fn replace_snapshot(world: &mut World, from: &ResourceID, to: WorkspaceCatalogEntry) {
    let catalog = world.get_resource_mut::<WorkspaceCatalog>().unwrap();
    catalog
        .0
        .retain(|item| &item.id != from && item.id != to.id);
    catalog.0.push(to);
    sort(&mut catalog.0);
}
fn remove_snapshot(world: &mut World, id: &ResourceID) {
    world
        .get_resource_mut::<WorkspaceCatalog>()
        .unwrap()
        .0
        .retain(|item| &item.id != id);
}
fn catalog_has(world: &World, id: &ResourceID) -> bool {
    world
        .get_resource::<WorkspaceCatalog>()
        .is_some_and(|catalog| catalog.0.iter().any(|entry| &entry.id == id))
}
fn live_ids(world: &World) -> Vec<ResourceID> {
    world
        .workspace_list()
        .into_iter()
        .filter_map(|entity| {
            world
                .get_component::<Workspace>(entity)
                .map(|workspace| workspace.definition().id.clone())
        })
        .collect()
}
fn require_stopped(
    world: &World,
    workspace: &ResourceID,
    action: &str,
) -> Result<(), WorkspaceError> {
    if live_ids(world).iter().any(|live| live == workspace) {
        return Err(err(
            WorkspaceErrorKind::WorkspaceBusy,
            format!("workspace is running; stop it before {action}"),
        ));
    }
    Ok(())
}
fn store(world: &World) -> Result<WorkspaceCatalogHandle, WorkspaceError> {
    world
        .get_resource::<WorkspaceCatalogHandle>()
        .cloned()
        .ok_or_else(|| {
            err(
                WorkspaceErrorKind::InvalidRequest,
                "workspace catalog storage is not installed",
            )
        })
}

/// Rejects a driver missing from the published [`DriverCatalog`].
fn require_driver(world: &World, driver: &ResourceID) -> Result<(), WorkspaceError> {
    if driver.resource_type() != "driver" {
        return Err(err(
            WorkspaceErrorKind::InvalidDefinition,
            "member driver must be a driver address",
        ));
    }
    match world.get_resource::<DriverCatalog>() {
        Some(catalog) if !catalog.contains(driver) => Err(err(
            WorkspaceErrorKind::InvalidDefinition,
            format!("driver {driver} is not available"),
        )),
        _ => Ok(()),
    }
}

/// Publishes a failure on the result event matching the action.
fn reject(world: &mut World, id: String, action: &Action, error: WorkspaceError) {
    match action {
        Action::Create => world.send_event(CreateWorkspaceResult {
            id,
            result: Err(error),
        }),
        Action::Rename { .. } => world.send_event(RenameWorkspaceResult {
            id,
            result: Err(error),
        }),
        Action::Delete => world.send_event(DeleteWorkspaceResult {
            id,
            result: Err(error),
        }),
        Action::RenameMember { .. } => world.send_event(RenameWorkspaceMemberResult {
            id,
            result: Err(error),
        }),
        Action::Start => world.send_event(StartRegisteredWorkspaceResult {
            id,
            result: Err(error),
        }),
        Action::Add(_) => world.send_event(AddWorkspaceMemberResult {
            id,
            result: Err(error),
        }),
        Action::Remove(_) => world.send_event(RemoveWorkspaceMemberResult {
            id,
            result: Err(error),
        }),
    }
}
fn complete(world: &mut World, id: String, result: Result<WorkspaceCatalogEntry, WorkspaceError>) {
    let op = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned();
    release(world, &id);
    let Some(op) = op else { return };
    let invalid = |what: &str| {
        err(
            WorkspaceErrorKind::InvalidRequest,
            format!("invalid {what} completion"),
        )
    };
    match op.action {
        Action::Create => world.send_event(CreateWorkspaceResult { id, result }),
        Action::Rename { .. } => world.send_event(RenameWorkspaceResult { id, result }),
        Action::RenameMember { .. } => world.send_event(RenameWorkspaceMemberResult { id, result }),
        Action::Add(_) => world.send_event(AddWorkspaceMemberResult { id, result }),
        Action::Remove(_) => world.send_event(RemoveWorkspaceMemberResult { id, result }),
        Action::Delete => world.send_event(DeleteWorkspaceResult {
            id,
            result: Err(result.err().unwrap_or_else(|| invalid("delete"))),
        }),
        Action::Start => world.send_event(StartRegisteredWorkspaceResult {
            id,
            result: Err(result.err().unwrap_or_else(|| invalid("start"))),
        }),
    }
}

/// Registers an operation, rejects it through its result event, or loads its definition.
fn begin(world: &mut World, id: String, workspace: ResourceID, revision: String, action: Action) {
    let precheck = (|| {
        let handle = store(world)?;
        if revision.is_empty() {
            return Err(err(
                WorkspaceErrorKind::InvalidRequest,
                "explicit revision required",
            ));
        }
        let ops = world.get_resource::<Operations>().unwrap();
        if ops.pending.contains_key(&id) || ops.leases.values().any(|(_, owner)| owner == &id) {
            return Err(busy());
        }
        if let Ok(entity) = world.workspace_by_id(&workspace) {
            let runtime = world.get_component::<Workspace>(entity).unwrap();
            if !matches!(runtime.state(), WorkspaceState::Running)
                || runtime.has_pending_creations()
            {
                return Err(busy());
            }
        }
        let lease_name = match &action {
            Action::Rename { rename, .. } => {
                let target = workspace_id(&rename.new_name)?;
                if ops.leases.contains_key(&target) || world.workspace_by_id(&target).is_ok() {
                    return Err(busy());
                }
                rename.new_name.clone()
            }
            _ => workspace.name().to_owned(),
        };
        acquire(world, &workspace, Some(lease_name), &id)?;
        Ok(handle)
    })();
    let handle = match precheck {
        Ok(handle) => handle,
        Err(error) => return reject(world, id, &action, error),
    };
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .pending
        .insert(
            id.clone(),
            Operation {
                workspace: workspace.clone(),
                action,
                entry: None,
                stage: Stage::Loading,
                stopped: false,
                restart: false,
            },
        );
    let sender = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::task::spawn_blocking(move || handle.0.load(&workspace, &revision))
            .await
            .unwrap_or_else(|error| Err(worker_failed(error)));
        sender.send_event(StorageOutput::Loaded(id, result));
    });
}

fn set_stage(world: &mut World, id: &str, stage: Stage) {
    if let Some(pending) = world
        .get_resource_mut::<Operations>()
        .unwrap()
        .pending
        .get_mut(id)
    {
        pending.stage = stage;
    }
}
fn save(world: &mut World, id: String, definition: WorkspaceDefinition, revision: String) {
    save_stage(world, id, definition, revision, Stage::Saving);
}
fn save_stage(
    world: &mut World,
    id: String,
    definition: WorkspaceDefinition,
    revision: String,
    stage: Stage,
) {
    let handle = match store(world) {
        Ok(handle) => handle,
        Err(error) => {
            complete(world, id, Err(error));
            return;
        }
    };
    if !world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .contains_key(&id)
    {
        return;
    }
    set_stage(world, &id, stage);
    let sender = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::task::spawn_blocking(move || handle.0.save(&definition, &revision))
            .await
            .unwrap_or_else(|error| Err(worker_failed(error)));
        sender.send_event(StorageOutput::Saved(id, result));
    });
}

/// Creates an empty workspace. The definition is saved create-only and never started.
pub(crate) fn create(world: &mut World, request: CreateWorkspace) {
    let id = request.id.clone();
    let precheck = (|| {
        let handle = store(world)?;
        if id.is_empty() {
            return Err(err(WorkspaceErrorKind::InvalidRequest, "empty request ID"));
        }
        let workspace = workspace_id(&request.name)?;
        if !request.project_root.is_absolute() {
            return Err(err(
                WorkspaceErrorKind::InvalidDefinition,
                "project root must be an absolute path",
            ));
        }
        if live_ids(world).iter().any(|live| live == &workspace) || catalog_has(world, &workspace) {
            return Err(err(
                WorkspaceErrorKind::DuplicateWorkspace,
                format!("workspace {} already exists", request.name),
            ));
        }
        let ops = world.get_resource::<Operations>().unwrap();
        if ops.pending.contains_key(&id) || ops.leases.values().any(|(_, owner)| owner == &id) {
            return Err(busy());
        }
        acquire(world, &workspace, Some(request.name.clone()), &id)?;
        Ok(handle)
    })();
    let handle = match precheck {
        Ok(handle) => handle,
        Err(error) => {
            world.send_event(CreateWorkspaceResult {
                id,
                result: Err(error),
            });
            return;
        }
    };
    let workspace = workspace_id(&request.name).expect("name validated by precheck");
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .pending
        .insert(
            id.clone(),
            Operation {
                workspace,
                action: Action::Create,
                entry: None,
                stage: Stage::Saving,
                stopped: false,
                restart: false,
            },
        );
    let sender = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::task::spawn_blocking(move || {
            let definition = WorkspaceDefinition {
                id: workspace_id(&request.name)?,
                name: request.name,
                project_root: request.project_root,
                agents: Vec::new(),
            };
            // The empty revision makes the store create-only: an existing document is
            // reported as a conflict instead of being overwritten.
            handle.0.save(&definition, "")
        })
        .await
        .unwrap_or_else(|error| Err(worker_failed(error)));
        sender.send_event(StorageOutput::Saved(id, result));
    });
}

/// Renames a stopped workspace: directory move plus workspace and member identities.
pub(crate) fn rename_workspace(world: &mut World, request: RenameWorkspace) {
    let id = request.id.clone();
    let bad = |world: &mut World, error| {
        world.send_event(RenameWorkspaceResult {
            id: id.clone(),
            result: Err(error),
        })
    };
    if request.workspace_id.resource_type() != "workspace" {
        return bad(
            world,
            err(
                WorkspaceErrorKind::InvalidRequest,
                "expected workspace address",
            ),
        );
    }
    let new_id = match workspace_id(&request.new_name) {
        Ok(new_id) => new_id,
        Err(error) => return bad(world, error),
    };
    if new_id == request.workspace_id {
        return bad(
            world,
            err(
                WorkspaceErrorKind::InvalidRequest,
                "workspace already has this name",
            ),
        );
    }
    if let Err(error) = require_stopped(world, &request.workspace_id, "renaming") {
        return bad(world, error);
    }
    if catalog_has(world, &new_id) {
        return bad(
            world,
            err(
                WorkspaceErrorKind::DuplicateWorkspace,
                format!("workspace {} already exists", request.new_name),
            ),
        );
    }
    let snapshot = world
        .get_resource::<WorkspaceCatalog>()
        .unwrap()
        .0
        .iter()
        .find(|entry| entry.id == request.workspace_id)
        .and_then(|entry| entry.definition.clone());
    let Some(snapshot) = snapshot else {
        return bad(
            world,
            err(
                WorkspaceErrorKind::InvalidDefinition,
                "workspace definition unavailable; refresh the catalog before renaming",
            ),
        );
    };
    begin(
        world,
        id,
        request.workspace_id.clone(),
        request.expected_revision.clone(),
        Action::Rename {
            rename: request,
            snapshot,
        },
    );
}

/// Deletes a stopped workspace by moving its whole directory to recoverable storage.
pub(crate) fn delete_workspace(world: &mut World, request: DeleteWorkspace) {
    let id = request.id.clone();
    let bad = |world: &mut World, error| {
        world.send_event(DeleteWorkspaceResult {
            id: id.clone(),
            result: Err(error),
        })
    };
    if request.workspace_id.resource_type() != "workspace" {
        return bad(
            world,
            err(
                WorkspaceErrorKind::InvalidRequest,
                "expected workspace address",
            ),
        );
    }
    if let Err(error) = require_stopped(world, &request.workspace_id, "deleting") {
        return bad(world, error);
    }
    begin(
        world,
        id,
        request.workspace_id.clone(),
        request.expected_revision,
        Action::Delete,
    );
}

/// Renames a member without moving its database directory.
pub(crate) fn rename_member(world: &mut World, request: RenameWorkspaceMember) {
    let id = request.id.clone();
    let member = request.member_id;
    let name = request.new_name;
    begin(
        world,
        id,
        request.workspace_id.clone(),
        request.expected_revision,
        Action::RenameMember { member, name },
    );
}

fn loaded(world: &mut World, id: String, result: Result<WorkspaceCatalogEntry, WorkspaceError>) {
    let mut entry = match result {
        Ok(entry) => entry,
        Err(error) => {
            complete(world, id, Err(error));
            return;
        }
    };
    annotate_runtime_conflict(world, &mut entry);
    let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    else {
        return;
    };
    let action = op.action.clone();
    // Create never loads, so any loaded action publishes its refreshed entry.
    snapshot(world, entry.clone());
    if let Some(pending) = world
        .get_resource_mut::<Operations>()
        .unwrap()
        .pending
        .get_mut(&id)
    {
        pending.entry = Some(entry.clone());
    }
    if entry.error.is_some() && world.workspace_by_id(&op.workspace).is_ok() {
        return complete(
            world,
            id,
            Err(err(
                WorkspaceErrorKind::WorkspaceBusy,
                entry.error.clone().unwrap(),
            )),
        );
    }
    let definition = entry.definition.clone();
    match action {
        Action::Create => {}
        Action::Start => {
            let Some(definition) = definition else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        entry
                            .error
                            .clone()
                            .unwrap_or_else(|| "workspace definition unavailable".into()),
                    )),
                );
            };
            set_stage(world, &id, Stage::Starting);
            let runtime_id = crate::handler::request_id();
            world
                .get_resource_mut::<Operations>()
                .unwrap()
                .runtime_starts
                .insert(runtime_id.clone(), id.clone());
            crate::handler::handle_registered_start(
                world,
                StartWorkspace {
                    id: runtime_id,
                    definition,
                },
            );
            if let Ok(entity) = world.workspace_by_id(&op.workspace) {
                world
                    .get_component_mut::<Workspace>(entity)
                    .unwrap()
                    .set_mutation_request(Some(id));
            }
        }
        Action::Rename { rename, snapshot } => {
            let Some(disk) = definition else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        entry
                            .error
                            .clone()
                            .unwrap_or_else(|| "workspace definition unavailable".into()),
                    )),
                );
            };
            if disk != snapshot {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::WorkspaceBusy,
                        "workspace definition changed on disk; refresh the catalog before renaming",
                    )),
                );
            }
            let Some(handle) = store(world).ok() else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidRequest,
                        "workspace catalog storage is not installed",
                    )),
                );
            };
            let Some(pending) = world
                .get_resource_mut::<Operations>()
                .unwrap()
                .pending
                .get_mut(&id)
            else {
                return;
            };
            pending.stage = Stage::Saving;
            let sender = world.event_sender();
            let revision = entry.revision.clone();
            let source = entry.id.clone();
            let new_name = rename.new_name;
            world.spawn_async_service(async move {
                let result = tokio::task::spawn_blocking(move || {
                    handle
                        .0
                        .rename_workspace(&source, &revision, &new_name, &disk)
                })
                .await
                .unwrap_or_else(|error| Err(worker_failed(error)));
                sender.send_event(StorageOutput::Renamed(id, result));
            });
        }
        Action::Delete => {
            let Some(handle) = store(world).ok() else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidRequest,
                        "workspace catalog storage is not installed",
                    )),
                );
            };
            let sender = world.event_sender();
            let revision = entry.revision.clone();
            let source = entry.id.clone();
            world.spawn_async_service(async move {
                let result = tokio::task::spawn_blocking(move || {
                    handle.0.delete_workspace(&source, &revision)
                })
                .await
                .unwrap_or_else(|error| Err(worker_failed(error)));
                sender.send_event(StorageOutput::Deleted(id, result));
            });
        }
        Action::Add(spec) => {
            let Some(mut definition) = definition else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "workspace definition unavailable",
                    )),
                );
            };
            if let Err(error) = require_driver(world, &spec.driver) {
                return complete(world, id, Err(error));
            }
            if member_id(&definition.id, &spec.name).is_err()
                || definition.agents.iter().any(|m| m.name == spec.name)
            {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "invalid or duplicate member name/driver",
                    )),
                );
            }
            let Some(home_root) = home_root(spec.home_root) else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "absolute member home root required",
                    )),
                );
            };
            let Some(parent) = entry.path.parent() else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "workspace document has no parent",
                    )),
                );
            };
            definition.agents.push(WorkspaceAgentDefinition {
                id: member_id(&definition.id, &spec.name).expect("member name validated"),
                name: spec.name.clone(),
                driver: spec.driver,
                home_root,
                history_path: parent.join("agents").join(&spec.name).join("state.sqlite"),
            });
            save(world, id, definition, entry.revision);
        }
        Action::RenameMember { member, name } => {
            let Some(previous) = definition else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "workspace definition unavailable",
                    )),
                );
            };
            let Some(target) = previous.agents.iter().find(|m| m.id == member).cloned() else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidRequest,
                        "member is not registered",
                    )),
                );
            };
            if target.name == name {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidRequest,
                        "member already has this name",
                    )),
                );
            }
            if member_id(&previous.id, &name).is_err()
                || previous.agents.iter().any(|m| m.name == name)
            {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "invalid or duplicate member name",
                    )),
                );
            }
            let running = world
                .workspace_by_id(&op.workspace)
                .ok()
                .and_then(|entity| {
                    world
                        .get_component::<Workspace>(entity)
                        .unwrap()
                        .iter()
                        .find(|(key, _)| **key == target.id)
                        .map(|(_, agent)| (entity, agent))
                });
            let Some((entity, agent)) = running else {
                // Nothing to stop: the database directory is untouched and only the
                // definition identity changes.
                return persist_member_rename(world, id, entry, member, name);
            };
            world
                .get_resource_mut::<Operations>()
                .unwrap()
                .pending
                .get_mut(&id)
                .unwrap()
                .restart = true;
            stop_member(world, &id, entity, agent);
        }
        Action::Remove(member) => {
            let Some(definition) = definition else {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidDefinition,
                        "workspace definition unavailable",
                    )),
                );
            };
            if !definition.agents.iter().any(|m| m.id == member) {
                return complete(
                    world,
                    id,
                    Err(err(
                        WorkspaceErrorKind::InvalidRequest,
                        "member is not registered",
                    )),
                );
            }
            if let Ok(entity) = world.workspace_by_id(&op.workspace) {
                let agent = world
                    .get_component::<Workspace>(entity)
                    .unwrap()
                    .iter()
                    .find(|(key, _)| *key == &member)
                    .map(|(_, entity)| entity);
                if let Some(agent) = agent {
                    stop_member(world, &id, entity, agent);
                    return;
                }
            }
            let mut next = definition;
            next.agents.retain(|m| m.id != member);
            save_stage(world, id, next, entry.revision, Stage::SavingRemove);
        }
    }
}

pub(crate) fn system(world: &mut World) {
    for event in world
        .event_reader::<QueryWorkspaceCatalog>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        world.send_event(QueryWorkspaceCatalogResult {
            id: event.id,
            result: Ok(world.get_resource::<WorkspaceCatalog>().unwrap().0.clone()),
        });
    }
    for event in world
        .event_reader::<RefreshWorkspaceCatalog>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        let available = {
            let ops = world.get_resource::<Operations>().unwrap();
            !ops.refreshing && ops.leases.is_empty()
        };
        if !available
            || world.workspace_list().iter().any(|entity| {
                world.get_component::<Workspace>(*entity).is_some_and(|w| {
                    !matches!(w.state(), WorkspaceState::Running) || w.has_pending_creations()
                })
            })
        {
            world.send_event(RefreshWorkspaceCatalogResult {
                id: event.id,
                result: Err(busy()),
            });
            continue;
        }
        let handle = match store(world) {
            Ok(handle) => handle,
            Err(error) => {
                world.send_event(RefreshWorkspaceCatalogResult {
                    id: event.id,
                    result: Err(error),
                });
                continue;
            }
        };
        world.get_resource_mut::<Operations>().unwrap().refreshing = true;
        for entity in world.workspace_list() {
            world
                .get_component_mut::<Workspace>(entity)
                .unwrap()
                .set_mutation_request(Some("catalog-refresh".into()));
        }
        let sender = world.event_sender();
        world.spawn_async_service(async move {
            let result = tokio::task::spawn_blocking(move || handle.0.list())
                .await
                .unwrap_or_else(|error| Err(worker_failed(error)));
            sender.send_event(StorageOutput::Refresh(event.id, result));
        });
    }
    for event in world
        .event_reader::<CreateWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        create(world, event);
    }
    for event in world
        .event_reader::<RenameWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        rename_workspace(world, event);
    }
    for event in world
        .event_reader::<DeleteWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        delete_workspace(world, event);
    }
    for event in world
        .event_reader::<RenameWorkspaceMember>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        rename_member(world, event);
    }
    for event in world
        .event_reader::<StartRegisteredWorkspace>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        begin(
            world,
            event.id,
            event.workspace_id,
            event.expected_revision,
            Action::Start,
        );
    }
    for event in world
        .event_reader::<AddWorkspaceMember>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        begin(
            world,
            event.id,
            event.workspace_id,
            event.expected_revision,
            Action::Add(event.member),
        );
    }
    for event in world
        .event_reader::<RemoveWorkspaceMember>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        begin(
            world,
            event.id,
            event.workspace_id,
            event.expected_revision,
            Action::Remove(event.member_id),
        );
    }
    for output in world
        .event_reader::<StorageOutput>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        match output {
            StorageOutput::Refresh(id, mut result) => {
                world.get_resource_mut::<Operations>().unwrap().refreshing = false;
                for entity in world.workspace_list() {
                    world
                        .get_component_mut::<Workspace>(entity)
                        .unwrap()
                        .release_mutation_request("catalog-refresh");
                }
                if let Ok(entries) = &mut result {
                    let previous = world.get_resource::<WorkspaceCatalog>().unwrap().0.clone();
                    for mut entry in previous {
                        if !entries.iter().any(|item| item.id == entry.id)
                            && world.workspace_by_id(&entry.id).is_ok()
                        {
                            entry.error = Some("workspace home is missing from catalog storage while runtime is alive; last confirmed definition retained".into());
                            entries.push(entry);
                        }
                    }
                    for entry in entries.iter_mut() {
                        annotate_runtime_conflict(world, entry);
                    }
                    sort(entries);
                    world.get_resource_mut::<WorkspaceCatalog>().unwrap().0 = entries.clone();
                }
                world.send_event(RefreshWorkspaceCatalogResult { id, result });
            }
            StorageOutput::Loaded(id, result) => loaded(world, id, result),
            StorageOutput::Saved(id, result) => saved(world, id, result),
            StorageOutput::Renamed(id, result) => renamed(world, id, result),
            StorageOutput::Deleted(id, result) => deleted(world, id, result),
        }
    }
    for event in world
        .event_reader::<StartWorkspaceResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        let registered = world
            .get_resource_mut::<Operations>()
            .unwrap()
            .runtime_starts
            .remove(&event.id);
        if let Some(id) = registered {
            // The inner start owns the lease until its result; a member runtime start shares
            // the lifecycle request ID, so only a completed member start releases it.
            if !world
                .get_resource::<Operations>()
                .unwrap()
                .member_runs
                .contains_key(&event.id)
            {
                release(world, &id);
            }
            world.send_event(StartRegisteredWorkspaceResult {
                id,
                result: event.result,
            });
        }
    }
    for event in world
        .event_reader::<LoadAgentDriverResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        driver_loaded(world, event);
    }
    for event in world
        .event_reader::<StartAgentResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        member_started(world, event);
    }
    for event in world
        .event_reader::<StopAgentResult>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>()
    {
        member_stopped(world, event);
    }
}

fn stop_member(world: &mut World, id: &str, entity: Entity, agent: Entity) {
    let request = crate::handler::request_id();
    set_stage(
        world,
        id,
        Stage::StoppingMember {
            entity,
            agent,
            request: request.clone(),
        },
    );
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .member_runs
        .insert(request.clone(), id.to_owned());
    world.send_event(StopAgent { id: request, agent });
}

fn persist_member_rename(
    world: &mut World,
    id: String,
    entry: WorkspaceCatalogEntry,
    member: ResourceID,
    name: String,
) {
    let handle = match store(world) {
        Ok(handle) => handle,
        Err(error) => return complete(world, id, Err(error)),
    };
    set_stage(world, &id, Stage::SavingRename);
    let sender = world.event_sender();
    world.spawn_async_service(async move {
        let result = tokio::task::spawn_blocking(move || {
            handle
                .0
                .rename_member(&entry.id, &entry.revision, &member, &name)
        })
        .await
        .unwrap_or_else(|error| Err(worker_failed(error)));
        sender.send_event(StorageOutput::Saved(id, result));
    });
}

fn saved(world: &mut World, id: String, result: Result<WorkspaceCatalogEntry, WorkspaceError>) {
    let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    else {
        return;
    };
    let entry = match result {
        Ok(entry) => entry,
        Err(error) => {
            let message = if op.stopped {
                match op.action {
                    Action::RenameMember { .. } => format!("{}; member runtime was stopped; runtime retains last confirmed definition, durable identity may require recovery", error.message()),
                    _ => format!("{}; member runtime was stopped; persisted definition retained", error.message()),
                }
            } else {
                error.message().to_owned()
            };
            return complete(world, id, Err(err(error.kind(), message)));
        }
    };
    snapshot(world, entry.clone());
    if let Ok(entity) = world.workspace_by_id(&op.workspace) {
        if let Some(definition) = &entry.definition {
            world
                .get_component_mut::<Workspace>(entity)
                .unwrap()
                .replace_definition(definition.clone());
        }
        let name = match &op.action {
            Action::Add(spec) => Some(spec.name.as_str()),
            Action::RenameMember { name, .. } if op.restart => Some(name.as_str()),
            _ => None,
        };
        if let Some(name) = name {
            if matches!(
                world.get_component::<Workspace>(entity).unwrap().state(),
                WorkspaceState::Running
            ) {
                if let Some(member) = entry
                    .definition
                    .as_ref()
                    .and_then(|d| d.agents.iter().find(|m| m.name == name))
                    .cloned()
                {
                    let request = crate::handler::request_id();
                    let pending = world
                        .get_resource_mut::<Operations>()
                        .unwrap()
                        .pending
                        .get_mut(&id)
                        .unwrap();
                    pending.entry = Some(entry);
                    pending.stage = Stage::AddingMember {
                        entity,
                        request: request.clone(),
                    };
                    world
                        .get_resource_mut::<Operations>()
                        .unwrap()
                        .member_runs
                        .insert(request.clone(), id);
                    world.send_event(LoadAgentDriver {
                        id: request,
                        target: entity,
                        resource_id: member.driver,
                    });
                    return;
                }
            }
        }
    }
    complete(world, id, Ok(entry));
}
fn driver_loaded(world: &mut World, event: LoadAgentDriverResult) {
    let Some(id) = world
        .get_resource::<Operations>()
        .unwrap()
        .member_runs
        .get(&event.id)
        .cloned()
    else {
        return;
    };
    let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    else {
        return;
    };
    let Stage::AddingMember { entity, request } = &op.stage else {
        return;
    };
    if request != &event.id || *entity != event.target {
        return;
    }
    let entity = *entity;
    let name = match &op.action {
        Action::Add(spec) => &spec.name,
        Action::RenameMember { name, .. } => name,
        _ => return,
    };
    let entry = op.entry.unwrap();
    let Some(member) = entry
        .definition
        .as_ref()
        .and_then(|d| d.agents.iter().find(|m| &m.name == name))
        .cloned()
    else {
        return;
    };
    if member.driver != event.resource_id {
        return;
    }
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .member_runs
        .remove(&event.id);
    let Some(runtime) = world.get_component::<Workspace>(entity) else {
        return complete(world, id, Ok(entry));
    };
    if !matches!(runtime.state(), WorkspaceState::Running) {
        return complete(world, id, Ok(entry));
    }
    let project_root = runtime.definition().project_root.clone();
    let failure =
        match event.result {
            Err(error) => Some(err(
                WorkspaceErrorKind::AgentDriverLoadFailed,
                error.to_string(),
            )),
            Ok(())
                if !world
                    .get_component::<resource_id_plugin::ResourceSet<
                        margatroid_types::resource_components::AgentDriver,
                    >>(entity)
                    .is_some_and(|drivers| drivers.get(&member.driver).is_some()) =>
            {
                Some(err(
                    WorkspaceErrorKind::ResourceResolutionFailed,
                    "driver success has no entry on target workspace",
                ))
            }
            Ok(()) => None,
        };
    if let Some(error) = failure {
        crate::handler::record_member_failure(world, entity, member.id, member.driver, error);
        return complete(world, id, Ok(entry));
    }
    // The accepted lifecycle lease authorizes exactly this persisted member's start.
    let request = StartAgent {
        id: id.clone(),
        workspace: entity,
        resource_id: member.id.clone(),
        driver: member.driver.clone(),
        definition: AgentStartDefinition {
            project_root,
            home_root: member.home_root,
            history_path: member.history_path,
        },
    };
    set_stage(
        world,
        &id,
        Stage::StartingMember {
            entity,
            request: id.clone(),
        },
    );
    if let Err(error) = world.workspace_accept_agent_start(entity, request) {
        crate::handler::record_member_failure(world, entity, member.id, member.driver, error);
        complete(world, id, Ok(entry));
    }
}
fn member_started(world: &mut World, event: StartAgentResult) {
    let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&event.id)
        .cloned()
    else {
        return;
    };
    if let Stage::StartingMember { entity, request } = &op.stage {
        if request != &event.id {
            return;
        }
        // Consume the normal start result before releasing the full mutation lease. This is
        // also safe when the ordinary lifecycle system has already consumed it.
        let matches = world
            .get_component::<Workspace>(*entity)
            .is_some_and(|w| w.has_pending_creation(&event.id));
        if matches {
            crate::handler::handle_start_agent_result(world, event.clone());
        }
        complete(world, event.id, Ok(op.entry.unwrap()));
    }
}
fn member_stopped(world: &mut World, event: StopAgentResult) {
    let Some(id) = world
        .get_resource::<Operations>()
        .unwrap()
        .member_runs
        .get(&event.id)
        .cloned()
    else {
        return;
    };
    let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    else {
        return;
    };
    let Stage::StoppingMember {
        entity,
        agent,
        request,
    } = op.stage
    else {
        return;
    };
    if request != event.id {
        return;
    }
    if let Ok(actual) = &event.result {
        if *actual != agent {
            return;
        }
    }
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .member_runs
        .remove(&event.id);
    if let Err(error) = event.result {
        return complete(
            world,
            id,
            Err(err(WorkspaceErrorKind::AgentStopFailed, error.to_string())),
        );
    }
    let member = match &op.action {
        Action::Remove(member) | Action::RenameMember { member, .. } => member.clone(),
        _ => return,
    };
    if let Some(workspace) = world.get_component_mut::<Workspace>(entity) {
        if !workspace
            .iter()
            .any(|(key, actual)| key == &member && actual == agent)
        {
            return complete(
                world,
                id,
                Err(err(
                    WorkspaceErrorKind::AgentStopFailed,
                    "member stopped but runtime reference changed; reconciliation required",
                )),
            );
        }
        workspace.remove_agent(&member);
    }
    world
        .get_resource_mut::<Operations>()
        .unwrap()
        .pending
        .get_mut(&id)
        .unwrap()
        .stopped = true;
    let Some(entry) = op.entry else { return };
    if let Action::RenameMember { name, .. } = op.action {
        return persist_member_rename(world, id, entry, member, name);
    }
    let Some(mut definition) = entry.definition else {
        return;
    };
    definition.agents.retain(|m| m.id != member);
    save_stage(world, id, definition, entry.revision, Stage::SavingRemove);
}

fn renamed(world: &mut World, id: String, result: Result<WorkspaceCatalogEntry, WorkspaceError>) {
    if let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    {
        if let Ok(entry) = &result {
            replace_snapshot(world, &op.workspace, entry.clone());
        }
    }
    release(world, &id);
    world.send_event(RenameWorkspaceResult { id, result });
}
fn deleted(world: &mut World, id: String, result: Result<WorkspaceDeletion, WorkspaceError>) {
    if let Some(op) = world
        .get_resource::<Operations>()
        .unwrap()
        .pending
        .get(&id)
        .cloned()
    {
        if result.is_ok() {
            remove_snapshot(world, &op.workspace);
        }
    }
    release(world, &id);
    world.send_event(DeleteWorkspaceResult { id, result });
}
