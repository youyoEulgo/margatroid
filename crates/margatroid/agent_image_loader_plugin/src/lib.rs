mod error;
mod events;
mod handler;
mod system;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use app_runtime_plugin::{RuntimeHandle, RuntimePlugin};
use async_runtime_plugin::AppAsyncExt;
use core_plugin::{App, Component as MecsComponent, Plugin, Resource};
use margatroid_types::ResourceId;
use resource_id_plugin::ResourceIdPluginInstalled;

pub use error::{AgentImageLoadError, AgentImageLoadErrorKind};
pub use events::{LoadAgentImage, LoadAgentImageResult};
pub use types::{
    AgentImageBaseDriver, AgentImageBaseMcl, AgentImageDefaultVisibility, AgentImageDependencies,
    AgentImageDependency, AgentImageModelConfig, AgentImageModelParameters,
};

use handler::read_agent_image;
use system::{apply_agent_image_load_system, prepare_agent_image_load_system};
use types::AgentImageLoaderLimits;

#[derive(Clone, Debug)]
pub struct AgentImage {
    base_driver: AgentImageBaseDriver,
    dependencies: AgentImageDependencies,
    model: AgentImageModelConfig,
    default_visibility: AgentImageDefaultVisibility,
}

impl AgentImage {
    pub fn base_driver(&self) -> &AgentImageBaseDriver {
        &self.base_driver
    }

    pub fn dependencies(&self) -> &[AgentImageDependency] {
        self.dependencies.entries()
    }

    pub fn model(&self) -> &AgentImageModelConfig {
        &self.model
    }

    pub fn default_visibility(&self) -> impl Iterator<Item = &ResourceId> + '_ {
        self.default_visibility.resources()
    }
}

impl MecsComponent for AgentImage {}

pub struct AgentImageLoaderPlugin {
    root: PathBuf,
    schedule: String,
    limits: AgentImageLoaderLimits,
}

impl AgentImageLoaderPlugin {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AgentImageLoadError> {
        let root = handler::normalize_root(root.into())?;
        handler::ensure_root(&root)?;
        Ok(Self {
            root,
            schedule: RuntimePlugin::PRE_UPDATE.to_owned(),
            limits: AgentImageLoaderLimits::default(),
        })
    }

    pub fn with_schedule(mut self, schedule: impl Into<String>) -> Self {
        self.schedule = schedule.into();
        self
    }
}

impl Plugin for AgentImageLoaderPlugin {
    fn build(self, app: &mut App) {
        if !app.world().contains_resource::<RuntimeHandle>() {
            panic!("RuntimePlugin is not installed");
        }
        if !app
            .world()
            .contains_resource::<async_runtime_plugin::AsyncRuntimeHandle>()
        {
            panic!("AsyncRuntimePlugin is not installed");
        }
        if !app.world().contains_resource::<ResourceIdPluginInstalled>() {
            panic!("ResourceIdPlugin is not installed");
        }
        if app
            .world()
            .contains_resource::<AgentImageLoaderPluginInstalled>()
        {
            panic!("AgentImageLoaderPlugin is already installed");
        }
        if !app.contains_schedule(&self.schedule) {
            panic!("AgentImageLoaderPlugin schedule does not exist");
        }
        let schedule = self.schedule;
        app.world_mut()
            .insert_resource(AgentImageLoaderPluginInstalled);
        app.world_mut().insert_resource(AgentImageLoaderState {
            root: Arc::new(self.root),
            limits: self.limits,
            pending: HashMap::new(),
        });
        app.add_system(&schedule, prepare_agent_image_load_system)
            .add_async_system(&schedule, read_agent_image)
            .add_system(&schedule, apply_agent_image_load_system);
    }
}

pub struct AgentImageLoaderPluginInstalled;

impl Resource for AgentImageLoaderPluginInstalled {}

pub(crate) struct AgentImageLoaderState {
    pub(crate) root: Arc<PathBuf>,
    pub(crate) limits: AgentImageLoaderLimits,
    pub(crate) pending: HashMap<ResourceId, Vec<String>>,
}

impl Resource for AgentImageLoaderState {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use app_runtime_plugin::{RuntimePlugin, WorldEventExt};
    use async_runtime_plugin::AsyncRuntimePlugin;
    use core_plugin::{App, Entity};
    use margatroid_types::ResourceId;
    use resource_id_plugin::ResourceIdPlugin;

