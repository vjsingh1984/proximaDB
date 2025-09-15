//! Sales Enablement Platform
//!
//! Customer-facing sales tools and trial automation implementing
//! task_sales_enablement_platform_design.adoc

pub mod trial_platform;
pub mod demos;
pub mod analytics;
pub mod competitive;

pub use trial_platform::{EnterpriseTrialManager, EnterpriseTrial, TrialType, TrialStatus};
pub use demos::{AIShowcasePlatform, DemoScenario, DemonstrationResult};
pub use analytics::{SalesPipelineAnalytics, ConversionAnalysis, CustomerEngagementTracker};
pub use competitive::{CompetitiveIntelligence, CompetitiveAnalysis, PositioningRecommendation};