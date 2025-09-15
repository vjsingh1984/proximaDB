//! Trial Platform Module

pub mod trial_manager;

pub use trial_manager::{
    EnterpriseTrialManager, EnterpriseTrial, TrialType, TrialStatus,
    TrialCreationRequest, TrialEnvironment, EvaluationProgress
};