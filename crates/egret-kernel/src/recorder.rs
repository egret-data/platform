//! Output recorder: taps the Jupyter channels WS, correlates execute_request →
//! iopub by msg_id, accumulates per-cell outputs, and persists them on `idle`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use uuid::Uuid;

use crate::jmsg::JMsg;
use egret_notebook::output::CellOutput;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrameDir {
    ClientToKernel,
    KernelToClient,
}

/// Persistence seam (real impl wraps NotebookStore::record_output).
#[async_trait::async_trait]
pub trait OutputSink: Send + Sync {
    async fn record(
        &self,
        nb: Uuid,
        cid: Uuid,
        outputs: Vec<CellOutput>,
        execution_count: Option<i32>,
        elapsed_ms: Option<i32>,
    );
}

/// In-memory registry of currently-running cells per (notebook, user), for the
/// `active-executions` restore endpoint. Cheap clone (Arc).
type RunningMap = HashMap<(Uuid, Uuid), Vec<Uuid>>;

#[derive(Clone, Default)]
pub struct ActiveExecutions(Arc<Mutex<RunningMap>>);

impl ActiveExecutions {
    pub fn running(&self, nb: Uuid, user: Uuid) -> Vec<Uuid> {
        self.0
            .lock()
            .unwrap()
            .get(&(nb, user))
            .cloned()
            .unwrap_or_default()
    }
    fn add(&self, nb: Uuid, user: Uuid, cid: Uuid) {
        self.0
            .lock()
            .unwrap()
            .entry((nb, user))
            .or_default()
            .push(cid);
    }
    fn remove(&self, nb: Uuid, user: Uuid, cid: Uuid) {
        if let Some(v) = self.0.lock().unwrap().get_mut(&(nb, user)) {
            v.retain(|c| *c != cid);
        }
    }
}

struct Pending {
    cell_id: Uuid,
    started: Instant,
    outputs: Vec<CellOutput>,
    execution_count: Option<i32>,
}

pub struct Recorder {
    nb: Uuid,
    user: Uuid,
    sink: Arc<dyn OutputSink>,
    active: ActiveExecutions,
    pending: Mutex<HashMap<String, Pending>>, // execute_request msg_id → accumulator
}

