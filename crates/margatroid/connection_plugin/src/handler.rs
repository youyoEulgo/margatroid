use server_plugin::{RegisterConnection, WebSocketConnections};

pub(crate) fn handle_register_connection(
    connections: &WebSocketConnections,
    request: &RegisterConnection,
) {
    let client_type = request.client_type.trim();
    if !valid_client_type(client_type) {
        tracing::warn!(connection = request.connection_id.get(), client_type = %request.client_type, "invalid WebSocket client type");
        return;
    }
    let name = format!("{client_type}-{}", request.connection_id.get());
    if !connections.set_connection_type(request.connection_id, client_type) {
        tracing::warn!(
            connection = request.connection_id.get(),
            "WebSocket connection disappeared before registration"
        );
        return;
    }
    if let Err(error) = connections.set_name(request.connection_id, name.clone()) {
        tracing::warn!(connection = request.connection_id.get(), error = %error, "WebSocket connection could not be named");
        return;
    }
    tracing::info!(
        request_id = %request.id,
        connection = request.connection_id.get(),
        client_type,
        name,
        "WebSocket connection registered"
    );
}

fn valid_client_type(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_type_uses_stable_identifier_characters() {
        assert!(valid_client_type("webui"));
        assert!(valid_client_type("desktop-2"));
        assert!(!valid_client_type(""));
        assert!(!valid_client_type("WebUI"));
        assert!(!valid_client_type("web ui"));
    }
}
