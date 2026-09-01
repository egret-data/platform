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
#[async_trait::async_trait]
pub trait WorkflowRefPort: Send + Sync {
    async fn workspace_of(
        &self,
        kind: RefKind,
        name_or_id: &str,
    ) -> Result<Option<(Uuid, Uuid)>, egret_core::Error>;
}
