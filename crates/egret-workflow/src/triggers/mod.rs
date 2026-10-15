//! Reading triggers (W7e spec §3.5, §3.6): a flow's, and the workspace's.
//! Switching them on and off at runtime (§3.3, §3.7) is [`switch`], behind
//! the two `PUT` routes in [`routes`]. A schedule's backfill (W7f §3.4,
//! §3.5) is created, paused, resumed and deleted by [`backfill`], behind the
//! five routes in [`backfill_routes`].
//!
//! # What is read, from where
//!
//! - **The declared triggers** come from the flow's saved draft, parsed with
//!   `spec::parse` -- the source every execution runs. Only the source knows a
//!   `Webhook` or a `Leaf` trigger: neither has a row anywhere.
//! - **Their state** comes from five tables, one statement each, for the flow
//!   ids in scope ([`state::read_state`]): the runtime switch
//!   (`workflow.disabled_triggers`, with the switcher's display name), the
//!   schedule rows (`workflow.triggers`), the listener rows
//!   (`workflow.flow_listeners`), the open `dependsOn` windows
//!   (`workflow.flow_trigger_windows`) and the schedule backfills
//!   (`workflow.trigger_backfills`, with the creator's and pauser's names;
//!   W7f), plus the database's `now()`.
//! - **The last execution** of each trigger is §3.6's statement
//!   ([`state::LAST_EXECUTION_SQL`]), read for one flow's triggers or for one
//!   page of the workspace list, never for the whole match.
//! - **Every field rule** is in [`entry`], which is pure: a flow row, its
//!   parsed draft, the state rows and the caller's `workflow:manage` in,
//!   entries out.
//!
//! # The bound
//!
//! The workspace list reads at most
//! [`MAX_FLOWS`](crate::namespaces::dependencies::MAX_FLOWS) flows (2,000)
//! **after** the namespace and flow filters, which are in SQL; one more is a
//! 422 `triggers_too_large` (D9). The drafts are parsed and the entries built
//! on tokio's blocking pool
//! ([`off_runtime`](crate::namespaces::dependencies::off_runtime)): seconds of
//! CPU at the bound must not pin a runtime worker that also runs the
//! scheduler, the reaper and lease renewal.

pub(crate) mod backfill;
pub(crate) mod backfill_routes;
pub(crate) mod entry;
pub(crate) mod routes;
pub(crate) mod state;
pub(crate) mod switch;

#[cfg(test)]
mod tests {
    /// The listing hands its parse to `off_runtime`, whose closure runs on
    /// another thread than the (single-threaded) runtime awaiting it: inline
    /// execution would report the same thread. Mirrors
    /// `namespaces::dependencies`'s `the_build_runs_off_the_awaiting_worker`,
    /// from this module, through the `pub(crate)` lift (D15).
    #[tokio::test(flavor = "current_thread")]
    async fn the_build_runs_off_the_awaiting_worker() {
        let here = std::thread::current().id();
        let there = crate::namespaces::dependencies::off_runtime(|| std::thread::current().id())
            .await
            .unwrap();
        assert_ne!(here, there);
    }
}
