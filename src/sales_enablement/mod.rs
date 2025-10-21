//! Sales Enablement Platform
//!
//! Customer-facing sales tools and trial automation implementing
//! task_sales_enablement_platform_design.adoc

pub mod analytics;
pub mod competitive;
pub mod demos;
pub mod trial_platform;

pub use analytics::{ConversionAnalysis, CustomerEngagementTracker, SalesPipelineAnalytics};
pub use competitive::{CompetitiveAnalysis, CompetitiveIntelligence, PositioningRecommendation};
pub use demos::{AIShowcasePlatform, DemoScenario, DemonstrationResult};
pub use trial_platform::{EnterpriseTrial, EnterpriseTrialManager, TrialStatus, TrialType};
