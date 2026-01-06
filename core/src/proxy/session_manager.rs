//! Session Manager for extracting session fingerprints
//! Used for sticky session routing

use sha2::{Sha256, Digest};
use crate::proxy::mappers::claude::models::ClaudeRequest;

pub struct SessionManager;

impl SessionManager {
    /// Extract a session ID from Claude request for sticky routing
    pub fn extract_session_id(request: &ClaudeRequest) -> String {
        // Use metadata.user_id if available
        if let Some(metadata) = &request.metadata {
            if let Some(user_id) = &metadata.user_id {
                return user_id.clone();
            }
        }
        
        // Otherwise, generate a fingerprint from the request content
        let mut hasher = Sha256::new();
        
        // Hash the model
        hasher.update(request.model.as_bytes());
        
        // Hash first message content (if any)
        if let Some(first_msg) = request.messages.first() {
            let content_str = match &first_msg.content {
                crate::proxy::mappers::claude::models::MessageContent::String(s) => s.clone(),
                crate::proxy::mappers::claude::models::MessageContent::Array(arr) => {
                    arr.iter()
                        .filter_map(|b| {
                            match b {
                                crate::proxy::mappers::claude::models::ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("")
                }
            };
            hasher.update(content_str.as_bytes());
        }
        
        let result = hasher.finalize();
        format!("{:x}", result)[..16].to_string()
    }
}
