use uuid::Uuid;

use crate::RefKind;

/// What publish asks the rest of egret about the objects a flow names.
///
/// # Scoping — an implementation MUST honour this
///
/// `name_or_id` comes from a flow's YAML, so it is author-controlled. An
/// implementation returns the object's id and **its own** owning workspace, and
/// never filters by a caller-supplied workspace. The caller compares and refuses
/// a mismatch exactly as it refuses `None` — see `publish`. Splitting it this way
/// is deliberate: it mirrors `DataApiEndpointPort::workspace_of`, and it means a
/// caller that forgets its own check still cannot be told about another
/// workspace's objects, because it never learns anything but a uuid it already
/// has to compare.
///
/// # `RefKind::Subflow`'s `name_or_id` encoding (Task 6 fix rounds 1-2)
///
/// Every other `RefKind` names its object with one string a task's YAML
/// already carries as one field (a `database:`, a `notebook:`, ...). A
/// `core.subflow` task names its target by a **pair** — `namespace` and
/// `flowId` — so `egret-workflow`'s `publish` (`resolve_task`'s
/// `TaskKind::Subflow` arm) encodes both into one `name_or_id` before calling
/// this trait:
///
/// ```text
/// "{namespace}/{flowId}"    e.g. "company.analytics/nightly_rollup"
/// ```
///
/// **`/` is the separator, not `.`, and this is load-bearing.** A flow's own
/// `namespace` is *itself* dot-separated (`"company.analytics"`), so `.`
/// cannot tell a namespace boundary from the namespace/`flowId` boundary —
/// `"a.b.c"` would be ambiguous between `namespace="a", flowId="b.c"` and
/// `namespace="a.b", flowId="c"`. `/` has no such ambiguity **only because
/// neither half of the pair is allowed to contain one** — and that is
/// enforced, not assumed:
///
/// * `Subflow::namespace` is checked against the SAME grammar
///   `Flow::namespace` itself uses (`is_valid_namespace`,
///   `crates/egret-workflow/src/spec/validate.rs:258`,
///   `^[a-z0-9][a-z0-9._-]*`, no `/` in its alphabet), from the
///   `TaskKind::Subflow` arm at `validate.rs:691` (`subflow_namespace_invalid`).
/// * `Subflow::flow_id` is checked against Kestra's own flow `id` pattern
///   (`is_valid_flow_id`, `validate.rs:286`,
///   `^[a-zA-Z0-9][a-zA-Z0-9._-]*`, also no `/` — see that function's doc
///   comment for the `AbstractFlow.java` citation), from the same arm at
///   `validate.rs:712` (`subflow_flow_id_invalid`).
///
/// Both checks run inside `spec::validate`, which `publish` calls (step 3)
/// and refuses to proceed past on any error — **before** it ever reaches the
/// `TaskKind::Subflow` arm that builds one of these strings. So the FIRST `/`
/// in `name_or_id` is guaranteed to be the separator, never a character
/// smuggled in from either half — not because a DIFFERENT field's grammar
/// happens to exclude `/` (an earlier version of this doc cited
/// `Flow::namespace`'s own check, one field over from the one actually
/// encoded — a namespace `"a/b"` on a `Subflow` passed that check while
/// still colliding; fixed here, and both directions are covered by
/// `subflow_namespace_containing_a_slash_is_an_error`/
/// `subflow_flow_id_containing_a_slash_is_an_error` in `validate.rs`'s own
/// test module).
///
/// An implementation MUST still split on the FIRST `/` only (Rust's
/// `str::split_once('/')` does exactly this) and take everything after it as
/// `flowId` **whole** — never split again looking for a second `/`. Now that
/// neither half can contain a `/` at all, a second one could only ever
/// appear by construction error on this crate's side, but the decode rule
/// stays "first split, rest is `flowId`" rather than "assume there is
/// exactly one" — the cheaper invariant to hold.
#[async_trait::async_trait]
pub trait WorkflowRefPort: Send + Sync {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error>;
}
