//! Cedar-based policy decision point.
pub mod audit;
pub mod entities;
pub mod templates;

pub use audit::AuditingPdp;

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, PolicyId, PolicySet, Request, Template,
};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_core::Result;
use egret_events::EventBus;
use egret_metastore::Db;
use tokio::sync::broadcast;

use crate::events::RolesChanged;
use crate::grants::AccessGrantStore;

/// The compiled policy state the PDP authorizes against. Held behind an
/// `ArcSwap` so a background task can atomically swap in a freshly-loaded set
/// when the role/permission catalog changes, without blocking readers.
struct PdpInner {
    templates: PolicySet,
    role_names: HashMap<uuid::Uuid, String>,
}

/// The Cedar-backed decision point. Templates are built from `role_permissions`
/// at construction (B1a) and hot-reloaded on every `RolesChanged` event (B1b).
#[derive(Clone)]
pub struct CedarPdp {
    grants: AccessGrantStore,
    db: Db,
    inner: Arc<ArcSwap<PdpInner>>,
}

impl CedarPdp {
    /// Build the initial template set and spawn a task that rebuilds it on every
    /// `RolesChanged` event from `bus`.
    pub async fn build(db: Db, bus: &EventBus) -> Result<Self> {
        let inner = Arc::new(ArcSwap::from_pointee(Self::load_inner(&db).await?));
        let pdp = Self {
            grants: AccessGrantStore::new(db.clone()),
            db,
            inner,
        };
        pdp.spawn_reload(bus.subscribe::<RolesChanged>());
        Ok(pdp)
    }

