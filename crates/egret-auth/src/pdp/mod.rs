//! Cedar-based policy decision point.
pub mod audit;
pub mod entities;
pub mod grant_source_switch;
pub mod source_switch;
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
use crate::grants::{AccessGrantStore, Grant};
use source_switch::{Divergence, DivergenceKind};

/// The compiled policy state the PDP authorizes against. Held behind an
/// `ArcSwap` so a background task can atomically swap in a freshly-loaded set
/// when the role/permission catalog changes, without blocking readers.
pub(crate) struct PdpInner {
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

    /// EO3: the reload orchestrator. Reads the switch (fail-safe to legacy) and
    /// returns the PdpInner to serve. `shadow`/`compiled` also build the compiled
    /// set and compare it (a production-data canary); `compiled` serves it, failing
    /// safe to legacy if the compiled build errors. Same signature as pre-EO3, so
    /// `build` and `spawn_reload` are unchanged and the default (`legacy`) path is
    /// byte-identical to before.
    async fn load_inner(db: &Db) -> Result<PdpInner> {
        use crate::pdp::source_switch::{read_source, PolicySource};
        match read_source(db).await {
            PolicySource::Legacy => Self::build_legacy(db).await,
            source => {
                // Build legacy too, so we keep comparing (canary) AND have a fail-safe.
                let legacy = Self::build_legacy(db).await?;
                let compiled = Self::build_compiled(db).await;
                Ok(Self::select_served(source, legacy, compiled))
            }
        }
    }

    /// EO3: choose what to serve for a non-legacy state. `shadow` always serves
    /// legacy; `compiled` serves the compiled set, failing safe to legacy (EO3-D5)
    /// when the compiled build errored. Never errors: authz must not break.
    pub(crate) fn select_served(
        source: source_switch::PolicySource,
        legacy: PdpInner,
        compiled: Result<PdpInner>,
    ) -> PdpInner {
        use source_switch::PolicySource;
        match (source, compiled) {
            (PolicySource::Compiled, Ok(compiled)) => {
                Self::log_shadow(&legacy, &compiled);
                compiled
            }
            (PolicySource::Compiled, Err(e)) => {
                tracing::error!(error = ?e, "EO3 compiled: compiled build failed; FAIL-SAFE to legacy");
                legacy
            }
            (_, Ok(compiled)) => {
                Self::log_shadow(&legacy, &compiled);
                legacy
            }
            (_, Err(e)) => {
                tracing::error!(error = ?e, "EO3 shadow: compiled build failed; serving legacy");
                legacy
            }
        }
    }

    /// EO3: emit the shadow comparison result. Empty means equivalent (info); any
    /// divergence yields a warn per entry plus a count, the operator's cutover signal.
    fn log_shadow(legacy: &PdpInner, compiled: &PdpInner) {
        let d = diff(legacy, compiled);
        if d.is_empty() {
            tracing::info!("EO3 shadow: legacy and compiled policy sets are equivalent");
        } else {
            tracing::warn!(divergences = d.len(), "EO3 shadow: policy sets DIVERGE");
            for x in &d {
                tracing::warn!(kind = ?x.kind, id = %x.id, detail = %x.detail, "EO3 shadow divergence");
            }
        }
    }

    fn ids_of(inner: &PdpInner) -> Vec<String> {
        let mut ids: Vec<String> = inner
            .templates
            .templates()
            .map(|t| t.id().to_string())
            .chain(inner.templates.policies().map(|p| p.id().to_string()))
            .collect();
        ids.sort();
        ids
    }

    /// Test-only: ids (templates + policies, sorted) the live reload currently serves.
    #[doc(hidden)]
    pub async fn served_policy_ids_for_test(db: &Db) -> Result<Vec<String>> {
        Ok(Self::ids_of(&Self::load_inner(db).await?))
    }

    /// Test-only: ids (templates + policies, sorted) the LIVE PDP currently holds in
    /// its `ArcSwap` (what `authorize` reads), as opposed to a fresh `load_inner`.
    #[doc(hidden)]
    pub fn live_served_ids_for_test(&self) -> Vec<String> {
        Self::ids_of(&self.inner.load())
    }

    /// Test-only: ids the legacy assembler produces.
    #[doc(hidden)]
    pub async fn legacy_ids_for_test(db: &Db) -> Result<Vec<String>> {
        Ok(Self::ids_of(&Self::build_legacy(db).await?))
    }

