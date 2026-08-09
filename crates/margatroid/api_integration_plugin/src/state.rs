use app_runtime_plugin::WorldEventExt;
use core_plugin::World;
use dto_plugin::{WebSocketMessageSend, WebSocketMessageTarget};
use margatroid_protocol::{BackendStateDto, IntoDto, ServerMessage};

pub(crate) fn sync_frontend_state_system(world: &mut World, frontend_type: &str) {
    let state: BackendStateDto = match ().into_dto(&*world) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(error = %error, "frontend state sync failed");
            return;
        }
    };
    world.send_event(WebSocketMessageSend {
        target: WebSocketMessageTarget::Type(frontend_type.into()),
        message: ServerMessage::StateSync { state },
    });
}
