//! Rate limit tracking

use dashmap::DashMap;
use std::time::{Duration, Instant};

pub struct RateLimitTracker {
    /// account_id -> (reset_time, reason)
    limits: DashMap<String, (Instant, String)>,
}

impl RateLimitTracker {
    pub fn new() -> Self {
        Self {
            limits: DashMap::new(),
        }
    }
    
    /// Mark an account as rate limited
    pub fn mark_limited(&self, account_id: &str, duration_secs: u64, reason: &str) {
        let reset_time = Instant::now() + Duration::from_secs(duration_secs);
        self.limits.insert(account_id.to_string(), (reset_time, reason.to_string()));
    }
    
    /// Parse rate limit from error response
    pub fn parse_from_error(
        &self,
        account_id: &str,
        status: u16,
        retry_after_header: Option<&str>,
        error_body: &str,
    ) {
        // Default wait time based on status
        let mut wait_secs = match status {
            429 => 60,       // Too Many Requests
            503 => 30,       // Service Unavailable
            500..=599 => 10, // Other server errors
            _ => return,     // Don't mark for other statuses
        };
        
        // Try to parse Retry-After header
        if let Some(retry_after) = retry_after_header {
            if let Ok(secs) = retry_after.parse::<u64>() {
                wait_secs = secs;
            }
        }
        
        // Try to parse Google's RetryInfo from error body
        if error_body.contains("retryDelay") {
            if let Some(delay) = Self::parse_retry_delay(error_body) {
                wait_secs = delay;
            }
        }
        
        let reason = format!("HTTP {} - {}", status, &error_body[..error_body.len().min(200)]);
        self.mark_limited(account_id, wait_secs, &reason);
        
        tracing::warn!("Account {} rate limited for {}s: {}", account_id, wait_secs, reason);
    }
    
    /// Parse retryDelay from Google error response
    fn parse_retry_delay(body: &str) -> Option<u64> {
        // Look for patterns like "retryDelay": "60s" or "retry_delay": {"seconds": 60}
        let re = regex::Regex::new(r#"(?:"retryDelay"|"retry_delay")\s*:\s*"?(\d+)"#).ok()?;
        re.captures(body)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse().ok())
    }
    
    /// Check if account is currently rate limited
    pub fn is_rate_limited(&self, account_id: &str) -> bool {
        if let Some(entry) = self.limits.get(account_id) {
            if Instant::now() < entry.0 {
                return true;
            }
            // Expired, remove it
            drop(entry);
            self.limits.remove(account_id);
        }
        false
    }
    
    /// Get remaining wait time in seconds
    pub fn get_remaining_wait(&self, account_id: &str) -> u64 {
        if let Some(entry) = self.limits.get(account_id) {
            let remaining = entry.0.saturating_duration_since(Instant::now());
            return remaining.as_secs();
        }
        0
    }
    
    /// Get reset time in seconds (None if not limited)
    pub fn get_reset_seconds(&self, account_id: &str) -> Option<u64> {
        if let Some(entry) = self.limits.get(account_id) {
            if Instant::now() < entry.0 {
                return Some(entry.0.saturating_duration_since(Instant::now()).as_secs());
            }
        }
        None
    }
    
    /// Clear rate limit for account
    pub fn clear(&self, account_id: &str) -> bool {
        self.limits.remove(account_id).is_some()
    }
    
    /// Cleanup expired entries
    pub fn cleanup_expired(&self) -> usize {
        let now = Instant::now();
        let mut removed = 0;
        self.limits.retain(|_, (reset_time, _)| {
            if now >= *reset_time {
                removed += 1;
                false
            } else {
                true
            }
        });
        removed
    }
}

impl Default for RateLimitTracker {
    fn default() -> Self {
        Self::new()
    }
}
