// Claude 协议处理器 (CLI 版本 - 移除 z.ai 支持)

use axum::{
    body::Body,
    extract::{Json, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use tracing::{debug, info};

use crate::proxy::mappers::claude::{
    transform_claude_request_in, transform_response, create_claude_sse_stream, ClaudeRequest,
};
use crate::proxy::server::AppState;
use axum::http::HeaderMap;

const MAX_RETRY_ATTEMPTS: usize = 3;
const MIN_SIGNATURE_LENGTH: usize = 10;
const JITTER_FACTOR: f64 = 0.2;

use crate::proxy::mappers::claude::models::{ContentBlock, Message, MessageContent};

/// 检查 thinking 块是否有有效签名
fn has_valid_signature(block: &ContentBlock) -> bool {
    match block {
        ContentBlock::Thinking { signature, thinking, .. } => {
            if thinking.is_empty() && signature.is_some() {
                return true;
            }
            signature.as_ref().map_or(false, |s| s.len() >= MIN_SIGNATURE_LENGTH)
        }
        _ => true
    }
}

/// 清理 thinking 块
fn sanitize_thinking_block(block: ContentBlock) -> ContentBlock {
    match block {
        ContentBlock::Thinking { thinking, signature, .. } => {
            ContentBlock::Thinking {
                thinking,
                signature,
                cache_control: None,
            }
        }
        _ => block
    }
}

/// 过滤消息中的无效 thinking 块
fn filter_invalid_thinking_blocks(messages: &mut Vec<Message>) {
    for msg in messages.iter_mut() {
        if msg.role != "assistant" && msg.role != "model" {
            continue;
        }
        
        if let MessageContent::Array(blocks) = &mut msg.content {
            let mut new_blocks = Vec::new();
            for block in blocks.drain(..) {
                if matches!(block, ContentBlock::Thinking { .. }) {
                    if has_valid_signature(&block) {
                        new_blocks.push(sanitize_thinking_block(block));
                    } else {
                        if let ContentBlock::Thinking { thinking, .. } = &block {
                            if !thinking.is_empty() {
                                new_blocks.push(ContentBlock::Text { text: thinking.clone() });
                            }
                        }
                    }
                } else {
                    new_blocks.push(block);
                }
            }
            
            *blocks = new_blocks;
            
            if blocks.is_empty() {
                blocks.push(ContentBlock::Text { text: String::new() });
            }
        }
    }
}

/// 移除尾部的无签名 thinking 块
fn remove_trailing_unsigned_thinking(blocks: &mut Vec<ContentBlock>) {
    if blocks.is_empty() {
        return;
    }
    
    let mut end_index = blocks.len();
    for i in (0..blocks.len()).rev() {
        match &blocks[i] {
            ContentBlock::Thinking { .. } => {
                if !has_valid_signature(&blocks[i]) {
                    end_index = i;
                } else {
                    break;
                }
            }
            _ => break
        }
    }
    
    if end_index < blocks.len() {
        blocks.truncate(end_index);
    }
}

/// Apply jitter to delay
fn apply_jitter(delay_ms: u64) -> u64 {
    use rand::Rng;
    let jitter_range = (delay_ms as f64 * JITTER_FACTOR) as i64;
    let jitter: i64 = rand::rng().random_range(-jitter_range..=jitter_range);
    ((delay_ms as i64) + jitter).max(1) as u64
}

/// 处理 Claude messages 请求
pub async fn handle_messages(
    State(state): State<AppState>,
    _headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // 生成随机 Trace ID
    let trace_id: String = {
        use rand::Rng;
        rand::rng()
            .sample_iter(&rand::distr::Alphanumeric)
            .take(6)
            .map(char::from)
            .collect::<String>()
            .to_lowercase()
    };
    
    // 解析请求
    let mut request: ClaudeRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": format!("Invalid request body: {}", e)
                    }
                }))
            ).into_response();
        }
    };

    // 过滤无效 Thinking 块
    filter_invalid_thinking_blocks(&mut request.messages);
    
    info!(
        "[{}] Claude Request | Model: {} | Stream: {} | Messages: {}",
        trace_id,
        request.model,
        request.stream,
        request.messages.len()
    );

    let upstream = state.upstream.clone();
    let token_manager = state.token_manager;
    
    let pool_size = token_manager.len();
    let max_attempts = MAX_RETRY_ATTEMPTS.min(pool_size).max(1);

    let mut last_error = String::new();
    let mut last_status: Option<u16> = None;
    let mut request_for_body = request.clone();
    
    for attempt in 0..max_attempts {
        // 模型路由
        let mapped_model = crate::proxy::common::model_mapping::resolve_model_route(
            &request_for_body.model,
            &*state.custom_mapping.read().await,
            &*state.openai_mapping.read().await,
            &*state.anthropic_mapping.read().await,
            true,
        );
        
        // 将 Claude 工具转为 Value 数组
        let tools_val: Option<Vec<Value>> = request_for_body.tools.as_ref().map(|list| {
            list.iter().map(|t| serde_json::to_value(t).unwrap_or(json!({}))).collect()
        });

        let config = crate::proxy::mappers::common_utils::resolve_request_config(
            &request_for_body.model, 
            &mapped_model, 
            &tools_val
        );

        // 获取 token
        let force_rotate = attempt > 0;
        let (access_token, project_id, email) = match token_manager.get_token(&config.request_type, force_rotate, None).await {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "type": "error",
                        "error": {
                            "type": "overloaded_error",
                            "message": format!("No available accounts: {}", e)
                        }
                    }))
                ).into_response();
            }
        };

        info!("[{}] Using account: {} (model: {})", trace_id, email, mapped_model);
        
        // 准备请求
        let mut request_with_mapped = request_for_body.clone();
        
        // 清理尾部无签名 thinking 块
        for msg in request_with_mapped.messages.iter_mut() {
            if msg.role == "assistant" || msg.role == "model" {
                if let MessageContent::Array(blocks) = &mut msg.content {
                    remove_trailing_unsigned_thinking(blocks);
                }
            }
        }
        
        request_with_mapped.model = mapped_model.clone();

        // 转换请求
        let gemini_body = match transform_claude_request_in(&request_with_mapped, &project_id) {
            Ok(b) => {
                debug!("[{}] Transformed body: {}", trace_id, serde_json::to_string_pretty(&b).unwrap_or_default());
                b
            },
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "type": "error",
                        "error": {
                            "type": "api_error",
                            "message": format!("Transform error: {}", e)
                        }
                    }))
                ).into_response();
            }
        };
        
        // 调用上游
        let is_stream = request.stream;
        let method = if is_stream { "streamGenerateContent" } else { "generateContent" };
        let query = if is_stream { Some("alt=sse") } else { None };

        let response = match upstream.call_v1_internal(method, &access_token, gemini_body, query).await {
            Ok(r) => r,
            Err(e) => {
                last_error = e.clone();
                debug!("[{}] Request failed: {}", trace_id, e);
                continue;
            }
        };
        
        let status = response.status();
        
        // 成功
        if status.is_success() {
            if request.stream {
                let stream = response.bytes_stream();
                let gemini_stream = Box::pin(stream);
                let claude_stream = create_claude_sse_stream(gemini_stream, trace_id, email);

                let sse_stream = claude_stream.map(|result| -> Result<Bytes, std::io::Error> {
                    match result {
                        Ok(bytes) => Ok(bytes),
                        Err(e) => Ok(Bytes::from(format!("data: {{\"error\":\"{}\"}}\n\n", e))),
                    }
                });

                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .header(header::CONNECTION, "keep-alive")
                    .body(Body::from_stream(sse_stream))
                    .unwrap();
            } else {
                let bytes = match response.bytes().await {
                    Ok(b) => b,
                    Err(e) => return (StatusCode::BAD_GATEWAY, format!("Failed to read body: {}", e)).into_response(),
                };

                let gemini_resp: Value = match serde_json::from_slice(&bytes) {
                    Ok(v) => v,
                    Err(e) => return (StatusCode::BAD_GATEWAY, format!("Parse error: {}", e)).into_response(),
                };

                let raw = gemini_resp.get("response").unwrap_or(&gemini_resp);

                let gemini_response: crate::proxy::mappers::claude::models::GeminiResponse = 
                    match serde_json::from_value(raw.clone()) {
                        Ok(r) => r,
                        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Convert error: {}", e)).into_response(),
                    };
                
                let claude_response = match transform_response(&gemini_response) {
                    Ok(r) => r,
                    Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Transform error: {}", e)).into_response(),
                };

                info!(
                    "[{}] Completed | In: {} | Out: {}", 
                    trace_id, 
                    claude_response.usage.input_tokens, 
                    claude_response.usage.output_tokens
                );

                return Json(claude_response).into_response();
            }
        }
        
        // 处理错误
        let status_code = status.as_u16();
        let error_text = response.text().await.unwrap_or_else(|_| format!("HTTP {}", status));
        last_error = format!("HTTP {}: {}", status_code, error_text);
        last_status = Some(status_code);
        
        debug!("[{}] Upstream error: {}", trace_id, last_error);
        
        // 重试逻辑
        if attempt + 1 < max_attempts {
            let delay_ms = match status_code {
                429 => apply_jitter(1000 * (attempt as u64 + 1)),
                503 | 529 | 500 => apply_jitter(500 * (attempt as u64 + 1)),
                _ => 0,
            };
            
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
    
    // 所有重试失败 - 保留原始状态码
    let response_status = match last_status {
        Some(429) => StatusCode::TOO_MANY_REQUESTS,
        Some(code) if code >= 400 && code < 600 => {
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY)
        }
        _ => StatusCode::BAD_GATEWAY,
    };

    // 对于 429，使用 rate_limit_error 类型增加语义
    let error_type = if last_status == Some(429) {
        "rate_limit_error"
    } else {
        "api_error"
    };

    (
        response_status,
        Json(json!({
            "type": "error",
            "error": {
                "type": error_type,
                "message": last_error
            }
        }))
    ).into_response()
}
