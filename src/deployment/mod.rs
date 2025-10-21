//! Enterprise Deployment Automation
//!
//! One-click enterprise deployment automation system implementing
//! the design from task_deployment_automation_design.adoc

pub mod automation;
pub mod discovery;
pub mod platforms;
pub mod validation;

pub use automation::{DeploymentProvisioner, DeploymentResult, EnterpriseDeploymentRequest};
pub use discovery::{DetectedEnvironment, EnvironmentDetector, PlatformType};
// Platform implementations are basic stubs for now
// pub use platforms::{KubernetesDeployment, DockerDeployment, CloudDeployment};
// pub use validation::{DeploymentValidator, HealthCheck, ValidationResult};
