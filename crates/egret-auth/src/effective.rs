//! Effective-access resolver (AX-3): for a principal, compute what they can do,
//! where, and why — enumerable from the data (Cedar can't enumerate). Read-only.

use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

use crate::roles::db_err; // reuse the existing error mapper

/// The shared principal-name projection used by all three `who_can_access`
/// layers: COALESCE(user email, group name).
///
/// A macro rather than a `const` so callers can `concat!` a WHERE clause onto it
/// and still get a `&'static str`. That matters since sqlx 0.9: `query_as` now
/// takes `SqlSafeStr`, which only `&'static str` implements, so building these
/// with `format!` would require asserting they are injection-free. Composed this
/// way the compiler establishes it instead — there is no runtime string at all.
macro_rules! principal_select {
    () => {
        "SELECT ag.principal_type, ag.principal_id, \
                   COALESCE(u.email::text, g.name) AS principal_name, \
                   r.name AS access, ag.effect \
              FROM access_grants ag \
              JOIN roles r ON r.id = ag.role_id \
              LEFT JOIN users u  ON u.id = ag.principal_id AND ag.principal_type = 'user' \
              LEFT JOIN groups g ON g.id = ag.principal_id AND ag.principal_type = 'group'"
    };
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectiveAccess {
    pub principal: PrincipalInfo,
    pub platform_roles: Vec<RoleSource>,
    pub permissions: Vec<EffectivePermission>,
    pub workspaces: Vec<WorkspaceMembership>,
    pub objects: Vec<ObjectGrant>,
    pub denies: Vec<DenyEntry>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrincipalInfo {
    pub r#type: String,
    pub id: String,
    pub name: Option<String>,
    pub is_platform_admin: bool,
}

#[derive(Debug, Serialize, Clone, utoipa::ToSchema)]
pub struct RoleSource {
    pub name: String,
    pub source: String, // "direct" | "group:{groupName}"
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PermVia {
    pub role: String,
    pub inherited_from: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EffectivePermission {
    pub key: String,
    pub via: Vec<PermVia>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WorkspaceMembership {
    pub workspace_id: String,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub role: String,
    pub source: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObjectGrant {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub workspace_slug: Option<String>,
    pub role: String,
    pub effect: String,
    pub source: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DenyEntry {
    pub scope: String,
    pub target: String,
    pub role: String,
    pub source: String,
}

/// Reverse of `EffectiveAccess`: for one object, every principal who can reach it
/// and why (`who_can_access`). Mirror of the principal→access resolver.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WhoCanAccess {
    pub object: ObjectRef,
    pub entries: Vec<AccessEntry>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ObjectRef {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub workspace_slug: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessEntry {
    pub principal_type: String,
    pub principal_id: String,
    pub principal_name: Option<String>,
    pub access: String,
    pub effect: String,
    pub source: String, // "direct" | "workspace:{slug}" | "platform"
}

#[derive(Clone)]
pub struct AccessResolver {
    db: Db,
}

impl AccessResolver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every grant feeding this principal, with a `source` label. For a user, this
    /// is their own grants plus grants held by any group they belong to; for a
    /// group, only its own grants (egret groups don't nest).
    async fn grant_rows(&self, ptype: &str, pid: Uuid) -> Result<Vec<GrantRow>, egret_core::Error> {
        let sql = "
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role,
                   'direct' AS source
              FROM access_grants ag JOIN roles r ON r.id = ag.role_id
             WHERE ag.principal_type = $1 AND ag.principal_id = $2
               AND (ag.expires_at IS NULL OR ag.expires_at > now())
            UNION ALL
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role,
                   'group:' || g.name AS source
              FROM access_grants ag
              JOIN roles r  ON r.id = ag.role_id
              JOIN groups g ON g.id = ag.principal_id AND ag.principal_type = 'group'
             WHERE $1 = 'user'
               AND ag.principal_id IN (SELECT group_id FROM user_groups WHERE user_id = $2)
               AND (ag.expires_at IS NULL OR ag.expires_at > now())
        ";
        sqlx::query_as::<_, GrantRow>(sql)
            .bind(ptype)
            .bind(pid)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }
    async fn principal_info(
        &self,
        ptype: &str,
        pid: Uuid,
        grants: &[GrantRow],
    ) -> Result<PrincipalInfo, egret_core::Error> {
        let name: Option<String> = match ptype {
            "user" => sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1"),
            "group" => sqlx::query_scalar("SELECT name FROM groups WHERE id = $1"),
            _ => {
                return Err(egret_core::Error::InvalidInput(format!(
                    "bad principal_type: {ptype}"
                )))
            }
        }
        .bind(pid)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let is_platform_admin = grants.iter().any(|g| {
            g.scope_type == "platform" && g.role == "platform_admin" && g.effect == "allow"
        });
        Ok(PrincipalInfo {
            r#type: ptype.to_string(),
            id: pid.to_string(),
            name,
            is_platform_admin,
        })
    }
    fn platform_roles(grants: &[GrantRow]) -> Vec<RoleSource> {
        grants
            .iter()
            .filter(|g| g.scope_type == "platform" && g.effect == "allow")
            .map(|g| RoleSource {
                name: g.role.clone(),
                source: g.source.clone(),
            })
            .collect()
    }

    /// Effective permissions for the granted platform roles, with provenance.
    /// For each granted role, walks its parent chain (role hierarchy) and records,
    /// per permission, which granted role conferred it and (if it came from a
    /// parent) which parent. Skipped for platform_admin (superuser).
    async fn permissions(
        &self,
        platform_roles: &[RoleSource],
    ) -> Result<Vec<EffectivePermission>, egret_core::Error> {
        use std::collections::BTreeMap;
        let mut acc: BTreeMap<String, Vec<PermVia>> = BTreeMap::new();
        for rs in platform_roles {
            if rs.name == "platform_admin" {
                continue; // superuser — enumerated permissions are meaningless
            }
            // (permission, owning_role_name) for this granted role's whole closure.
            let rows: Vec<(String, String)> = sqlx::query_as(
                "WITH RECURSIVE closure AS ( \
                   SELECT id, name, parent_role_id FROM roles WHERE name = $1 AND scope = 'platform' \
                   UNION \
                   SELECT r.id, r.name, r.parent_role_id FROM roles r \
                     JOIN closure c ON r.id = c.parent_role_id \
                 ) \
                 SELECT rp.permission, c.name AS owner FROM closure c \
                   JOIN role_permissions rp ON rp.role_id = c.id",
            )
            .bind(&rs.name)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)?;
            for (perm, owner) in rows {
                let inherited_from = if owner == rs.name { None } else { Some(owner) };
                let via = PermVia {
                    role: rs.name.clone(),
                    inherited_from,
                };
                let entry = acc.entry(perm).or_default();
                // Dedupe: a role reachable by two paths shouldn't duplicate a (role, inherited_from) pair.
                if !entry
                    .iter()
                    .any(|v| v.role == via.role && v.inherited_from == via.inherited_from)
                {
                    entry.push(via);
                }
            }
        }
        Ok(acc
            .into_iter()
            .map(|(key, via)| EffectivePermission { key, via })
            .collect())
    }

    async fn workspaces(
        &self,
        grants: &[GrantRow],
    ) -> Result<Vec<WorkspaceMembership>, egret_core::Error> {
        let mut out = Vec::new();
        for g in grants
            .iter()
            .filter(|g| g.scope_type == "workspace" && g.effect == "allow")
        {
            let Some(ws_id) = g.scope_id else { continue };
            let row: Option<(Uuid, String, String)> =
                sqlx::query_as("SELECT id, slug, name FROM workspaces WHERE id = $1")
                    .bind(ws_id)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(db_err)?;
            if let Some((id, slug, name)) = row {
                out.push(WorkspaceMembership {
                    workspace_id: id.to_string(),
                    workspace_slug: slug,
                    workspace_name: name,
                    role: g.role.clone(),
                    source: g.source.clone(),
                });
            }
        }
        Ok(out)
    }

    /// Map an object `kind` to its `analytics` table name. Returns a `'static`
    /// literal only — never the caller's raw string — so the value spliced into
    /// SQL below can't carry an injection.
    fn object_table(kind: &str) -> Option<&'static str> {
        match kind {
            "dashboard" => Some("dashboards"),
            "chart" => Some("charts"),
            "dataset" => Some("datasets"),
            "notebook" => Some("notebooks"),
            "saved_query" => Some("saved_queries"),
            "folder" => Some("folders"),
            "app" => Some("apps"),
            _ => None,
        }
    }

    async fn objects(&self, grants: &[GrantRow]) -> Result<Vec<ObjectGrant>, egret_core::Error> {
        let mut out = Vec::new();
        for g in grants.iter().filter(|g| g.scope_type == "object") {
            let (Some(kind), Some(id)) = (g.object_type.as_deref(), g.scope_id) else {
                continue;
            };
            let (name, ws_slug) = if let Some(table) = Self::object_table(kind) {
                let sql = format!(
                    "SELECT o.name::text, w.slug FROM analytics.{table} o \
                     JOIN workspaces w ON w.id = o.workspace_id WHERE o.id = $1"
                );
                // AssertSqlSafe: audited. `table` is `object_table`'s `&'static str`
                // from a closed match — the caller's `kind` is matched against, never
                // spliced — and `id` is bound. Nothing here is caller-controlled text.
                sqlx::query_as::<_, (Option<String>, Option<String>)>(sqlx::AssertSqlSafe(sql))
                    .bind(id)
                    .fetch_optional(&self.db.pool)
                    .await
                    .map_err(db_err)?
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };
            out.push(ObjectGrant {
                kind: kind.to_string(),
                id: id.to_string(),
                name,
                workspace_slug: ws_slug,
                role: g.role.clone(),
                effect: g.effect.clone(),
                source: g.source.clone(),
            });
        }
        Ok(out)
    }

    fn denies(objects: &[ObjectGrant], grants: &[GrantRow]) -> Vec<DenyEntry> {
        let mut out: Vec<DenyEntry> = objects
            .iter()
            .filter(|o| o.effect == "deny")
            .map(|o| DenyEntry {
                scope: "object".into(),
                target: format!(
                    "{}:{}",
                    o.kind,
                    o.name.clone().unwrap_or_else(|| o.id.clone())
                ),
                role: o.role.clone(),
                source: o.source.clone(),
            })
            .collect();
        for g in grants
            .iter()
            .filter(|g| g.effect == "deny" && g.scope_type != "object")
        {
            out.push(DenyEntry {
                scope: "role".into(),
                target: g.role.clone(),
                role: g.role.clone(),
                source: g.source.clone(),
            });
        }
        out
    }

    /// Resolve a principal identifier to a UUID: accept a real UUID directly,
    /// else look up a user by email / a group by name. `Ok(None)` if unknown.
    /// Returns `InvalidInput` for an unrecognized `ptype`.
    pub async fn resolve_ident(
        &self,
        ptype: &str,
        ident: &str,
    ) -> Result<Option<Uuid>, egret_core::Error> {
        if let Ok(u) = Uuid::parse_str(ident) {
            return Ok(Some(u));
        }
        let sql = match ptype {
            "user" => "SELECT id FROM users WHERE email = $1",
            "group" => "SELECT id FROM groups WHERE name = $1",
            _ => {
                return Err(egret_core::Error::InvalidInput(format!(
                    "bad principal_type: {ptype}"
                )))
            }
        };
        sqlx::query_scalar(sql)
            .bind(ident)
            .fetch_optional(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// Resolve a principal's full effective access.
    pub async fn resolve(
        &self,
        ptype: &str,
        pid: Uuid,
    ) -> Result<EffectiveAccess, egret_core::Error> {
        let grants = self.grant_rows(ptype, pid).await?;
        let principal = self.principal_info(ptype, pid, &grants).await?;
        let platform_roles = Self::platform_roles(&grants);
        let permissions = if principal.is_platform_admin {
            Vec::new()
        } else {
            self.permissions(&platform_roles).await?
        };
        let workspaces = self.workspaces(&grants).await?;
        let objects = self.objects(&grants).await?;
        let denies = Self::denies(&objects, &grants);
        Ok(EffectiveAccess {
            principal,
            platform_roles,
            permissions,
            workspaces,
            objects,
            denies,
        })
    }

    /// Reverse of `resolve`: for one object, list every principal who can reach it
    /// and why. Three provenance layers, each a parameterized query scoped to THIS
    /// object (direct) or THIS object's OWN workspace (workspace-role holders) —
    /// the object→workspace resolution is the cross-workspace guard — plus global
    /// platform admins. Groups are listed as principals, not expanded to members
    /// (parity with `resolve`). Read-only.
    pub async fn who_can_access(
        &self,
        kind: &str,
        id: Uuid,
    ) -> Result<WhoCanAccess, egret_core::Error> {
        let table = Self::object_table(kind)
            .ok_or_else(|| egret_core::Error::NotFound(format!("unknown object kind: {kind}")))?;

        // Resolve the object → its name + owning workspace (id + slug). The only
        // spliced value is the `'static` table literal from `object_table`.
        let sql = format!(
            "SELECT o.name::text, o.workspace_id, w.slug \
               FROM analytics.{table} o \
               JOIN workspaces w ON w.id = o.workspace_id \
              WHERE o.id = $1"
        );
        // AssertSqlSafe: audited — same reason as `objects` above, the only spliced
        // value is `object_table`'s `'static` literal.
        let (name, ws_id, ws_slug): (Option<String>, Uuid, String) =
            sqlx::query_as(sqlx::AssertSqlSafe(sql))
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(db_err)?
                .ok_or_else(|| egret_core::Error::NotFound(format!("{kind}: {id}")))?;

        // 1. Direct object grants on THIS object.
        let direct: Vec<WhoRow> = sqlx::query_as(concat!(
            principal_select!(),
            " WHERE ag.scope_type = 'object' AND ag.scope_id = $1 \
              AND (ag.expires_at IS NULL OR ag.expires_at > now())"
        ))
        .bind(id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        // 2. Workspace-role holders on THIS object's OWN workspace (cascade).
        let workspace: Vec<WhoRow> = sqlx::query_as(concat!(
            principal_select!(),
            " WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1 \
              AND (ag.expires_at IS NULL OR ag.expires_at > now())"
        ))
        .bind(ws_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        // 3. Platform admins (superusers — reach every object).
        let platform: Vec<WhoRow> = sqlx::query_as(concat!(
            principal_select!(),
            " WHERE ag.scope_type = 'platform' AND r.name = 'platform_admin' AND ag.effect = 'allow' \
                AND (ag.expires_at IS NULL OR ag.expires_at > now())"
        ))
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;

        let ws_source = format!("workspace:{ws_slug}");
        let mut entries: Vec<AccessEntry> = Vec::new();
        entries.extend(
            direct
                .into_iter()
                .map(|r| r.into_entry("direct".to_string())),
        );
        entries.extend(
            workspace
                .into_iter()
                .map(|r| r.into_entry(ws_source.clone())),
        );
        entries.extend(
            platform
                .into_iter()
                .map(|r| r.into_entry("platform".to_string())),
        );
        entries.sort_by(|a, b| {
            (a.source.as_str(), a.principal_name.as_deref())
                .cmp(&(b.source.as_str(), b.principal_name.as_deref()))
        });

        Ok(WhoCanAccess {
            object: ObjectRef {
                kind: kind.to_string(),
                id: id.to_string(),
                name,
                workspace_slug: Some(ws_slug),
            },
            entries,
        })
    }
}

#[derive(sqlx::FromRow, Clone)]
struct GrantRow {
    scope_type: String,
    scope_id: Option<Uuid>,
    object_type: Option<String>,
    effect: String,
    role: String,
    source: String,
}

/// One access-grant row projected for `who_can_access` (before its `source` label
/// is attached by the caller — the label depends on which query produced it).
#[derive(sqlx::FromRow)]
struct WhoRow {
    principal_type: String,
    principal_id: Uuid,
    principal_name: Option<String>,
    access: String,
    effect: String,
}

impl WhoRow {
    fn into_entry(self, source: String) -> AccessEntry {
        AccessEntry {
            principal_type: self.principal_type,
            principal_id: self.principal_id.to_string(),
            principal_name: self.principal_name,
            access: self.access,
            effect: self.effect,
            source,
        }
    }
}
