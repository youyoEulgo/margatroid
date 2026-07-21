use core_plugin::{EventReader, World};

use crate::events::{EventBusPublishFailed, WorkspaceEventEmitted};
use crate::resource::EventBus;

pub(crate) fn publish_workspace_events(
    world: &mut World,
    reader: &mut EventReader<WorkspaceEventEmitted>,
) {
    let events = world.read_events(reader);
    for emitted in events {
        let data = match serde_json::to_string(&emitted.event) {
            Ok(data) => data,
            Err(error) => {
                world.send_event(EventBusPublishFailed {
                    channel: emitted.channel,
                    message: error.to_string(),
                });
                continue;
            }
        };

        let result = world
            .resource::<EventBus>()
            .expect("EventBus resource should be registered by EventBusPlugin")
            .publish(&emitted.channel, data);

        if let Err(error) = result {
            world.send_event(EventBusPublishFailed {
                channel: emitted.channel,
                message: error.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use core_plugin::{App, Stage, World};
    use types::events::{EventContent, EventMetadata, WorkspaceEvent};

    use crate::{EventBus, EventBusPlugin, EventBusPublishFailed, WorkspaceEventEmitted};

    fn test_event(event: &str) -> WorkspaceEvent {
        WorkspaceEvent {
            metadata: EventMetadata::new(event, "coder", "task-1"),
            content: EventContent::MemberStatus {
                state: "working".into(),
            },
        }
    }

    #[test]
    fn event_bus_register_subscribe_and_publish() {
        let bus = EventBus::with_capacity(8);
        let mut first = bus.register("demo/stream");
        let mut second = bus.subscribe("demo/stream").unwrap();

        let receiver_count = bus.publish("demo/stream", "hello".into()).unwrap();
        assert_eq!(receiver_count, 2);
        assert_eq!(first.try_recv().unwrap(), "hello");
        assert_eq!(second.try_recv().unwrap(), "hello");
    }

    #[test]
    fn event_bus_reports_missing_channel() {
        let bus = EventBus::new();
        let error = bus.publish("missing", "hello".into()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "event bus channel `missing` is not registered"
        );
    }

    #[test]
    fn plugin_publishes_workspace_events_to_registered_channel() {
        let mut app = App::new();
        app.add_plugins(EventBusPlugin::new());

        let mut receiver = app
            .world()
            .resource::<EventBus>()
            .unwrap()
            .register("demo/stream");

        app.world().send_event(WorkspaceEventEmitted::new(
            "demo/stream",
            test_event("member_status"),
        ));
        app.tick();

        let data = receiver.try_recv().unwrap();
        let value: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["metadata"]["event"], "member_status");
        assert_eq!(value["content"]["state"], "working");
    }

    #[test]
    fn plugin_emits_failure_when_channel_is_missing() {
        let mut app = App::new();
        app.add_plugins(EventBusPlugin::new());

        let failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_failures = failures.clone();
        let mut reader = app.event_reader::<EventBusPublishFailed>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                system_failures
                    .lock()
                    .unwrap()
                    .extend(world.read_events(&mut reader));
            }],
        );

        app.world().send_event(WorkspaceEventEmitted::new(
            "missing",
            test_event("member_status"),
        ));
        app.tick();

        let failures = failures.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].channel, "missing");
    }
}
