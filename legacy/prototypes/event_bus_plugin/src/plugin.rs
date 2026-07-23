use core_plugin::{App, Plugin, Stage, World};

use crate::events::{EventBusPublishFailed, WorkspaceEventEmitted};
use crate::resource::EventBus;
use crate::systems::publish_workspace_events;

#[derive(Clone, Debug)]
pub struct EventBusPlugin {
    channel_capacity: usize,
}

impl EventBusPlugin {
    pub fn new() -> Self {
        Self {
            channel_capacity: EventBus::DEFAULT_CHANNEL_CAPACITY,
        }
    }

    pub fn with_channel_capacity(mut self, capacity: usize) -> Self {
        self.channel_capacity = capacity;
        self
    }
}

impl Default for EventBusPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for EventBusPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<WorkspaceEventEmitted>();
        app.add_event::<EventBusPublishFailed>();

        if app.world().resource::<EventBus>().is_none() {
            app.world_mut()
                .add_resource(EventBus::with_capacity(self.channel_capacity));
        }

        let mut reader = app.event_reader::<WorkspaceEventEmitted>();
        app.add_systems(
            Stage::Update,
            [move |world: &mut World| {
                publish_workspace_events(world, &mut reader);
            }],
        );
    }
}
