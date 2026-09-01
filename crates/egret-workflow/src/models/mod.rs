// kestra: core/src/main/java/io/kestra/core/models/
//! Flow, task, execution, and trigger models. No I/O.

mod input;
mod property;
mod state;
mod task;
pub use input::{DependsOn, Input, InputType};
pub use property::Property;
pub use state::State;
pub use task::{Task, TaskKind};
