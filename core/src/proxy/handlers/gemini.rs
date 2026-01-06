//! Gemini passthrough handler
//! Handles /v1beta/models/:model_action

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    response::IntoResponse,
};

use crate::proxy::server::AppState;

/// Handle Gemini API requests (passthrough)
pub async fn handle_gemini_request(
    State(state): State<AppState>,
    Path(model_action): Path<String>,
    _request: Request<Body>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    // Get token
    let (_access_token, _project_id, email) = state.token_manager
        .get_token("gemini", false, None)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    
    tracing::debug!("Gemini passthrough: {} using account {}", model_action, email);
    
    // For now, return not implemented
    // Full implementation would parse model_action and forward to Gemini
    Err((StatusCode::NOT_IMPLEMENTED, "Gemini passthrough not yet implemented".to_string()))
}
