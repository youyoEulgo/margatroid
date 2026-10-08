use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use core_plugin::App;
use resource_library_plugin::ResourceLibraryRoot;
use tool_plugin::ToolPlugin;

fn runtime() -> App {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin);
    app
}

#[test]
fn open_installs_the_requested_library_root() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = runtime();
    app.add_plugin(ToolPlugin::open(directory.path()).unwrap());
    assert_eq!(
        app.world().get_resource::<ResourceLibraryRoot>().unwrap().0,
        directory.path()
    );
}

#[test]
fn default_respects_an_explicitly_injected_root() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = runtime();
    app.world_mut()
        .insert_resource(ResourceLibraryRoot(directory.path().to_owned()));
    app.add_plugin(ToolPlugin::default());
    assert_eq!(
        app.world().get_resource::<ResourceLibraryRoot>().unwrap().0,
        directory.path()
    );
}

#[test]
#[should_panic(expected = "conflicts with ResourceLibraryRoot")]
fn open_does_not_silently_override_an_installed_root() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = runtime();
    app.world_mut()
        .insert_resource(ResourceLibraryRoot(directory.path().join("first")));
    app.add_plugin(ToolPlugin::open(directory.path().join("second")).unwrap());
}

#[test]
fn open_rejects_relative_and_parent_traversal_paths() {
    assert!(ToolPlugin::open("relative").is_err());
    assert!(ToolPlugin::open("/tmp/../resources").is_err());
}
