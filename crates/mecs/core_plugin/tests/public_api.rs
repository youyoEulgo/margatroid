use core_plugin::{App, Component, Event, Plugin, Resource, Schedule, World};

struct Position(i32);
impl Component for Position {}

struct Settings(i32);
impl Resource for Settings {}

struct Updated;
impl Event for Updated {}

struct TestPlugin;

impl Plugin for TestPlugin {
    fn build(self, app: &mut App) {
        app.world_mut().insert_resource(Settings(1));
        app.register_event::<Updated>();
        app.add_schedule("update".into());
    }
}

#[test]
fn documented_public_api_composes_from_an_external_crate() {
    let mut app = App::new();
    app.add_plugin(TestPlugin);
    let entity = app.world_mut().spawn();
    assert!(app.world_mut().insert_component(entity, Position(1)));
    app.add_system("update", move |world: &mut World| {
        world.get_component_mut::<Position>(entity).unwrap().0 += 1;
        world.get_resource_mut::<Settings>().unwrap().0 += 1;
        world.event_write().send_event(Updated);
    });

    app.tick();
    app.tick();

    assert_eq!(app.world().get_component::<Position>(entity).unwrap().0, 3);
    assert_eq!(app.world().get_resource::<Settings>().unwrap().0, 3);
    assert_eq!(app.world().event_reader::<Updated>().len(), 1);
}

#[test]
fn schedule_is_independently_usable() {
    let mut schedule = Schedule::new();
    schedule.add_system(|world: &mut World| {
        world.insert_resource(Settings(7));
    });
    let mut world = World::new();

    schedule.run(&mut world);

    assert_eq!(world.get_resource::<Settings>().unwrap().0, 7);
}
