//! OpenAI to Gemini request conversion

use serde_json::{json, Value};

/// Convert OpenAI chat request to Gemini format
pub fn convert_chat_request(body: &Value, gemini_model: &str) -> Value {
    let mut contents = Vec::new();
    
    // Process messages
    if let Some(messages) = body.get("messages").and_then(|v| v.as_array()) {
        let mut system_content = String::new();
        
        // First pass: collect system messages
        for msg in messages {
            if msg.get("role").and_then(|v| v.as_str()) == Some("system") {
                if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                    if !system_content.is_empty() {
                        system_content.push('\n');
                    }
                    system_content.push_str(content);
                }
            }
        }
        
        // Inject system message as first exchange if present
        if !system_content.is_empty() {
            contents.push(json!({
                "role": "user",
                "parts": [{"text": format!("[System Instructions]:\n{}", system_content)}]
            }));
            contents.push(json!({
                "role": "model",
                "parts": [{"text": "Understood. I will follow these instructions."}]
            }));
        }
        
        // Second pass: process user/assistant messages
        for msg in messages {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            if role == "system" {
                continue; // Already handled
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
    
    json!({
        "model": gemini_model,
        "contents": contents,
        "generationConfig": gen_config
    })
}

/// Parse data URL to extract mime type and base64 data
fn parse_data_url(url: &str) -> Option<(String, String)> {
    // Format: data:image/png;base64,<data>
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
