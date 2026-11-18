//! EO2: compile the Cedar policy the control-plane PDP runs today from a
//! neutral input. The pure text formatters (`text`) are the single source of
//! Cedar policy strings, shared with `egret-auth`'s PDP; `compile` assembles
//! the same template set `load_inner` builds; `load` reads the legacy
//! control-plane tables into `CompileInputs`. Not wired to the runtime (EO3).
pub mod compile;
pub mod text;
