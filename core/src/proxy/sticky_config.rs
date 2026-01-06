//! Sticky session configuration

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingMode {
    /// Performance first - no session stickiness
    PerformanceFirst,
    /// Balance - moderate session stickiness
    Balance,
    /// Cache first - maximum session stickiness
    CacheFirst,
}

impl Default for SchedulingMode {
    fn default() -> Self {
        Self::Balance
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickySessionConfig {
    #[serde(default)]
    pub mode: SchedulingMode,
    #[serde(default = "default_max_wait_seconds")]
    pub max_wait_seconds: u64,
}

impl Default for StickySessionConfig {
    fn default() -> Self {
        Self {
            mode: SchedulingMode::default(),
            max_wait_seconds: default_max_wait_seconds(),
        }
    }
}

fn default_max_wait_seconds() -> u64 {
    30
}
