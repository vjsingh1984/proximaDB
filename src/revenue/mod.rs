//! Enterprise Revenue Engine
//!
//! Complete enterprise revenue platform for usage metering, subscription billing,
//! and customer success automation implementing task_enterprise_revenue_engine_design.adoc

pub mod analytics;
pub mod billing;
pub mod customer_success;
pub mod metering;
pub mod subscriptions;

pub use metering::{UsageAggregate, UsageEvent, UsageEventType, UsageMeteringEngine};
// pub use subscriptions::{SubscriptionManager, EnterpriseSubscription, EnterprisePlan};
// pub use customer_success::{CustomerSuccessEngine, CustomerHealthScore, ExpansionOpportunity};
// pub use billing::{BillingEngine, Invoice, PricingConfig};
// pub use analytics::{RevenueAnalytics, RevenueReport, CustomerInsights};
