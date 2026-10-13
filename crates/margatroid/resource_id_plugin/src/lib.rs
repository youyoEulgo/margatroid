mod components;
mod error;
mod handler;
mod types;

use core_plugin::{Plugin, Resource};

pub use components::{LibraryComponent, ResourceInner, ResourceSet, ResourcesComponent};
pub use error::{ResourceError, ResourceIDError};
pub use handler::{
    find_resource_entities, insert_library_component, insert_resources_component,
    register_library_type, register_resources_type, ResourceComponentKind,
    ResourceTypeRegistration, ResourceTypeRegistry, WorldResourceExt,
};
pub use types::ResourceID;

pub struct ResourcePlugin;

pub struct ResourcePluginInstalled;
impl Resource for ResourcePluginInstalled {}

impl Plugin for ResourcePlugin {
    fn build(self, app: &mut core_plugin::App) {
        if app.world().contains_resource::<ResourcePluginInstalled>() {
            panic!("ResourcePlugin is already installed");
        }
        app.world_mut().insert_resource(ResourcePluginInstalled);
        app.world_mut()
            .insert_resource(ResourceTypeRegistry::default());
    }
}