    /// Test-only: ids the compiled assembler produces.
    #[doc(hidden)]
    pub async fn compiled_ids_for_test(db: &Db) -> Result<Vec<String>> {
        Ok(Self::ids_of(&Self::build_compiled(db).await?))
    }

    /// Test-only: run the state selector with an injected compiled-build result
    /// (the fail-safe seam: a real `build_compiled` error is hard to force from DB
    /// rows, since both assemblers read the same tables). Returns served ids.
    #[doc(hidden)]
    pub async fn select_served_for_test(
        db: &Db,
        source: source_switch::PolicySource,
        compiled_err: bool,
    ) -> Result<Vec<String>> {
        let legacy = Self::build_legacy(db).await?;
        let compiled = if compiled_err {
            Err(egret_core::Error::Internal(anyhow::anyhow!("injected")))
        } else {
            Self::build_compiled(db).await
        };
        Ok(Self::ids_of(&Self::select_served(source, legacy, compiled)))
    }

    /// The B1a `build` body: load roles + role_permissions, compile one template
    /// per role keyed by `role:{id}`, gate the admin body on scope+name.
    pub(crate) async fn build_legacy(db: &Db) -> Result<PdpInner> {
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
        let forbid: cedar_policy::Policy = egret_cedar::text::admin_graph_forbid()
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
        let class_rows: Vec<(uuid::Uuid, String, Vec<String>, Vec<uuid::Uuid>)> = sqlx::query_as(
            "SELECT cp.tag_id, t.key, cp.required_roles, cp.required_groups \
             FROM classification_policies cp JOIN tags t ON t.id = cp.tag_id",
        )
        .fetch_all(&db.pool)
        .await
        .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        for (tag_id, key, required_roles, required_groups) in class_rows {
            // Clearance = >=1 required ROLE OR >=1 required GROUP (Access P3.4).
            // BOTH empty ⇒ informational tag only, NO clearance gate → emit nothing.
            if required_roles.is_empty() && required_groups.is_empty() {
                continue;
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
                templates::classification_forbid(&key, &required_roles, &required_groups),
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

    /// Test-only: the PDP's compiled template set + role-name map, for the EO2
    /// decision-equivalence differential. Not for runtime use.
    #[doc(hidden)]
    pub async fn compiled_policy_set_for_test(
        db: &Db,
    ) -> Result<(PolicySet, HashMap<uuid::Uuid, String>)> {
        let inner = Self::build_legacy(db).await?;
        Ok((inner.templates, inner.role_names))
    }

    /// EO3: assemble the PDP policy via the egret-cedar compiler (the path EO3
    /// switches to). Reads the SAME legacy tables `build_legacy` reads, through
    /// `egret_cedar::load::load_inputs`, then `egret_cedar::compile::compile`.
    pub(crate) async fn build_compiled(db: &Db) -> Result<PdpInner> {
        let inputs = egret_cedar::load::load_inputs(db)
            .await
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let compiled = egret_cedar::compile::compile(&inputs).map_err(|e| {
            egret_core::Error::Internal(anyhow::anyhow!("egret-cedar compile: {e:?}"))
        })?;
        Ok(PdpInner {
            templates: compiled.policy_set,
            role_names: compiled.role_names,
        })
    }

    /// EO3: a one-shot live equivalence report: build BOTH assemblers from the
    /// current DB and diff them. Empty means safe to switch to `compiled`. Read-only.
    pub async fn report(db: &Db) -> Result<Vec<Divergence>> {
        let legacy = Self::build_legacy(db).await?;
        let compiled = Self::build_compiled(db).await?;
        Ok(diff(&legacy, &compiled))
    }

    /// Test-only: run the request path against an arbitrary template set (the
    /// live PDP's, or the egret-cedar-compiled one), loading grants/tags from the
    /// DB exactly as `authorize`. Isolates the compiled set in the differential.
    #[doc(hidden)]
    pub async fn decide_with_for_test(
        &self,
        templates: &PolicySet,
        role_names: &HashMap<uuid::Uuid, String>,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> Result<Decision> {
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
        self.decide_with(templates, role_names, &grants, subject, action, resource)
            .await
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
        self.decide_with(
            &inner.templates,
            &inner.role_names,
            &grants,
            subject,
            action,
            resource,
        )
        .await
    }
}

impl CedarPdp {
    /// The decision body: link `grants` into `templates`, build the entities and
    /// context, and ask Cedar. Shared by `authorize` (the live template set) and
    /// the EO2 differential (an arbitrary template set).
    async fn decide_with(
        &self,
        templates: &PolicySet,
        role_names: &HashMap<uuid::Uuid, String>,
        grants: &[Grant],
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> Result<Decision> {
        let policies = entities::link_grants(templates.clone(), subject, grants, role_names);
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
        // holds a platform-scoped ALLOW grant on the `platform_admin` role. Computed
        // from the grants already loaded above — no extra query.
        let is_admin = entities::holds_platform_admin(grants, role_names);
        // The distinct role NAMES the subject holds (from the grants already
        // loaded), fed to the classification `forbid`'s `unless` clearance check.
        // ALLOW-effect grants only: a `deny`-effect grant strips access, so its
        // role name must NOT count as clearance (otherwise a deny grant on the
        // clearance role would perversely satisfy the `unless` and unlock the gate).
        let roles: std::collections::BTreeSet<String> = grants
            .iter()
            .filter(|g| g.effect == "allow")
            .filter_map(|g| role_names.get(&g.role_id).cloned())
            .collect();
        // The subject's group ids as strings (Access P3.4 clearance-by-group): the
        // classification `forbid`'s `unless` checks `context.groups.containsAny(..)`
        // against the required-group uuids. Groups come straight off the Subject
        // (OIDC group claims → departments); NO extra query.
        let groups: Vec<String> = subject.group_ids.iter().map(|g| g.0.to_string()).collect();
        let context = Context::from_json_value(
            serde_json::json!({ "is_platform_admin": is_admin, "roles": roles, "groups": groups }),
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

/// EO3: set-equality comparison of the two assemblers' output. Compares every
/// policy and template by `PolicyId -> text` (byte equality; both sides share
/// the EO2-D1 formatters), plus the `role_names` maps. Pure; no DB.
pub(crate) fn diff(legacy: &PdpInner, compiled: &PdpInner) -> Vec<Divergence> {
    fn text_map(ps: &PolicySet) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        for t in ps.templates() {
            m.insert(t.id().to_string(), t.to_string());
        }
        for p in ps.policies() {
            m.insert(p.id().to_string(), p.to_string());
        }
        m
    }
    let l = text_map(&legacy.templates);
    let c = text_map(&compiled.templates);
    let mut out = Vec::new();
    for (id, lt) in &l {
        match c.get(id) {
            None => out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "only in legacy".into(),
            }),
            Some(ct) if ct != lt => out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "text differs".into(),
            }),
            Some(_) => {}
        }
    }
    for id in c.keys() {
        if !l.contains_key(id) {
            out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "only in compiled".into(),
            });
        }
    }
    let mut role_ids: std::collections::BTreeSet<&uuid::Uuid> = Default::default();
    role_ids.extend(legacy.role_names.keys());
    role_ids.extend(compiled.role_names.keys());
    for id in role_ids {
        let ln = legacy.role_names.get(id);
        let cn = compiled.role_names.get(id);
        if ln != cn {
            out.push(Divergence {
                kind: DivergenceKind::RoleName,
                id: id.to_string(),
                detail: format!("legacy={ln:?} compiled={cn:?}"),
            });
        }
    }
    out
}

#[cfg(test)]
mod eo3_diff_tests {
    use super::*;
    use cedar_policy::Policy;

