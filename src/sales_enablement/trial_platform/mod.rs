//! Trial Platform Module

pub mod trial_manager;

pub use trial_manager::{
    EnterpriseTrial, EnterpriseTrialManager, EvaluationProgress, TrialCreationRequest,
    TrialEnvironment, TrialStatus, TrialType,
};
