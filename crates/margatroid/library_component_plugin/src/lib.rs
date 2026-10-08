use core_plugin::{App, Component, Plugin, Resource};

pub trait LibraryComponent: Component {}

pub struct LibraryComponentPlugin;

pub struct LibraryComponentPluginInstalled;
impl Resource for LibraryComponentPluginInstalled {}

impl Plugin for LibraryComponentPlugin {
    fn build(self, app: &mut App) {
        if app
            .world()
            .contains_resource::<LibraryComponentPluginInstalled>()
        {
            panic!("LibraryComponentPlugin is already installed");
        }
        app.world_mut()
            .insert_resource(LibraryComponentPluginInstalled);
    }
}
