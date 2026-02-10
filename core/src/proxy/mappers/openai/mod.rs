//! OpenAI streaming response conversion (Gemini SSE → OpenAI SSE)
//!
//! Converts Gemini's streaming format to OpenAI's chat completion chunk format.

use bytes::Bytes;
use futures::Stream;
use serde_json::json;
use std::pin::Pin;

/// Usage statistics from Gemini
#[derive(Debug, Clone, Default)]
pub struct UsageStats {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// OpenAI streaming state
pub struct OpenAIStreamingState {
    pub completion_id: String,
    pub model: String,
    pub created: i64,
    pub first_chunk_sent: bool,
    pub usage: Option<UsageStats>,
}

impl OpenAIStreamingState {
    pub fn new(model: &str) -> Self {
        Self {
            completion_id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
            model: model.to_string(),
            created: chrono::Utc::now().timestamp(),
            first_chunk_sent: false,
            usage: None,
        }
    }

    /// Create an OpenAI chunk
    pub fn create_chunk(
        &self,
        delta_content: Option<&str>,
        finish_reason: Option<&str>,
        include_usage: bool,
    ) -> serde_json::Value {
        let mut delta = json!({});
        
        if let Some(content) = delta_content {
            delta["content"] = json!(content);
        }
        
        // Add role on first content chunk
        if delta_content.is_some() && !self.first_chunk_sent {
            delta["role"] = json!("assistant");
        }

        let mut chunk = json!({
            "id": self.completion_id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason
            }]
        });

        // Add usage on final chunk if available
        if include_usage {
            if let Some(ref usage) = self.usage {
                chunk["usage"] = json!({
                    "prompt_tokens": usage.prompt_tokens,
                    "completion_tokens": usage.completion_tokens,
                    "total_tokens": usage.total_tokens
                });
            }
        }

        chunk
    }

    /// Emit SSE data line
    pub fn emit(&self, data: serde_json::Value) -> Bytes {
        let sse = format!("data: {}\n\n", serde_json::to_string(&data).unwrap_or_default());
        Bytes::from(sse)
    }
}

