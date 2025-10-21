//! Executive Intelligence module for C-level strategic analytics

pub mod intelligence_platform;

pub use intelligence_platform::{
    AutomatedBoardReport, ExecutiveIntelligenceDashboard, ExecutiveIntelligencePlatform,
    ExecutiveRole,
};

use anyhow::Result;

/// Executive intelligence coordinator for C-level analytics
pub struct ExecutiveIntelligenceCoordinator {
    /// Executive intelligence platform
    intelligence_platform: intelligence_platform::ExecutiveIntelligencePlatform,
}

impl ExecutiveIntelligenceCoordinator {
    /// Create executive intelligence coordinator
    pub async fn new() -> Result<Self> {
        Ok(Self {
            intelligence_platform: intelligence_platform::ExecutiveIntelligencePlatform::new()
                .await?,
        })
    }

    /// Get executive intelligence platform
    pub fn get_platform(&self) -> &intelligence_platform::ExecutiveIntelligencePlatform {
        &self.intelligence_platform
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_executive_intelligence_coordinator_creation() {
        let coordinator = ExecutiveIntelligenceCoordinator::new().await.unwrap();
        // Basic validation that coordinator was created
        assert!(true);
    }
}
