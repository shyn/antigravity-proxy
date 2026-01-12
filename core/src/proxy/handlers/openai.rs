//! OpenAI-compatible handler
//! Handles /v1/chat/completions, /v1/completions, /v1/models, /v1/images/generations

use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::{json, Value};

use crate::proxy::server::AppState;

const MAX_RETRY_ATTEMPTS: usize = 3;

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
    
    let token_manager = &state.token_manager;
    let pool_size = token_manager.len();
    let max_attempts = MAX_RETRY_ATTEMPTS.min(pool_size).max(1);
    
    let mut last_error = String::new();
    let mut last_status: Option<u16> = None;
    
    for attempt in 0..max_attempts {
        let force_rotate = attempt > 0;
        
        // Get token
        let session_id = None; // TODO: extract from headers
        let (access_token, project_id, email) = match token_manager
            .get_token("text", force_rotate, session_id)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                return Err((StatusCode::SERVICE_UNAVAILABLE, e.to_string()));
            }
        };
        
        tracing::info!("OpenAI request: {} -> {} (account: {}, attempt: {})", model, gemini_model, email, attempt + 1);
        
        // Build v1internal request
        let v1_request = build_v1internal_request(&body, &gemini_model, &project_id)?;
        
        // Call upstream
        let client = crate::proxy::upstream::client::UpstreamClient::new(None);
        
        let method = if stream { "streamGenerateContent" } else { "generateContent" };
        let query = if stream { Some("alt=sse") } else { None };
        
        let response = match client
            .call_v1_internal(method, &access_token, v1_request, query, false)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_error = e.clone();
                
                // Parse status code from error and mark account as rate-limited
                let status_code = if e.contains("429") { 
                    429u16 
                } else if e.contains("503") { 
                    503u16 
                } else if e.contains("500") { 
                    500u16 
                } else { 
                    502u16 
                };
                last_status = Some(status_code);
                
                // Mark account as rate limited so next get_token() returns a different account
                if status_code == 429 || status_code == 503 || status_code == 500 {
                    token_manager.mark_rate_limited(
                        &email,
                        status_code,
                        None,
                        &e,
                    );
                    tracing::info!("OpenAI: Account {} marked as rate-limited (status {})", email, status_code);
                }
                
                continue;
            }
        };
        
        // Success
        if stream {
            // TODO: Implement SSE streaming conversion
            let body_text = response.text().await.unwrap_or_default();
            return Ok((StatusCode::OK, body_text).into_response());
        } else {
            let raw_response: Value = response.json().await
                .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Invalid JSON response: {}", e)))?;
            
            // Extract response from v1internal wrapper
            let gemini_response = raw_response.get("response").unwrap_or(&raw_response);
            
            // Convert Gemini response to OpenAI format
            let openai_response = crate::proxy::mappers::gemini_to_openai::convert_chat_response(gemini_response, model);
            
            return Ok(Json(openai_response).into_response());
        }
    }
    
    // All retries failed
    let response_status = match last_status {
        Some(429) => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    };
    
    Err((response_status, last_error))
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
    
    // [FIX] Antigravity 身份注入 (matches TypeScript implementation)
    // Only inject for non-image generation models
    let is_image_model = gemini_model.contains("image");
    if !is_image_model {
        const ANTIGRAVITY_IDENTITY: &str = r#"You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.
You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.
**Absolute paths only**
**Proactiveness**"#;

        // Check if user already provided Antigravity identity
        let user_has_antigravity = system_instruction
            .as_ref()
            .and_then(|si| si.get("parts"))
            .and_then(|parts| parts.as_array())
            .map(|parts| {
                parts.iter().any(|p| {
                    p.get("text")
                        .and_then(|t| t.as_str())
                        .map(|t| t.contains("You are Antigravity"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        if !user_has_antigravity {
            if let Some(ref mut sys_inst) = system_instruction {
                // Prepend identity to existing system instruction
                if let Some(parts) = sys_inst.get_mut("parts").and_then(|p| p.as_array_mut()) {
                    parts.insert(0, json!({"text": ANTIGRAVITY_IDENTITY}));
                    tracing::debug!("[Identity] Injected Antigravity identity at beginning of existing systemInstruction");
                }
            } else {
                // Create new system instruction with identity
                system_instruction = Some(json!({
                    "parts": [{"text": ANTIGRAVITY_IDENTITY}]
                }));
                tracing::debug!("[Identity] Created new systemInstruction with Antigravity identity");
            }
        }
    }
    
    // Generate session ID (matches TypeScript: `alma-${Date.now()}-${random}`)
    let session_id = format!("alma-{}-{}", 
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
    );
    
    // Build inner request
    let mut inner_request = json!({
        "contents": contents,
        "sessionId": session_id,
        "safetySettings": [
            { "category": "HARM_CATEGORY_HARASSMENT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "OFF" },
            { "category": "HARM_CATEGORY_CIVIC_INTEGRITY", "threshold": "OFF" }
        ]
    });
    
    // Add systemInstruction with role: 'user' (required by Antigravity API)
    if let Some(mut sys_inst) = system_instruction {
        sys_inst["role"] = json!("user");
        inner_request["systemInstruction"] = sys_inst;
    }
    
    if !gen_config.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        inner_request["generationConfig"] = gen_config;
    }
    
    // Generate request ID (matches TypeScript: `alma-${crypto.randomUUID()}`)
    let request_id = format!("alma-{}", uuid::Uuid::new_v4());
    
    // Build v1internal wrapper (matches TypeScript AntigravityRequestBody)
    let v1_body = json!({
        "project": project_id,
        "model": gemini_model,
        "request": inner_request,
        "userAgent": "antigravity",
        "requestId": request_id,
        "requestType": "agent"
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
    
    let token_manager = &state.token_manager;
    let pool_size = token_manager.len();
    let max_attempts = MAX_RETRY_ATTEMPTS.min(pool_size).max(1);
    
    let mut last_error = String::new();
    let mut last_status: Option<u16> = None;
    
    for attempt in 0..max_attempts {
        let force_rotate = attempt > 0;
        
        // Get token for image generation
        let (access_token, project_id, email) = match token_manager
            .get_token("image_gen", force_rotate, None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                return Err((StatusCode::SERVICE_UNAVAILABLE, e.to_string()));
            }
        };
        
        tracing::info!("Image generation request (account: {}, attempt: {})", email, attempt + 1);
        
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
        
        let response = match client
            .call_v1_internal("generateContent", &access_token, v1_body, None, false)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_error = e.clone();
                
                let status_code = if e.contains("429") { 
                    429u16 
                } else if e.contains("503") { 
                    503u16 
                } else if e.contains("500") { 
                    500u16 
                } else { 
                    502u16 
                };
                last_status = Some(status_code);
                
                if status_code == 429 || status_code == 503 || status_code == 500 {
                    token_manager.mark_rate_limited(&email, status_code, None, &e);
                    tracing::info!("Image: Account {} marked as rate-limited (status {})", email, status_code);
                }
                
                continue;
            }
        };
        
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
        
        return Ok(Json(json!({
            "created": chrono::Utc::now().timestamp(),
            "data": images
        })));
    }
    
    // All retries failed
    let response_status = match last_status {
        Some(429) => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    };
    
    Err((response_status, last_error))
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
