//! Lowerings: each policy family's decisions, from store rows to neutral
//! [`crate::statement::Statement`]s. A target renders the statements; it
//! decides nothing a lowering already decided.

pub mod grants;
