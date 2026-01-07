//! OpenAI-compatible handler
//! Handles /v1/chat/completions, /v1/completions, /v1/models, /v1/images/generations

use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::{json, Value};

use crate::proxy::server::AppState;

/// Handle POST /v1/chat/completions
pub async fn handle_chat_completions(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Extract model and check if streaming
    let model = body.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini-1.5-flash");
    
    let stream = body.get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    // Resolve model mapping
    let gemini_model = resolve_model(&state, model).await;
    
    // Get token
    let session_id = None; // TODO: extract from headers
    let (access_token, project_id, email) = state.token_manager
        .get_token("text", false, session_id)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    
    tracing::info!("OpenAI request: {} -> {} (account: {})", model, gemini_model, email);
    
    // Build v1internal request
    let v1_request = build_v1internal_request(&body, &gemini_model, &project_id)?;
    
    // Call upstream
    let client = crate::proxy::upstream::client::UpstreamClient::new(None);
    
    let method = if stream { "streamGenerateContent" } else { "generateContent" };
    let query = if stream { Some("alt=sse") } else { None };
    
    let response = client
        .call_v1_internal(method, &access_token, v1_request, query)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    
    let status = response.status();
    
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        tracing::error!("Upstream error {}: {}", status, error_text);
        return Err((StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY), error_text));
    }
    
    if stream {
        // TODO: Implement SSE streaming conversion
        let body_text = response.text().await.unwrap_or_default();
        Ok((StatusCode::OK, body_text).into_response())
    } else {
        let raw_response: Value = response.json().await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Invalid JSON response: {}", e)))?;
        
        // Extract response from v1internal wrapper
        let gemini_response = raw_response.get("response").unwrap_or(&raw_response);
        
        // Convert Gemini response to OpenAI format
        let openai_response = crate::proxy::mappers::gemini_to_openai::convert_chat_response(gemini_response, model);
        
        Ok(Json(openai_response).into_response())
    }
}

