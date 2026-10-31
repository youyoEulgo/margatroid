use app_runtime_plugin::RuntimePlugin;
use closure_plugin::{AppClosureExt, ClosurePlugin, WorldClosureExt};
use core_plugin::{App, Resource};
struct Counter(u32);
impl Resource for Counter {}
#[test]
fn a_cloned_sender_can_cross_threads_without_borrowing_world() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(ClosurePlugin)
        .add_closure_system(RuntimePlugin::UPDATE)
        .add_closure_system(RuntimePlugin::POST_UPDATE);
    app.world_mut().insert_resource(Counter(0));
    let sender = app.world().closure_sender(RuntimePlugin::UPDATE);
    let another = sender.clone();
    std::thread::spawn(move || {
        sender.send(|world| world.get_resource_mut::<Counter>().unwrap().0 += 1);
        another.send(|world| world.get_resource_mut::<Counter>().unwrap().0 += 2);
    })
    .join()
    .unwrap();
    assert_eq!(app.world().get_resource::<Counter>().unwrap().0, 0);
    app.tick();
    assert_eq!(app.world().get_resource::<Counter>().unwrap().0, 3);
    app.tick();
    assert_eq!(app.world().get_resource::<Counter>().unwrap().0, 3);
}
