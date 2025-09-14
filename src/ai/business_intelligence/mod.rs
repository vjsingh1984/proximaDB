//! Business Intelligence Module
//!
//! Provides AI-powered business intelligence capabilities including
//! automated insight generation and executive dashboard creation.

pub mod engine;
pub mod insight_generator;
pub mod report_generator;
pub mod trend_analyzer;

pub use engine::{BusinessIntelligenceEngine, BIError};
pub use insight_generator::{InsightGenerator, BusinessInsight, InsightType};
pub use report_generator::{ReportGenerator, ExecutiveReport, ReportFormat};
pub use trend_analyzer::{TrendAnalyzer, TrendAnalysis, TrendDirection};