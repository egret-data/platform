// kestra: core/src/main/java/io/kestra/core/models/
//! Flow, task, execution, and trigger models. No I/O.

mod execution;
mod flow;
mod input;
// `pub(crate)` since group C1 Task 2: `property::render_map` is reached by its
// module path from the worker handlers that read a map property.
pub(crate) mod property;
mod state;
mod task;
mod trigger;
mod wire;
pub use execution::{
    Execution, ExecutionKind, StateHistory, StateTransition, TaskRun, TaskRunAttempt,
};
// The backfill create route's `labels` (W7f D24).
pub(crate) use flow::deserialize_labels;
pub use flow::{
    Check, CheckBehavior, CheckStyle, Concurrency, ConcurrencyBehavior, Flow, FlowOutput, Label,
    Quota, QuotaBehavior, Sla, SlaBehavior, SlaCommon,
};
pub use input::{DependsOn, Input, InputType};
pub use property::Property;
pub use state::State;
pub(crate) use task::canonical_flow_type;
/// A `WorkingDirectory` block's definition, read by the worker's block claims
/// (spec §5.1), not only by the measurement.
pub(crate) use task::WorkingDirectory;
/// The flowables' typed structs, for `spec::keys`.
pub(crate) use task::{AllowFailure, Dag, DagNode, If, Loop, Parallel, Sequential, Switch};
pub use task::{
    CheckFrequency, LoopUntil, Pause, Retry, RetryBehavior, RetryCommon, Subflow, Task, TaskKind,
};
pub use trigger::{
    Dependency, FetchType, FlowTrigger, FlowTriggerMode, Schedule, Trigger, TriggerKind, Webhook,
    Window,
};
