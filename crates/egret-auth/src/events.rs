//! Events published by the auth/RBAC seam.

/// The role/permission catalog changed (a role was created, deleted, or had its
/// permission set replaced). Consumers that cache role→permission data (e.g. the
/// Cedar PDP template set) rebuild on this. Grant *assignments* do NOT emit it —
/// they are read per-request, not cached.
#[derive(Clone, Debug)]
pub struct RolesChanged;
impl egret_events::Event for RolesChanged {}
