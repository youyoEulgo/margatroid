use axum::{Json, extract::State};

use crate::state::AppState;

pub async fn list(State(state): State<AppState>) -> Json<serde_json::Value> {
    let providers = state.list_providers().await;
    Json(serde_json::json!({
        "providers": providers,
        "count": providers.len(),
    }))
}
