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
/// # `RefKind::Subflow`'s `name_or_id` encoding (Task 6 fix round 1)
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
/// `namespace="a.b", flowId="c"`. `/` has no such ambiguity: a namespace's own
/// grammar (`egret_workflow::spec::validate`'s `^[a-z0-9][a-z0-9._-]*`) has no
/// `/` in its alphabet at all, and `publish` has already refused any flow
/// whose own `namespace` fails that pattern before it ever builds one of
/// these strings — so the FIRST `/` in `name_or_id` is guaranteed to be the
/// separator, never a character smuggled in from within `namespace` itself.
///
/// `flowId` carries no equivalent restriction in this crate today, but that
/// does not reopen the ambiguity: an implementation MUST split on the FIRST
/// `/` only (Rust's `str::split_once('/')` does exactly this) and take
/// everything after it as `flowId` **whole** — never split again looking for
/// a second `/`. A `flowId` that itself contains a `/` still round-trips
/// correctly under that rule; only splitting from the wrong end, or
/// splitting more than once, would not.
#[async_trait::async_trait]
pub trait WorkflowRefPort: Send + Sync {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error>;
}