    /// The B1a `build` body: load roles + role_permissions, compile one template
    /// per role keyed by `role:{id}`, gate the admin body on scope+name.
    async fn load_inner(db: &Db) -> Result<PdpInner> {
        // Key templates by role_id (UUID), not name: `roles` is UNIQUE(scope,
        // name), so a workspace role can share a name with a platform role. Keying
        // by name collided across scopes → duplicate `add_template` id → `build()`
        // failure (a latent DoS). The role_id is globally unique.
        //
        // Two bulk queries (all roles with their parent pointer; all
        // role_permissions) replace the old per-role N+1 so a reload is O(1)
        // round-trips regardless of role count.
        let roles: Vec<(uuid::Uuid, String, String, Option<uuid::Uuid>)> =
            sqlx::query_as("SELECT id, name, scope, parent_role_id FROM roles")
                .fetch_all(&db.pool)
                .await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let perm_rows: Vec<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT role_id, permission FROM role_permissions")
                .fetch_all(&db.pool)
                .await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;

        // Own permissions and parent pointer per role.
        let mut own: HashMap<uuid::Uuid, Vec<String>> = HashMap::new();
        for (rid, perm) in perm_rows {
            own.entry(rid).or_default().push(perm);
        }
        let parent: HashMap<uuid::Uuid, Option<uuid::Uuid>> =
            roles.iter().map(|(id, _, _, p)| (*id, *p)).collect();

        // Effective permissions = own ∪ ancestors' own, walking parent_role_id.
        // The chain is acyclic (Task 1 app + trigger), but cap defensively. A
        // no-parent role's effective set == its own perms, so the allow-only
        // parity matrix is unchanged.
        let effective = |start: uuid::Uuid| -> Vec<String> {
            let mut acc: std::collections::BTreeSet<String> = Default::default();
            let mut cur = Some(start);
            let mut hops = 0;
            while let Some(c) = cur {
                if let Some(ps) = own.get(&c) {
                    acc.extend(ps.iter().cloned());
                }
                hops += 1;
                if hops > 64 {
                    tracing::error!(role = %start, "role closure exceeded 64 hops; graph may be corrupt");
                    break;
                }
                cur = parent.get(&c).copied().flatten();
            }
            acc.into_iter().collect()
        };

        let mut templates = PolicySet::new();
        let mut role_names = HashMap::new();
        for (id, name, scope, _parent) in &roles {
            let perms = effective(*id);
            // Cedar's `@id(...)` annotation is NOT honored as the template PolicyId
            // by `FromStr`/`parse(None, ..)` (it defaults to "policy0"); set it
            // explicitly so `link_grants` can find the template by `role:<role_id>`.
            let tid: PolicyId = format!("role:{id}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            let tmpl =
                Template::parse(Some(tid), templates::template_for_role(name, scope, &perms))
                    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            templates
                .add_template(tmpl)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            // Forbid counterpart, keyed `role-deny:{id}`, using the SAME effective
            // perms so a deny links a full block of the role's action set. Cedar's
            // forbid-beats-permit makes a linked deny win over any permit.
            let did: PolicyId = format!("role-deny:{id}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            let dtmpl = Template::parse(
                Some(did),
                templates::deny_template_for_role(name, scope, &perms),
            )
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            templates
                .add_template(dtmpl)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            role_names.insert(*id, name.clone());
        }
        // Non-delegatable admin-graph gate: a single static `forbid` covering the
        // authz-graph-mutating actions, exempt only when the request context flags
        // the subject as platform_admin. `forbid` overrides every `permit`, so even
        // a role force-granted one of these actions cannot escalate — a non-admin
        // is always denied. Fail-safe: a missing `is_platform_admin` flag ⇒ the
        // `unless` is false ⇒ the forbid applies ⇒ deny.
        let actions = egret_core::rbac::ADMIN_GRAPH_ACTIONS
            .iter()
            .map(|a| format!("Egret::Action::\"{a}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let forbid_src = format!(
            "forbid(principal, action in [{actions}], resource) \
             unless {{ context has is_platform_admin && context.is_platform_admin }};"
        );
        let forbid: cedar_policy::Policy = forbid_src
            .parse()
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("forbid policy: {e:?}")))?;
        templates
            .add(forbid)
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("add forbid: {e:?}")))?;

        // Classification (clearance) policies (Access P3.3): one static `forbid`
        // per gated tag. A resource carrying the tag key is forbidden unless the
        // request context lists a cleared role (platform_admin always appended).
        // `forbid` beats `permit`, so this is deny-wins. An EMPTY `required_roles`
        // is NO gate → no policy emitted. These are static (no slots), so they
        // ride along in the `templates` clone every request with zero per-request
        // cost. The join resolves the validated tag KEY used in Cedar.
        let class_rows: Vec<(uuid::Uuid, String, Vec<String>)> = sqlx::query_as(
            "SELECT cp.tag_id, t.key, cp.required_roles \
             FROM classification_policies cp JOIN tags t ON t.id = cp.tag_id",
        )
        .fetch_all(&db.pool)
        .await
        .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        for (tag_id, key, required_roles) in class_rows {
            if required_roles.is_empty() {
                continue; // informational tag only — no clearance gate
            }
            let pid: PolicyId = format!("classification:{tag_id}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            // Defense in depth: `required_roles` is validated on write (safe
            // role-name charset), but a malformed row that somehow slipped past
            // validation (direct DB edit, legacy row) must NOT fail the whole PDP
            // reload — that would break authz for everyone and block boot
            // (DoS / stale-authz fail-open). LOG and SKIP just this row instead.
            // Role/admin templates above keep `?`-propagating as before.
            let policy = match cedar_policy::Policy::parse(
                Some(pid),
                templates::classification_forbid(&key, &required_roles),
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        tag_id = %tag_id, tag_key = %key, error = ?e,
                        "skipping malformed classification forbid policy; the tag's \
                         clearance gate is INERT until the policy is corrected"
                    );
                    continue;
                }
            };
            if let Err(e) = templates.add(policy) {
                tracing::warn!(
                    tag_id = %tag_id, tag_key = %key, error = ?e,
                    "skipping classification forbid policy that failed to add to the set"
                );
                continue;
            }
        }
        Ok(PdpInner {
            templates,
            role_names,
        })
    }

    fn spawn_reload(&self, mut rx: broadcast::Receiver<RolesChanged>) {
        let db = self.db.clone();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            // Loop until the last bus sender is dropped (`Closed`), which ends the
            // `while let`. Both a normal event and a `Lagged` gap trigger a full
            // reload — the reload reads the whole catalog, so a missed event is
            // harmless as long as we rebuild afterward.
            while let Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) = rx.recv().await {
                match CedarPdp::load_inner(&db).await {
                    Ok(next) => inner.store(Arc::new(next)),
                    Err(e) => tracing::error!(
                        error = ?e,
                        "PDP reload failed; keeping previous policy set"
                    ),
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl Pdp for CedarPdp {
    async fn authorize(
        &self,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> Result<Decision> {
        // Load the grants relevant to this resource: platform grants always,
        // the resource's workspace grants (so ws roles cascade), and — for an
        // object — object grants on the object itself or its containing folder
        // (so a folder grant cascades to items inside it).
        let (ws, object_ids) = match resource {
            Resource::Platform => (None, Vec::new()),
            Resource::Workspace(w) => (Some(*w), Vec::new()),
            Resource::Object(o) => (
                Some(o.workspace_id),
                match o.folder_id {
                    Some(f) => vec![o.id, f],
                    None => vec![o.id],
                },
            ),
        };
        let grants = self
            .grants
            .grants_for(subject.user_id, ws, &object_ids)
            .await?;
        let inner = self.inner.load();
        let policies =
            entities::link_grants(inner.templates.clone(), subject, &grants, &inner.role_names);
        // The object's classification tag keys, so a classification `forbid` can
        // gate on `resource.tags.contains("<key>")`. ONE extra query, and only for
        // an Object resource; platform/workspace decisions carry no tags.
        let object_tags: Vec<String> = match resource {
            Resource::Object(o) => sqlx::query_scalar(
                "SELECT t.key FROM tag_assignments ta JOIN tags t ON t.id = ta.tag_id \
                 WHERE ta.object_type = $1 AND ta.object_id = $2",
            )
            .bind(&o.kind)
            .bind(o.id)
            .fetch_all(&self.db.pool)
            .await
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?,
            _ => Vec::new(),
        };
        let ents = entities::build_entities(subject, resource, &object_tags);
        let (p, a, r) = entities::request_uids(subject, action, resource);
        // Feed the admin-graph `forbid` its exemption flag: true iff the subject
        // holds a platform-scoped grant on the `platform_admin` role. Computed
        // from the grants already loaded above — no extra query.
        let is_admin = grants.iter().any(|g| {
            g.scope_type == "platform"
                && inner.role_names.get(&g.role_id).map(String::as_str) == Some("platform_admin")
        });
        // The distinct role NAMES the subject holds (from the grants already
        // loaded), fed to the classification `forbid`'s `unless` clearance check.
        // ALLOW-effect grants only: a `deny`-effect grant strips access, so its
        // role name must NOT count as clearance (otherwise a deny grant on the
        // clearance role would perversely satisfy the `unless` and unlock the gate).
        let roles: std::collections::BTreeSet<String> = grants
            .iter()
            .filter(|g| g.effect == "allow")
            .filter_map(|g| inner.role_names.get(&g.role_id).cloned())
            .collect();
        let context = Context::from_json_value(
            serde_json::json!({ "is_platform_admin": is_admin, "roles": roles }),
            None,
        )
        .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("context: {e:?}")))?;
        let req = Request::new(p, a, r, context, None)
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
        let resp = Authorizer::new().is_authorized(&req, &policies, &ents);
        Ok(Decision {
            allow: resp.decision() == CedarDecision::Allow,
            reasons: resp
                .diagnostics()
                .reason()
                .map(|id| id.to_string())
                .collect(),
        })
    }
}
