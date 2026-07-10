//! Cedar-based policy decision point.
pub mod entities;
pub mod templates;

use std::collections::HashMap;

use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, PolicyId, PolicySet, Request, Template,
};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_core::Result;
use egret_metastore::Db;

use crate::grants::AccessGrantStore;

/// The Cedar-backed decision point. Templates are built from `role_permissions`
/// once at construction (B1a); hot-reload on `role_permissions` changes is B2.
#[derive(Clone)]
pub struct CedarPdp {
    grants: AccessGrantStore,
    templates: PolicySet,
    role_names: HashMap<uuid::Uuid, String>,
}

impl CedarPdp {
    pub async fn build(db: Db) -> Result<Self> {
        let roles: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT id, name FROM roles")
            .fetch_all(&db.pool)
            .await
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let mut templates = PolicySet::new();
        let mut role_names = HashMap::new();
        for (id, name) in roles {
            let perms: Vec<(String,)> =
                sqlx::query_as("SELECT permission FROM role_permissions WHERE role_id = $1")
                    .bind(id)
                    .fetch_all(&db.pool)
                    .await
                    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
            let perms: Vec<String> = perms.into_iter().map(|(p,)| p).collect();
            // Cedar's `@id(...)` annotation is NOT honored as the template PolicyId
            // by `FromStr`/`parse(None, ..)` (it defaults to "policy0"); set it
            // explicitly so `link_grants` can find the template by `role:<name>`.
            let tid: PolicyId = format!("role:{name}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            let tmpl = Template::parse(Some(tid), templates::template_for_role(&name, &perms))
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            templates
                .add_template(tmpl)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            role_names.insert(id, name);
        }
        Ok(Self {
            grants: AccessGrantStore::new(db),
            templates,
            role_names,
        })
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
        let ws = match resource {
            Resource::Workspace(w) => Some(*w),
            Resource::Platform => None,
        };
        let grants = self.grants.grants_for(subject.user_id, ws).await?;
        let policies =
            entities::link_grants(self.templates.clone(), subject, &grants, &self.role_names);
        let ents = entities::build_entities(subject, resource);
        let (p, a, r) = entities::request_uids(subject, action, resource);
        let req = Request::new(p, a, r, Context::empty(), None)
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
