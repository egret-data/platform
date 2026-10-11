// kestra: core/src/main/java/io/kestra/core/models/
//! Flow, task, execution, and trigger models. No I/O.

mod execution;
mod flow;
mod input;
mod property;
mod state;
mod task;
mod trigger;
mod wire;
pub use execution::{
    Execution, ExecutionKind, StateHistory, StateTransition, TaskRun, TaskRunAttempt,
};
pub use flow::{
    Check, CheckBehavior, CheckStyle, Concurrency, ConcurrencyBehavior, Flow, FlowOutput, Label,
    Quota, QuotaBehavior, Sla, SlaBehavior, SlaCommon,
};
pub use input::{DependsOn, Input, InputType};
pub use property::Property;
pub use state::State;
pub(crate) use task::canonical_flow_type;
/// The flowables' typed structs, for `spec::schema`'s property measurement.
#[cfg(test)]
pub(crate) use task::{
    AllowFailure, Dag, If, Loop, Parallel, Sequential, Switch, WorkingDirectory,
};
pub use task::{
    CheckFrequency, LoopUntil, Pause, Retry, RetryBehavior, RetryCommon, Subflow, Task, TaskKind,
};
pub use trigger::{
    Dependency, FetchType, FlowTrigger, FlowTriggerMode, Schedule, Trigger, TriggerKind, Webhook,
    Window,
};