    fn pset(texts: &[(&str, &str)]) -> PolicySet {
        let mut ps = PolicySet::new();
        for (id, body) in texts {
            let pid: PolicyId = id.parse().unwrap();
            ps.add(Policy::parse(Some(pid), *body).unwrap()).unwrap();
        }
        ps
    }

    fn tset(texts: &[(&str, &str)]) -> PolicySet {
        let mut ps = PolicySet::new();
        for (id, body) in texts {
            let pid: PolicyId = id.parse().unwrap();
            ps.add_template(cedar_policy::Template::parse(Some(pid), *body).unwrap())
                .unwrap();
        }
        ps
    }

    #[test]
    fn differing_and_missing_templates_diverge() {
        let legacy = PdpInner {
            templates: tset(&[
                ("t1", "permit(principal == ?principal, action, resource);"),
                ("t2", "permit(principal == ?principal, action, resource);"),
            ]),
            role_names: HashMap::new(),
        };
        let compiled = PdpInner {
            templates: tset(&[("t1", "forbid(principal == ?principal, action, resource);")]),
            role_names: HashMap::new(),
        };
        let d = diff(&legacy, &compiled);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| matches!(x.kind, DivergenceKind::Policy)));
        assert!(d.iter().any(|x| x.id == "t1" && x.detail == "text differs"));
        assert!(d
            .iter()
            .any(|x| x.id == "t2" && x.detail == "only in legacy"));
    }

    #[test]
    fn identical_sets_have_no_divergence() {
        let a = pset(&[("p1", "permit(principal, action, resource);")]);
        let b = pset(&[("p1", "permit(principal, action, resource);")]);
        let legacy = PdpInner {
            templates: a,
            role_names: HashMap::new(),
        };
        let compiled = PdpInner {
            templates: b,
            role_names: HashMap::new(),
        };
        assert!(diff(&legacy, &compiled).is_empty());
    }

    #[test]
    fn missing_and_differing_policies_diverge() {
        let legacy = PdpInner {
            templates: pset(&[
                ("p1", "permit(principal, action, resource);"),
                ("p2", "forbid(principal, action, resource);"),
            ]),
            role_names: HashMap::new(),
        };
        let compiled = PdpInner {
            templates: pset(&[
                ("p1", "forbid(principal, action, resource);"),
                ("p3", "permit(principal, action, resource);"),
            ]),
            role_names: HashMap::new(),
        };
        let d = diff(&legacy, &compiled);
        assert_eq!(d.len(), 3);
        assert!(d.iter().all(|x| matches!(x.kind, DivergenceKind::Policy)));
    }

    #[test]
    fn differing_role_names_diverge() {
        let r = uuid::Uuid::from_u128(1);
        let ps = pset(&[("p1", "permit(principal, action, resource);")]);
        let legacy = PdpInner {
            templates: ps.clone(),
            role_names: HashMap::from([(r, "admin".to_string())]),
        };
        let compiled = PdpInner {
            templates: ps,
            role_names: HashMap::from([(r, "ADMIN".to_string())]),
        };
        let d = diff(&legacy, &compiled);
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0].kind, DivergenceKind::RoleName));
    }

    /// Routing guard: the two inputs are provably distinguishable (different
    /// policy ids and role names), so serving the wrong side fails an assertion.
    #[test]
    fn select_served_routes_by_state() {
        use source_switch::PolicySource;
        let r = uuid::Uuid::from_u128(7);
        let legacy = || PdpInner {
            templates: pset(&[("legacyA", "permit(principal, action, resource);")]),
            role_names: HashMap::from([(r, "legacy-role".to_string())]),
        };
        let compiled = || PdpInner {
            templates: pset(&[("compiledB", "permit(principal, action, resource);")]),
            role_names: HashMap::from([(r, "compiled-role".to_string())]),
        };
        let is_legacy = |i: &PdpInner| {
            i.role_names[&r] == "legacy-role"
                && i.templates
                    .policies()
                    .any(|p| p.id().to_string() == "legacyA")
        };
        let is_compiled = |i: &PdpInner| {
            i.role_names[&r] == "compiled-role"
                && i.templates
                    .policies()
                    .any(|p| p.id().to_string() == "compiledB")
        };
        let err = || Err(egret_core::Error::Internal(anyhow::anyhow!("boom")));

        let out = CedarPdp::select_served(PolicySource::Compiled, legacy(), Ok(compiled()));
        assert!(is_compiled(&out), "compiled+Ok must serve compiled");
        let out = CedarPdp::select_served(PolicySource::Shadow, legacy(), Ok(compiled()));
        assert!(is_legacy(&out), "shadow+Ok must serve legacy");
        let out = CedarPdp::select_served(PolicySource::Compiled, legacy(), err());
        assert!(is_legacy(&out), "compiled+Err must fail safe to legacy");
        let out = CedarPdp::select_served(PolicySource::Shadow, legacy(), err());
        assert!(is_legacy(&out), "shadow+Err must serve legacy");
    }
}