    use super::{
        AgentImage, AgentImageLoadError, AgentImageLoadErrorKind, AgentImageLoaderPlugin,
        AgentImageLoaderState, LoadAgentImage, LoadAgentImageResult,
    };

    static DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "margatroid-agent-image-loader-{label}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn image_root(library: &Path) -> PathBuf {
        library.join("local/coder/latest")
    }

    fn write_image(library: &Path, soul: &str) {
        let image = image_root(library);
        fs::create_dir_all(&image).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "skill:local/code-review:latest"

[[dependencies]]
id = "tool:local/list-directory:latest"
"#,
        )
        .unwrap();
        fs::write(image.join("SOUL.md"), soul).unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"IMPORT skill:local/code-review:latest AS review\")\nmcl_command(\"IMPORT tool:local/list-directory:latest AS list_dir\")\nmcl_command(\"INJECT review, list_dir TO tool_default FROM tool\")\nmcl_command(\"INJECT SELECT tool_default FROM tool COVER tool_dynamic FROM tool\")\n",
        )
        .unwrap();
    }

    fn app(library: &Path) -> App {
        let mut app = App::new();
        app.add_plugin(RuntimePlugin::default())
            .add_plugin(AsyncRuntimePlugin)
            .add_plugin(ResourceIdPlugin)
            .add_plugin(AgentImageLoaderPlugin::open(library).unwrap());
        app
    }

    fn load(
        app: &mut App,
        id: &str,
        reference: &ResourceId,
    ) -> Result<Entity, AgentImageLoadError> {
        app.world().send_event(LoadAgentImage {
            id: id.to_owned(),
            reference: reference.clone(),
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            app.tick();
            if let Some(event) = app
                .world()
                .event_reader::<LoadAgentImageResult>()
                .into_iter()
                .find(|event| event.id == id)
            {
                return event.result.clone();
            }
            assert!(Instant::now() < deadline, "agent image load timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn loads_an_image_into_complete_read_only_components() {
        let library = unique_directory("components");
        write_image(&library, "You are a careful coder.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();

        let entity = load(&mut app, "load-components", &reference).unwrap();

        let identity = app.world().get_component::<ResourceId>(entity).unwrap();
        let image = app.world().get_component::<AgentImage>(entity).unwrap();

        assert_eq!(identity, &reference);
        assert_eq!(image.model().model(), "deepseek-v4-flash");
        assert_eq!(image.model().parameters().temperature(), Some(0.7));
        assert_eq!(image.model().parameters().max_output_tokens(), Some(8192));
        assert_eq!(image.model().parameters().top_p(), Some(0.9));
        assert_eq!(image.model().parameters().stop(), ["DONE"]);
        assert_eq!(
            image
                .default_visibility()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "skill:local/code-review:latest",
                "tool:local/list-directory:latest"
            ]
        );
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn concurrent_requests_share_one_read_and_entity() {
        let library = unique_directory("concurrent");
        write_image(&library, "Concurrent image.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        for id in ["first", "second"] {
            app.world().send_event(LoadAgentImage {
                id: id.to_owned(),
                reference: reference.clone(),
            });
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let entities = loop {
            app.tick();
            let results = app
                .world()
                .event_reader::<LoadAgentImageResult>()
                .into_iter()
                .filter(|event| event.id == "first" || event.id == "second")
                .map(|event| event.result.as_ref().copied().unwrap())
                .collect::<Vec<_>>();
            if results.len() == 2 {
                break results;
            }
            assert!(Instant::now() < deadline, "agent image load timed out");
            std::thread::yield_now();
        };

        assert_eq!(entities[0], entities[1]);
        assert_eq!(app.world().entity_count(), 1);
        let state = app.world().get_resource::<AgentImageLoaderState>().unwrap();
        assert!(state.pending.is_empty());
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn successful_reload_reuses_the_entity_and_replaces_components() {
        let library = unique_directory("reload-success");
        write_image(&library, "Old soul.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();
        let entity = load(&mut app, "initial", &reference).unwrap();
        fs::write(image_root(&library).join("SOUL.md"), "New soul.\n").unwrap();

        let reloaded = load(&mut app, "reload", &reference).unwrap();

        assert_eq!(reloaded, entity);
        assert_eq!(app.world().entity_count(), 1);
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn failed_reload_preserves_the_previous_entity() {
        let library = unique_directory("reload-failure");
        write_image(&library, "Stable soul.\n");
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();
        let _entity = load(&mut app, "initial", &reference).unwrap();
        fs::write(image_root(&library).join("unknown.txt"), "invalid").unwrap();

        let error = load(&mut app, "broken", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::InvalidLayout);
        assert_eq!(app.world().entity_count(), 1);
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn missing_images_return_not_found_without_creating_entities() {
        let library = unique_directory("missing");
        fs::create_dir_all(&library).unwrap();
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/missing").unwrap();

        let error = load(&mut app, "missing", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::NotFound);
        assert_eq!(app.world().entity_count(), 0);
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn image_layout_allows_resource_directories_without_soul() {
        let library = unique_directory("layout-dirs");
        let image = image_root(&library);
        fs::create_dir_all(image.join("skills")).unwrap();
        fs::create_dir_all(image.join("hooks")).unwrap();
        fs::create_dir_all(image.join("tools")).unwrap();
        fs::create_dir_all(image.join("shells")).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]
"#,
        )
        .unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"EMIT EFFECT finish\")\n",
        )
        .unwrap();

        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        let entity = load(&mut app, "layout-dirs", &reference).unwrap();

        assert!(app.world().get_component::<ResourceId>(entity).is_some());
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn prompt_dependency_requires_uppercase_file_in_image_root() {
        let library = unique_directory("prompt-deps");
        let image = image_root(&library);
        fs::create_dir_all(&image).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "prompt:system/test:latest"
"#,
        )
        .unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"EMIT EFFECT finish\")\n",
        )
        .unwrap();

        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        let error = load(&mut app, "missing-test", &reference).unwrap_err();
        assert_eq!(error.kind(), AgentImageLoadErrorKind::PromptReadFailed);

        fs::write(image.join("TEST.md"), "You are a careful coder.\n").unwrap();
        let entity = load(&mut app, "with-test", &reference).unwrap();
        assert!(app.world().get_component::<ResourceId>(entity).is_some());
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn prompt_dependency_allows_same_file_for_system_and_user() {
        let library = unique_directory("prompt-dual-role");
        let image = image_root(&library);
        fs::create_dir_all(&image).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "prompt:system/test:latest"

[[dependencies]]
id = "prompt:user/test:latest"
"#,
        )
        .unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"EMIT EFFECT finish\")\n",
        )
        .unwrap();
        fs::write(image.join("TEST.md"), "Shared prompt.\n").unwrap();

        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        let entity = load(&mut app, "dual-role", &reference).unwrap();

        assert!(app.world().get_component::<ResourceId>(entity).is_some());
        let _ = fs::remove_dir_all(library);
    }

    #[test]
    fn duplicate_prompt_dependency_is_rejected() {
        let library = unique_directory("prompt-duplicate");
        let image = image_root(&library);
        fs::create_dir_all(&image).unwrap();
        fs::write(
            image.join("agent.toml"),
            r#"schema_version = 1

[inference]
model = "deepseek-v4-flash"
temperature = 0.7
max_output_tokens = 8192
top_p = 0.9
stop = ["DONE"]

[[dependencies]]
id = "prompt:system/test:latest"

[[dependencies]]
id = "prompt:system/test:v1"
"#,
        )
        .unwrap();
        fs::write(
            image.join("base.lua"),
            "mcl_command(\"EMIT EFFECT finish\")\n",
        )
        .unwrap();
        fs::write(image.join("TEST.md"), "Shared prompt.\n").unwrap();

        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder:latest").unwrap();
        let error = load(&mut app, "duplicate", &reference).unwrap_err();
        assert_eq!(error.kind(), AgentImageLoadErrorKind::DuplicateDependency);
        let _ = fs::remove_dir_all(library);
    }

    #[cfg(unix)]
    #[test]
    fn image_layout_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let library = unique_directory("symlink");
        write_image(&library, "Symlink test.\n");
        symlink(
            image_root(&library).join("SOUL.md"),
            image_root(&library).join("linked-soul"),
        )
        .unwrap();
        let mut app = app(&library);
        let reference = ResourceId::parse("image:local/coder").unwrap();

        let error = load(&mut app, "symlink", &reference).unwrap_err();

        assert_eq!(error.kind(), AgentImageLoadErrorKind::SymlinkNotAllowed);
        assert_eq!(app.world().entity_count(), 0);
        let _ = fs::remove_dir_all(library);
    }
}
