//! Kubernetes access seam. Filled in a later task.

/// Minimal k8s surface egret needs, kept behind a trait so tests can inject a
/// fake and kube-rs stays out of the templating layer.
pub trait KubeClient: Send + Sync {}

/// Real kube-rs backed client (wired up in a later task).
#[derive(Default)]
pub struct LiveKube {}
