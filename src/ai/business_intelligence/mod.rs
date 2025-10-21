//! Business Intelligence Module
//!
//! Provides AI-powered business intelligence capabilities including
//! automated insight generation and executive dashboard creation.

pub mod engine;
pub mod insight_generator;
pub mod report_generator;
pub mod trend_analyzer;

pub use engine::{BIError, BusinessIntelligenceEngine};
pub use insight_generator::{BusinessInsight, InsightGenerator, InsightType};
pub use report_generator::{ExecutiveReport, ReportFormat, ReportGenerator};
pub use trend_analyzer::{TrendAnalysis, TrendAnalyzer, TrendDirection};