/// Create an OpenAI-compatible SSE stream from Gemini SSE stream
pub fn create_openai_sse_stream(
    mut gemini_stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    model: String,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>> {
    use async_stream::stream;
    use bytes::BytesMut;
    use futures::StreamExt;

    Box::pin(stream! {
        let mut state = OpenAIStreamingState::new(&model);
        let mut buffer = BytesMut::new();

        while let Some(chunk_result) = gemini_stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    buffer.extend_from_slice(&chunk);

                    // Process complete lines
                    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line_raw = buffer.split_to(pos + 1);
                        if let Ok(line_str) = std::str::from_utf8(&line_raw) {
                            let line = line_str.trim();
                            if line.is_empty() { continue; }

                            if let Some(sse_chunks) = process_sse_line(line, &mut state) {
                                for sse_chunk in sse_chunks {
                                    yield Ok(sse_chunk);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(format!("Stream error: {}", e));
                    break;
                }
            }
        }

        // Send [DONE] marker
        yield Ok(Bytes::from("data: [DONE]\n\n"));
    })
}

/// Process a single SSE line from Gemini
fn process_sse_line(line: &str, state: &mut OpenAIStreamingState) -> Option<Vec<Bytes>> {
    if !line.starts_with("data: ") {
        return None;
    }

    let data_str = line[6..].trim();
    if data_str.is_empty() {
        return None;
    }

    // Gemini also sends [DONE]
    if data_str == "[DONE]" {
        return Some(vec![Bytes::from("data: [DONE]\n\n")]);
    }

    // Parse JSON
    let json_value: serde_json::Value = match serde_json::from_str(data_str) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let mut chunks = Vec::new();

    // Unwrap response field if present (v1internal wrapper)
    let raw_json = json_value.get("response").unwrap_or(&json_value);

    // Extract text content from candidates
    if let Some(text) = extract_text_from_response(raw_json) {
        if !text.is_empty() {
            let chunk = state.create_chunk(Some(&text), None, false);
            chunks.push(state.emit(chunk));
            state.first_chunk_sent = true;
        }
    }

    // Extract usage metadata (update on each chunk, final value will be used)
    if let Some(usage_meta) = raw_json.get("usageMetadata") {
        let prompt_tokens = usage_meta
            .get("promptTokenCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let completion_tokens = usage_meta
            .get("candidatesTokenCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let total_tokens = usage_meta
            .get("totalTokenCount")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        
        state.usage = Some(UsageStats {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        });
    }

    // Check for finish reason
    if let Some(finish_reason) = raw_json
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|cand| cand.get("finishReason"))
        .and_then(|f| f.as_str())
    {
        let openai_finish = match finish_reason {
            "STOP" => "stop",
            "MAX_TOKENS" => "length",
            "SAFETY" => "content_filter",
            _ => "stop",
        };
        
        // Include usage in the final chunk
        let chunk = state.create_chunk(None, Some(openai_finish), true);
        chunks.push(state.emit(chunk));
    }

    if chunks.is_empty() {
        None
    } else {
        Some(chunks)
    }
}

/// Extract text content from Gemini response
fn extract_text_from_response(response: &serde_json::Value) -> Option<String> {
    let parts = response
        .get("candidates")?
        .get(0)?
        .get("content")?
        .get("parts")?
        .as_array()?;

    let text: String = parts
        .iter()
        .filter_map(|part| {
            // Skip thought/thinking parts
            if part.get("thought").is_some() {
                return None;
            }
            part.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
        })
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_chunk() {
        let state = OpenAIStreamingState::new("gpt-4");
        let chunk = state.create_chunk(Some("Hello"), None, false);
        
        assert_eq!(chunk["id"].as_str().unwrap().starts_with("chatcmpl-"), true);
        assert_eq!(chunk["object"], "chat.completion.chunk");
        assert_eq!(chunk["model"], "gpt-4");
        assert_eq!(chunk["choices"][0]["delta"]["content"], "Hello");
        assert_eq!(chunk["choices"][0]["delta"]["role"], "assistant");
    }

    #[test]
    fn test_create_chunk_finish() {
        let mut state = OpenAIStreamingState::new("gpt-4");
        state.first_chunk_sent = true;
        let chunk = state.create_chunk(None, Some("stop"), false);
        
        assert_eq!(chunk["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn test_create_chunk_with_usage() {
        let mut state = OpenAIStreamingState::new("gpt-4");
        state.first_chunk_sent = true;
        state.usage = Some(UsageStats {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        });
        let chunk = state.create_chunk(None, Some("stop"), true);
        
        assert_eq!(chunk["choices"][0]["finish_reason"], "stop");
        assert_eq!(chunk["usage"]["prompt_tokens"], 10);
        assert_eq!(chunk["usage"]["completion_tokens"], 20);
        assert_eq!(chunk["usage"]["total_tokens"], 30);
    }

    #[test]
    fn test_extract_text() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello world"}]
                }
            }]
        });
        
        let text = extract_text_from_response(&response);
        assert_eq!(text, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_text_skips_thought() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"thought": true, "text": "thinking..."},
                        {"text": "Hello"}
                    ]
                }
            }]
        });
        
        let text = extract_text_from_response(&response);
        assert_eq!(text, Some("Hello".to_string()));
    }

    #[test]
    fn test_process_sse_line() {
        let mut state = OpenAIStreamingState::new("gpt-4");
        let line = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}"#;
        
        let result = process_sse_line(line, &mut state);
        assert!(result.is_some());
        
        let chunks = result.unwrap();
        let output = String::from_utf8(chunks[0].to_vec()).unwrap();
        assert!(output.contains("chat.completion.chunk"));
        assert!(output.contains("Hi"));
    }
}
