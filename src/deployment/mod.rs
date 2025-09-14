//! Enterprise Deployment Automation
//!
//! One-click enterprise deployment automation system implementing
//! the design from task_deployment_automation_design.adoc

pub mod discovery;
pub mod automation;
pub mod platforms;
pub mod validation;

pub use discovery::{EnvironmentDetector, DetectedEnvironment, PlatformType};
pub use automation::{DeploymentProvisioner, DeploymentRequest, DeploymentResult};
pub use platforms::{KubernetesDeployment, DockerDeployment, CloudDeployment};
pub use validation::{DeploymentValidator, HealthCheck, ValidationResult};