mod app;
mod async_runtime;
mod component;
mod entity;
mod events;
mod plugin;
mod query;
mod resource;
mod schedule;
mod system;
mod world;

pub use app::{App, AppControl, Stage};
pub use async_runtime::{
    AsyncSystemOptions, AsyncTaskFailed, AsyncTaskFailureKind, AsyncTaskId, AsyncTaskStarted,
};
pub use component::{Bundle, Component};
pub use entity::Entity;
pub use events::{Event, EventReader};
pub use plugin::{Plugin, PluginGroup};
pub use query::{Query, QueryMut, Res, ResMut};
pub use resource::Resource;
pub use schedule::{Schedule, ScheduleReport, SystemRunFailure};
pub use system::{named_system, System, SystemFailed, WrappedFn};
pub use world::World;

/// CorePlugin — ECS 地基。
/// 不注册任何 system，只确保 App 有完整的 World + Schedule 基础设施。
/// 其他插件（ServerPlugin、LLMPlugin 等）在此基础上构建。
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, _app: &mut App) {
        // CorePlugin 不注册任何东西——World 和 Schedule 已在 App::new() 中初始化。
    }
}
