use axum::{Json, extract::State};

use crate::state::{AnyhowError, AppState};

pub async fn reload(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AnyhowError> {
    state.reload().await?;
    Ok(Json(serde_json::json!({"status": "reload"})))
}
