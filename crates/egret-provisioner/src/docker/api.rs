//! The seam between the provisioner and a Docker daemon. `DockerApi` is what the
//! provisioner needs and nothing more; the real client (bollard) lives behind it,
//! and `FakeDocker` stands in for it in tests.

use super::plan::ContainerPlan;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Restarting,
    /// `healthy` is `None` when the image has no health check configured.
    Running {
        healthy: Option<bool>,
    },
    Exited {
        code: i64,
    },
    Dead,
}

#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub state: ContainerState,
}

#[async_trait::async_trait]
pub trait DockerApi: Send + Sync {
    /// Pull only when the image is absent locally.
    async fn ensure_image(&self, image: &str) -> Result<(), String>;
    /// Create, upload the plan's files, start. Removes the container if any later
    /// step fails, so an `Err` leaves nothing behind.
    async fn create_and_start(&self, plan: &ContainerPlan, network: &str) -> Result<(), String>;
    /// Containers carrying every label in `label_filter`.
    async fn list(
        &self,
        label_filter: &BTreeMap<String, String>,
    ) -> Result<Vec<ContainerInfo>, String>;
    async fn restart(&self, name: &str) -> Result<(), String>;
    /// A missing container is `Ok`.
    async fn stop_and_remove(&self, name: &str) -> Result<(), String>;
    async fn log_tail(&self, name: &str, lines: usize) -> Result<String, String>;
}

#[cfg(test)]
pub use fake::FakeDocker;

#[cfg(test)]
mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct State {
        containers: Vec<(ContainerPlan, ContainerInfo, String)>, // plan, info, network
        foreign: Vec<ContainerInfo>,
        calls: Vec<String>,
        /// op -> number of successful calls to let through before one failure.
        fail: HashMap<String, usize>,
        log: String,
        /// Make `list` ignore its filter, as a misbehaving daemon or bridge could.
        list_ignores_filter: bool,
    }

    /// In-memory daemon. Records every call as `"op"` or `"op:name"`.
    #[derive(Default)]
    pub struct FakeDocker {
        state: Mutex<State>,
    }

    impl FakeDocker {
        pub fn new() -> Self {
            Self::default()
        }

        /// The next call to `op` fails once.
        pub fn fail_next(&self, op: &str) {
            self.fail_after(op, 0);
        }

        /// Let `n` calls to `op` succeed, then fail one.
        pub fn fail_after(&self, op: &str, n: usize) {
            self.state.lock().unwrap().fail.insert(op.to_string(), n);
        }

        pub fn set_log(&self, log: &str) {
            self.state.lock().unwrap().log = log.to_string();
        }

        pub fn set_state(&self, name: &str, state: ContainerState) {
            let mut s = self.state.lock().unwrap();
            for (_, info, _) in s.containers.iter_mut().filter(|c| c.1.name == name) {
                info.state = state.clone();
            }
        }

        /// A container this fake holds but that is not one of ours.
        pub fn add_foreign(&self, name: &str, labels: &[(&str, &str)]) {
            self.state.lock().unwrap().foreign.push(ContainerInfo {
                name: name.to_string(),
                labels: labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                state: ContainerState::Running { healthy: None },
            });
        }

        pub fn list_ignores_filter(&self) {
            self.state.lock().unwrap().list_ignores_filter = true;
        }

        pub fn calls(&self) -> Vec<String> {
            self.state.lock().unwrap().calls.clone()
        }

        pub fn calls_to(&self, op: &str) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|c| c == op || c.starts_with(&format!("{op}:")))
                .collect()
        }

        /// Names of the containers currently held (ours and foreign), sorted.
        pub fn names(&self) -> Vec<String> {
            let s = self.state.lock().unwrap();
            let mut v: Vec<String> = s
                .containers
                .iter()
                .map(|c| c.1.name.clone())
                .chain(s.foreign.iter().map(|f| f.name.clone()))
                .collect();
            v.sort();
            v
        }

        pub fn plan_of(&self, name: &str) -> Option<ContainerPlan> {
            let s = self.state.lock().unwrap();
            s.containers
                .iter()
                .find(|c| c.1.name == name)
                .map(|c| c.0.clone())
        }

        pub fn network_of(&self, name: &str) -> Option<String> {
            let s = self.state.lock().unwrap();
            s.containers
                .iter()
                .find(|c| c.1.name == name)
                .map(|c| c.2.clone())
        }

        /// Record the call and apply any armed failure for `op`.
        fn enter(&self, op: &str, name: Option<&str>) -> Result<(), String> {
            let mut s = self.state.lock().unwrap();
            s.calls.push(match name {
                Some(n) => format!("{op}:{n}"),
                None => op.to_string(),
            });
            match s.fail.get_mut(op) {
                Some(0) => {
                    s.fail.remove(op);
                    Err(format!("fake {op} failure"))
                }
                Some(n) => {
                    *n -= 1;
                    Ok(())
                }
                None => Ok(()),
            }
        }
    }

    #[async_trait::async_trait]
    impl DockerApi for FakeDocker {
        async fn ensure_image(&self, image: &str) -> Result<(), String> {
            self.enter("ensure_image", Some(image))
        }

        async fn create_and_start(
            &self,
            plan: &ContainerPlan,
            network: &str,
        ) -> Result<(), String> {
            self.enter("create_and_start", Some(&plan.name))?;
            let info = ContainerInfo {
                name: plan.name.clone(),
                labels: plan.labels.clone(),
                state: ContainerState::Running { healthy: None },
            };
            self.state
                .lock()
                .unwrap()
                .containers
                .push((plan.clone(), info, network.to_string()));
            Ok(())
        }

        async fn list(
            &self,
            label_filter: &BTreeMap<String, String>,
        ) -> Result<Vec<ContainerInfo>, String> {
            self.enter("list", None)?;
            let s = self.state.lock().unwrap();
            let matches = |i: &ContainerInfo| {
                s.list_ignores_filter
                    || label_filter.iter().all(|(k, v)| i.labels.get(k) == Some(v))
            };
            Ok(s.containers
                .iter()
                .map(|c| &c.1)
                .chain(s.foreign.iter())
                .filter(|i| matches(i))
                .cloned()
                .collect())
        }

        async fn restart(&self, name: &str) -> Result<(), String> {
            self.enter("restart", Some(name))
        }

        async fn stop_and_remove(&self, name: &str) -> Result<(), String> {
            self.enter("stop_and_remove", Some(name))?;
            let mut s = self.state.lock().unwrap();
            s.containers.retain(|c| c.1.name != name);
            s.foreign.retain(|f| f.name != name);
            Ok(())
        }

        async fn log_tail(&self, name: &str, lines: usize) -> Result<String, String> {
            self.enter("log_tail", Some(&format!("{name}:{lines}")))?;
            Ok(self.state.lock().unwrap().log.clone())
        }
    }
}
