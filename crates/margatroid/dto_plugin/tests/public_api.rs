use app_runtime_plugin::RuntimePlugin;
use async_runtime_plugin::AsyncRuntimePlugin;
use config_plugin::{ConfigPlugin, MargatroidConfig, WebSocketMessageTarget};
use core_plugin::App;
use dto_plugin::{DtoPlugin, WebSocketMessageSend};
use log_plugin::LogPlugin;
use margatroid_protocol::ServerMessage;
use server_plugin::ServerPlugin;

#[test]
fn documented_public_api_publishes_state_for_the_configured_frontend_type() {
    let mut app = App::new();
    app.add_plugin(RuntimePlugin::default())
        .add_plugin(AsyncRuntimePlugin)
        .add_plugin(LogPlugin::default().without_console().with_stream(8))
        .add_plugin(ServerPlugin::bind("127.0.0.1:0"))
        .add_plugin(ConfigPlugin::new(
            MargatroidConfig::new(
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Type("browser".into())],
                vec![WebSocketMessageTarget::Broadcast],
                vec![WebSocketMessageTarget::Broadcast],
            )
            .unwrap(),
        ))
        .add_plugin(DtoPlugin::default());

    app.tick();
    app.tick();

    let messages = app
        .world()
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .collect::<Vec<_>>();
    assert!(messages.iter().any(|message| {
        message.target == WebSocketMessageTarget::Type("browser".into())
            && matches!(
                &message.message,
                ServerMessage::StateSync { state }
                    if state.workspaces.is_empty() && state.histories.is_empty()
            )
    }));

    app.tick();
    app.tick();
    assert!(app
        .world()
        .event_reader::<WebSocketMessageSend>()
        .into_iter()
        .all(|message| !matches!(message.message, ServerMessage::StateSync { .. })));
}
