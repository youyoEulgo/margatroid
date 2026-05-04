use std::convert::Infallible;

use anyhow::anyhow;
use axum::{
    Json,
    extract::State,
    response::{Sse, sse::Event},
};
use futures::StreamExt;
use serde::Deserialize;
use types::ChatRequest;

use crate::state::{AnyhowError, AppState};
#[derive(Debug, Deserialize)]
pub struct StreamApiRequest {
    pub provider: String,
    #[serde(flatten)]
    pub req: ChatRequest,
}
pub async fn stream(
    State(state): State<AppState>,
    Json(payload): Json<StreamApiRequest>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, AnyhowError> {
    let provider = state
        .get_provider(&payload.provider)
        .await
        .ok_or_else(|| anyhow!("provider '{}' not found", payload.provider))?;
    let mut req = payload.req;
    req.stream = Some(true);
    let stream = provider.chat_stream_boxed(req).await?;
    let sse_stream = stream.map(|result| {
        let event = match result {
            Ok(chunk) => serde_json::to_string(&chunk)
                .map(|json| Event::default().data(json))
                .unwrap_or_else(|e| Event::default().event("error").data(e.to_string())),
            Err(e) => Event::default().event("error").data(e.to_string()),
        };
        Ok::<_, Infallible>(event)
    });
    Ok(Sse::new(sse_stream))
}
