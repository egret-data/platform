//! `io.kestra.plugin.ee.kubernetes.runner.Kubernetes`, egret's implementation
//! (W5a Task 4, spec §5): **one pod per task-run attempt**.
//!
//! Kestra ships a Kubernetes *task runner* only in its Enterprise edition, so
//! there is no artifact to read for the runner itself. The shape is taken from
//! the OSS `io.kestra.plugin.kubernetes.core.PodCreate` (plugin-kubernetes
//! 1.11.0, in `kestra/kestra:v2.0.0-rc13`, read with `javap -p -c -constants`),
//! which moves files the same way an EE runner has to:
//!
//! | Kestra `PodCreate` / `AbstractPod` | egret |
//! |---|---|
//! | init container `init-files`, sidecar `out-files` | [`pod_spec::FILES_IN`] (`files-in`), [`pod_spec::FILES_OUT`] (`files-out`) |
//! | one `emptyDir` `kestra-files` at `/kestra`, working dir `/kestra/working-dir`, markers `/kestra/ready` and `/kestra/ended` in the same volume | the working dir [`WORKING_DIR`](super::commands::WORKING_DIR) is its own `emptyDir`; the markers live in a SECOND `emptyDir` at [`pod_spec::MARKERS_DIR`] that `main` does not mount, so the author's command cannot fake "ready" or "ended" |
//! | helper script `echo 'waiting to be <m>!'; while [ ! -f /kestra/<m> ]; do sleep 0.5; done; echo '<m> successfully'` | the same loop, [`pod_spec::wait_script`] |
//! | upload: exec + tar into `init-files`, then verify (`find … \| wc -l`, `wc -c < f`), then upload the `ready` marker | the same three steps ([`runner`]): `tar -x` from stdin, `wc -c <` per file compared with the bytes sent, `touch ready` |
//! | download: exec + tar out of `out-files`, then the `ended` marker "to exit gracefully before pod deletion" | the same, one `tar -c` per declared file, read as a stream into internal storage (W9a Task 6: [`TarReader`](crate::storage::transfer::TarReader), [`crate::storage::collect`]) |
//! | log: `PodLogService` follows `usingTimestamps()`, `LoggingOutputStream` splits off the timestamp and calls `logConsumer.accept(line, false)` — every line as stdout | **egret separates stderr** (fix round 1): a shell script's fd 2 goes to a file `files-out` streams, so stderr stays ERROR as W11 measured; only a non-shell `interpreter` keeps the merged stream (INFO), with a WARN line saying so — see [`pod_spec::stderr_is_separable`] |
//! | `LoggingOutputStream` also drops a line whose `hashCode` it has seen before | **not ported**: it drops a legitimately repeated line (`echo retry` twice). egret skips only the exact `(kubelet timestamp, line)` pairs it has already read, and only across a reconnect (`runner::Seen`) |
//! | pods identified by labels | pods also get a deterministic NAME, [`pod_spec::pod_name`], keyed by **(run, attempt)** — the re-attach key |

pub mod pod_spec;
pub mod runner;

pub use pod_spec::{pod_name, PodConfig};
pub use runner::{sweep_orphans, KubernetesRunner};