/// Build v1internal request wrapper
fn build_v1internal_request(body: &Value, gemini_model: &str, project_id: &str) -> Result<Value, (StatusCode, String)> {
    let mut contents = Vec::new();
    let mut system_instruction: Option<Value> = None;
    
    // Process messages
    if let Some(messages) = body.get("messages").and_then(|v| v.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            
            // Handle system messages separately
            if role == "system" {
                if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                    system_instruction = Some(json!({
                        "parts": [{"text": content}]
                    }));
                }
                continue;
            }
            
            let gemini_role = match role {
                "assistant" => "model",
                _ => "user",
            };
            
            let mut parts = Vec::new();
            
            if let Some(content) = msg.get("content") {
                match content {
                    Value::String(s) => {
                        parts.push(json!({"text": s}));
                    }
                    Value::Array(arr) => {
                        for item in arr {
                            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                                parts.push(json!({"text": text}));
                            }
                            // Handle image_url type
                            if let Some(image_url) = item.get("image_url") {
                                if let Some(url) = image_url.get("url").and_then(|v| v.as_str()) {
                                    if url.starts_with("data:") {
                                        // Base64 encoded image
                                        if let Some((mime, data)) = parse_data_url(url) {
                                            parts.push(json!({
                                                "inlineData": {
                                                    "mimeType": mime,
                                                    "data": data
                                                }
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            
            if !parts.is_empty() {
                contents.push(json!({
                    "role": gemini_role,
                    "parts": parts
                }));
            }
        }
    }
    
    // Ensure we have at least one message
    if contents.is_empty() {
        contents.push(json!({
            "role": "user",
            "parts": [{"text": "Hello"}]
        }));
    }
    
    // Build generation config
    let mut gen_config = json!({});
    
    if let Some(max_tokens) = body.get("max_tokens").or(body.get("max_completion_tokens")) {
        gen_config["maxOutputTokens"] = max_tokens.clone();
    }
    if let Some(temp) = body.get("temperature") {
        gen_config["temperature"] = temp.clone();
    }
    if let Some(top_p) = body.get("top_p") {
        gen_config["topP"] = top_p.clone();
    }
    if let Some(stop) = body.get("stop") {
        gen_config["stopSequences"] = stop.clone();
    }
    
    // Build inner request
    let mut inner_request = json!({
        "contents": contents,
        "safetySettings": [
            { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_CIVIC_INTEGRITY", "threshold": "OFF" }
        ]
    });
    
    if let Some(sys_inst) = system_instruction {
        inner_request["systemInstruction"] = sys_inst;
    }
    
    if !gen_config.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        inner_request["generationConfig"] = gen_config;
    }
    
    // Generate request ID
    let request_id = format!("cli-{}", uuid::Uuid::new_v4().simple());
    
    // Build v1internal wrapper
    let v1_body = json!({
        "project": project_id,
        "requestId": request_id,
        "request": inner_request,
        "model": gemini_model,
        "userAgent": "antigravity-cli",
        "requestType": "text"
    });
    
    Ok(v1_body)
}

/// Parse data URL to extract mime type and base64 data
fn parse_data_url(url: &str) -> Option<(String, String)> {
    if !url.starts_with("data:") {
        return None;
    }
    
    let rest = &url[5..];
    let parts: Vec<&str> = rest.splitn(2, ',').collect();
    if parts.len() != 2 {
        return None;
    }
    
    let meta = parts[0];
    let data = parts[1];
    
    let mime = if meta.contains(';') {
        meta.split(';').next().unwrap_or("application/octet-stream")
    } else {
        meta
    };
    
    Some((mime.to_string(), data.to_string()))
}

/// Handle POST /v1/completions (legacy)
pub async fn handle_completions(
    State(state): State<AppState>,
    Json(mut body): Json<Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Convert legacy completions format to chat format
    if let Some(prompt) = body.get("prompt").cloned() {
        let prompt_str = match prompt {
            Value::String(s) => s,
            Value::Array(arr) => arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => "".to_string(),
        };
        
        body["messages"] = json!([{"role": "user", "content": prompt_str}]);
    }
    
    handle_chat_completions(State(state), Json(body)).await
}

/// Handle GET /v1/models
pub async fn handle_list_models(
    State(_state): State<AppState>,
) -> impl IntoResponse {
    let models = vec![
        model_object("gemini-2.5-pro"),
        model_object("gemini-2.5-flash"),
        model_object("gemini-2.5-flash-lite"),
        model_object("gemini-3-flash"),
        model_object("gemini-3-pro-low"),
        model_object("gemini-3-pro-high"),
        model_object("claude-sonnet-4-5"),
        model_object("claude-opus-4-5-thinking"),
        model_object("gpt-4"),
        model_object("gpt-4o"),
        model_object("gpt-4o-mini"),
        model_object("gpt-3.5-turbo"),
    ];
    
    Json(json!({
        "object": "list",
        "data": models
    }))
}

fn model_object(id: &str) -> Value {
    json!({
        "id": id,
        "object": "model",
        "created": 1700000000,
        "owned_by": "antigravity-proxy"
    })
}

/// Handle POST /v1/images/generations
pub async fn handle_images_generations(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let prompt = body.get("prompt")
        .and_then(|v| v.as_str())
        .ok_or((StatusCode::BAD_REQUEST, "Missing prompt".to_string()))?;
    
    // Get token for image generation
    let (access_token, project_id, email) = state.token_manager
        .get_token("image_gen", false, None)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    
    tracing::info!("Image generation request (account: {})", email);
    
    // Build v1internal request for image generation
    let inner_request = json!({
        "contents": [{
            "role": "user",
            "parts": [{"text": prompt}]
        }],
        "generationConfig": {
            "imageConfig": {
                "numberOfImages": body.get("n").and_then(|v| v.as_i64()).unwrap_or(1),
                "outputMimeType": "image/png"
            }
        }
    });
    
    let request_id = format!("cli-img-{}", uuid::Uuid::new_v4().simple());
    
    let v1_body = json!({
        "project": project_id,
        "requestId": request_id,
        "request": inner_request,
        "model": "gemini-3-pro-image",
        "userAgent": "antigravity-cli",
        "requestType": "image_gen"
    });
    
    let client = crate::proxy::upstream::client::UpstreamClient::new(None);
    
    let response = client
        .call_v1_internal("generateContent", &access_token, v1_body, None)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    
    if !response.status().is_success() {
        let status_code = response.status().as_u16();
        let error_text = response.text().await.unwrap_or_default();
        let status = StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY);
        return Err((status, error_text));
    }
    
    let raw_response: Value = response.json().await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Invalid JSON: {}", e)))?;
    
    let gemini_response = raw_response.get("response").unwrap_or(&raw_response);
    
    // Extract images from response
    let images: Vec<Value> = gemini_response.get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts.iter()
                .filter_map(|part| {
                    part.get("inlineData").map(|data| {
                        json!({
                            "b64_json": data.get("data").and_then(|v| v.as_str()).unwrap_or("")
                        })
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    
    Ok(Json(json!({
        "created": chrono::Utc::now().timestamp(),
        "data": images
    })))
}

/// Resolve model mapping
async fn resolve_model(state: &AppState, model: &str) -> String {
    // Check custom mapping first
    {
        let custom = state.custom_mapping.read().await;
        if let Some(mapped) = custom.get(model) {
            return mapped.clone();
        }
    }
    
    // Check OpenAI mapping
    {
        let openai = state.openai_mapping.read().await;
        if let Some(mapped) = openai.get(model) {
            return mapped.clone();
        }
    }
    
    // Check Anthropic mapping
    {
        let anthropic = state.anthropic_mapping.read().await;
        if let Some(mapped) = anthropic.get(model) {
            return mapped.clone();
        }
    }
    
    // Default: use as-is if it looks like a Gemini model, otherwise default to flash
    if model.starts_with("gemini-") || model.starts_with("models/") || model.starts_with("claude-") {
        model.to_string()
    } else {
        "gemini-2.5-flash".to_string()
    }
}
