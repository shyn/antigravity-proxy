//! Gemini to OpenAI response conversion

use serde_json::{json, Value};

/// Convert Gemini response to OpenAI chat completion format
pub fn convert_chat_response(gemini_response: &Value, original_model: &str) -> Value {
    let candidates = gemini_response.get("candidates").and_then(|v| v.as_array());
    
    let mut choices = Vec::new();
    
    if let Some(candidates) = candidates {
        for (i, candidate) in candidates.iter().enumerate() {
            let content = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
                .map(|parts| {
                    parts.iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            
            let finish_reason = candidate
                .get("finishReason")
                .and_then(|v| v.as_str())
                .map(|r| match r {
                    "STOP" => "stop",
                    "MAX_TOKENS" => "length",
                    "SAFETY" => "content_filter",
                    _ => "stop",
                })
                .unwrap_or("stop");
            
            choices.push(json!({
                "index": i,
                "message": {
                    "role": "assistant",
                    "content": content
                },
                "finish_reason": finish_reason
            }));
        }
    }
    
    // Parse usage if available
    let usage = gemini_response.get("usageMetadata").map(|u| {
        json!({
            "prompt_tokens": u.get("promptTokenCount").and_then(|v| v.as_i64()).unwrap_or(0),
            "completion_tokens": u.get("candidatesTokenCount").and_then(|v| v.as_i64()).unwrap_or(0),
            "total_tokens": u.get("totalTokenCount").and_then(|v| v.as_i64()).unwrap_or(0)
        })
    }).unwrap_or(json!({
        "prompt_tokens": 0,
        "completion_tokens": 0,
        "total_tokens": 0
    }));
    
    json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": original_model,
        "choices": choices,
        "usage": usage
    })
}
