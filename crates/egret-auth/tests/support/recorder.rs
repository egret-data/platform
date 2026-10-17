//! A recording `GroupMembershipRecompiler` for `egret-auth`'s own tests.
//!
//! The real implementation (`egret_perm_compile::PermGroupRecompiler`) lives in
//! a crate that depends on this one, so it cannot be used here; the compiled
//! policies a membership write must change are asserted in
//! `crates/egret-perm-compile/tests/group_grants.rs`. What THIS crate can pin is
//! the half it owns: that each writer calls the port, with which group names,
//! and that it locks before it recompiles.
//!
//! It is deliberately a test-only type. A no-op implementation in the library
//! would be a recompile somebody could wire by mistake.

// Each test binary that includes this uses a different subset of it.
#![allow(dead_code)]

use std::sync::Mutex;

use egret_auth::groups::GroupMembershipRecompiler;
use sqlx::{Postgres, Transaction};

/// One call the port received, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Lock(Vec<String>),
    Recompile,
}

#[derive(Default)]
pub struct Recorder {
    calls: Mutex<Vec<Call>>,
}

impl Recorder {
    /// Every call so far, with the names of each `Lock` sorted.
    pub fn take(&self) -> Vec<Call> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

#[async_trait::async_trait]
impl GroupMembershipRecompiler for Recorder {
    async fn lock_affected_in_tx(
        &self,
        _tx: &mut Transaction<'static, Postgres>,
        group_names: &[String],
    ) -> egret_core::Result<Vec<i64>> {
        let mut names = group_names.to_vec();
        names.sort();
        self.calls.lock().unwrap().push(Call::Lock(names));
        Ok(Vec::new())
    }

    async fn recompile_in_tx(
        &self,
        _tx: &mut Transaction<'static, Postgres>,
        _service_ids: &[i64],
    ) -> egret_core::Result<()> {
        self.calls.lock().unwrap().push(Call::Recompile);
        Ok(())
    }
}
