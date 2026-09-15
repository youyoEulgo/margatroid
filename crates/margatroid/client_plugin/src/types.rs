use server_plugin::WebSocketConnectionId;

pub fn client_source(
    client_type: Option<&str>,
    name: Option<&str>,
    connection_id: WebSocketConnectionId,
) -> String {
    match (client_type, name) {
        (Some(client_type), Some(name)) => {
            format!("client:{client_type}/{name}:{}", connection_id.get())
        }
        _ => format!("unregistered:{}", connection_id.get()),
    }
}