impl Recorder {
    pub fn new(nb: Uuid, user: Uuid, sink: Arc<dyn OutputSink>, active: ActiveExecutions) -> Self {
        Self {
            nb,
            user,
            sink,
            active,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn on_frame(&self, dir: FrameDir, bytes: &[u8]) {
        let Some(m) = JMsg::parse(bytes) else {
            return;
        };
        match dir {
            FrameDir::ClientToKernel => self.on_client(m),
            FrameDir::KernelToClient => self.on_kernel(m),
        }
    }

    fn on_client(&self, m: JMsg) {
        if m.header.msg_type != "execute_request" {
            return;
        }
        let Some(cell_id) = m
            .metadata
            .get("egret_cell_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            return;
        };
        self.active.add(self.nb, self.user, cell_id);
        self.pending.lock().unwrap().insert(
            m.header.msg_id,
            Pending {
                cell_id,
                started: Instant::now(),
                outputs: Vec::new(),
                execution_count: None,
            },
        );
    }

    fn on_kernel(&self, m: JMsg) {
        let parent = m.parent_header.msg_id;
        if parent.is_empty() {
            return;
        }
        // finalize on idle (persist + clear); otherwise accumulate.
        let mut guard = self.pending.lock().unwrap();
        let Some(p) = guard.get_mut(&parent) else {
            return;
        };
        match m.header.msg_type.as_str() {
            "execute_input" => {
                p.execution_count = m
                    .content
                    .get("execution_count")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);
            }
            "stream" => {
                let name = m
                    .content
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stdout")
                    .to_string();
                let text = m
                    .content
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                p.outputs.push(CellOutput::Stream { name, text });
            }
            "execute_result" => {
                p.outputs.push(CellOutput::ExecuteResult {
                    data: m.content.get("data").cloned().unwrap_or_default(),
                });
            }
            "display_data" => {
                p.outputs.push(CellOutput::DisplayData {
                    data: m.content.get("data").cloned().unwrap_or_default(),
                });
            }
            "error" => {
                p.outputs.push(CellOutput::Error {
                    ename: m
                        .content
                        .get("ename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    evalue: m
                        .content
                        .get("evalue")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    traceback: m
                        .content
                        .get("traceback")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            "status"
                if m.content.get("execution_state").and_then(|v| v.as_str()) == Some("idle") =>
            {
                let p = guard.remove(&parent).unwrap();
                self.active.remove(self.nb, self.user, p.cell_id);
                let (nb, user, cid) = (self.nb, self.user, p.cell_id);
                let ec = p.execution_count;
                let ms = i32::try_from(p.started.elapsed().as_millis()).ok();
                let outs = p.outputs;
                let sink = self.sink.clone();
                let _ = user; // captured for potential per-user store scoping
                tokio::spawn(async move {
                    sink.record(nb, cid, outs, ec, ms).await;
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_notebook::output::CellOutput;
    use std::sync::Mutex as StdMutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct FakeSink {
        calls: StdMutex<Vec<(Uuid, Vec<CellOutput>, Option<i32>)>>,
    }
    #[async_trait::async_trait]
    impl OutputSink for FakeSink {
        async fn record(
            &self,
            _nb: Uuid,
            cid: Uuid,
            outs: Vec<CellOutput>,
            ec: Option<i32>,
            _ms: Option<i32>,
        ) {
            self.calls.lock().unwrap().push((cid, outs, ec));
        }
    }
    fn exec_request(msg_id: &str, cell_id: Uuid) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "header": {"msg_id": msg_id, "msg_type": "execute_request"},
            "metadata": {"egret_cell_id": cell_id},
            "content": {"code": "1"}}))
        .unwrap()
    }
    fn iopub(parent: &str, msg_type: &str, content: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "header": {"msg_id": "x", "msg_type": msg_type},
            "parent_header": {"msg_id": parent, "msg_type": "execute_request"},
            "content": content}))
        .unwrap()
    }

    #[tokio::test]
    async fn accumulates_and_persists_on_idle() {
        let sink = std::sync::Arc::new(FakeSink::default());
        let cid = Uuid::now_v7();
        let rec = Recorder::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            sink.clone(),
            Default::default(),
        );
        rec.on_frame(FrameDir::ClientToKernel, &exec_request("m1", cid));
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m1",
                "execute_input",
                serde_json::json!({"execution_count": 7}),
            ),
        );
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m1",
                "stream",
                serde_json::json!({"name":"stdout","text":"hi"}),
            ),
        );
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m1",
                "status",
                serde_json::json!({"execution_state":"idle"}),
            ),
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let calls = sink.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, cid);
        assert_eq!(calls[0].2, Some(7));
        assert!(matches!(calls[0].1[0], CellOutput::Stream { .. }));
    }

    #[tokio::test]
    async fn active_executions_tracks_running_then_clears() {
        let reg: ActiveExecutions = Default::default();
        let (nb, user) = (Uuid::now_v7(), Uuid::now_v7());
        let rec = Recorder::new(
            nb,
            user,
            std::sync::Arc::new(FakeSink::default()),
            reg.clone(),
        );
        let cid = Uuid::now_v7();
        rec.on_frame(FrameDir::ClientToKernel, &exec_request("m1", cid));
        assert_eq!(reg.running(nb, user), vec![cid]);
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m1",
                "status",
                serde_json::json!({"execution_state":"idle"}),
            ),
        );
        assert!(reg.running(nb, user).is_empty());
    }

    #[tokio::test]
    async fn error_output_captured() {
        let sink = std::sync::Arc::new(FakeSink::default());
        let cid = Uuid::now_v7();
        let rec = Recorder::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            sink.clone(),
            Default::default(),
        );
        rec.on_frame(FrameDir::ClientToKernel, &exec_request("m2", cid));
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m2",
                "error",
                serde_json::json!({"ename":"ValueError","evalue":"bad","traceback":["a","b"]}),
            ),
        );
        rec.on_frame(
            FrameDir::KernelToClient,
            &iopub(
                "m2",
                "status",
                serde_json::json!({"execution_state":"idle"}),
            ),
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let calls = sink.calls.lock().unwrap();
        assert!(matches!(&calls[0].1[0], CellOutput::Error { ename, .. } if ename == "ValueError"));
    }
}
