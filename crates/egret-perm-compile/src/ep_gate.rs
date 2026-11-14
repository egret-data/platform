//! SP3d: the per-EP migration-complete gate. A pure verdict over an EP's own
//! rows — true only when no unconvertible operator policy remains (SP3b) and the
//! EP's role/workspace/clearance input is migrated (SP3c). The scoper that reads
//! those inputs lives in `ep_gate_scope`; this module decides nothing from the
//! database. Modeled on `egret_policy::opa_guard::stored_state_refusals`
//! ("empty = ready").

use crate::classify::EpMigrationStatus;
use egret_perm::CompileError;
use sqlx::{PgConnection, Postgres, Transaction};

/// One reason an EP is not yet migration-complete. Order is stable (operator →
/// role → workspace → clearance) so the surfaced list and the tests agree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateRefusal {
    /// Operator (hand-written) policies the EP still carries (SP3b).
    OperatorPolicies { pending: usize, blocked: usize },
    /// `kind='role'` principal refs the EP still names (SP3c-1).
    RolePrincipals { count: i64 },
    /// Implicit workspace grants the EP still carries (SP3c-3).
    WorkspaceGrants { count: i64 },
    /// Gated clearance tags with no `withhold` rule yet (SP3c-2, installation-wide).
    ClearanceTags { tags: Vec<String> },
}

impl GateRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::OperatorPolicies { pending, blocked } => format!(
                "{pending} operator policies await migration and {blocked} cannot be migrated"
            ),
            Self::RolePrincipals { count } => {
                format!("{count} role principal refs remain; migrate them to managed groups")
            }
            Self::WorkspaceGrants { count } => format!(
                "{count} implicit workspace grants remain; migrate them to `ws:<slug> members` groups"
            ),
            Self::ClearanceTags { tags } => format!(
                "{} gated clearance tags have no withhold rule yet: {}",
                tags.len(),
                tags.join(", ")
            ),
        }
    }
}

/// Whether an EP's migration is complete, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct GateVerdict {
    pub complete: bool,
    pub refusals: Vec<GateRefusal>,
}

/// Decide the verdict from the four already-computed inputs. Pure.
pub fn ep_gate(
    mig: &EpMigrationStatus,
    role_refs: i64,
    ws_grants: i64,
    unmigrated_clearance_tags: Vec<String>,
) -> GateVerdict {
    let mut refusals = Vec::new();
    if !mig.complete {
        refusals.push(GateRefusal::OperatorPolicies {
            pending: mig.pending,
            blocked: mig.blocked.len(),
        });
    }
    if role_refs > 0 {
        refusals.push(GateRefusal::RolePrincipals { count: role_refs });
    }
    if ws_grants > 0 {
        refusals.push(GateRefusal::WorkspaceGrants { count: ws_grants });
    }
    if !unmigrated_clearance_tags.is_empty() {
        refusals.push(GateRefusal::ClearanceTags {
            tags: unmigrated_clearance_tags,
        });
    }
    GateVerdict {
        complete: refusals.is_empty(),
        refusals,
    }
}

fn store_err(e: egret_policy::PolicyStoreError) -> CompileError {
    CompileError::Db(e.to_string())
}

/// Read the EP's four gate inputs. Role refs and workspace grants are scoped to
/// the EP's own catalogs/engines; clearance is installation-wide (one global
/// migration writes global withhold rules), so it is not scoped.
async fn gate_inputs(
    conn: &mut PgConnection,
    service_id: i64,
) -> Result<(EpMigrationStatus, i64, i64, Vec<String>), CompileError> {
    let classified = crate::classify::classify_service(conn, service_id)
        .await
        .map_err(store_err)?;
    let mig = crate::classify::ep_migration_status(&classified);

    let catalogs: Vec<uuid::Uuid> = egret_policy::catalogs_for_service_via(conn, service_id)
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|c| c.id)
        .collect();

    let role_refs: i64 = sqlx::query_scalar(
        "SELECT \
           (SELECT count(*) FROM perm.grants g \
              JOIN perm.grant_principals p ON p.grant_id = g.id AND p.kind = 'role' \
             WHERE g.catalog_id = ANY($1)) \
         + (SELECT count(*) FROM perm.engine_grant_principals p \
              JOIN perm.engine_grants eg ON eg.id = p.engine_grant_id \
              JOIN engine_mgmt.engines e ON e.id = eg.engine_id \
             WHERE e.authz_service_id = $2 AND p.kind = 'role') \
         + (SELECT count(*) FROM perm.rule_exemptions x \
              JOIN perm.rules r ON r.id = x.rule_id \
             WHERE x.kind = 'role' \
               AND (r.selector_kind = 'tag' OR r.catalog_id = ANY($1)))",
    )
    .bind(&catalogs)
    .bind(service_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    let ws_grants: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT g.id) FROM perm.grants g \
           LEFT JOIN perm.grant_principals p \
             ON p.grant_id = g.id AND p.kind = 'workspace_members' \
          WHERE g.catalog_id = ANY($1) \
            AND (g.origin = 'workspace_assignment' OR p.grant_id IS NOT NULL)",
    )
    .bind(&catalogs)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    let clearance: Vec<String> = sqlx::query_scalar(
        "SELECT t.key FROM public.classification_policies cp \
           JOIN public.tags t ON t.id = cp.tag_id \
          WHERE (cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0) \
            AND NOT EXISTS ( \
              SELECT 1 FROM perm.rules r \
               WHERE r.kind = 'withhold' AND r.selector_kind = 'tag' AND r.tag_key = t.key) \
          ORDER BY t.key",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;

    Ok((mig, role_refs, ws_grants, clearance))
}

/// The EP's verdict, read on a bare connection.
pub async fn ep_gate_via(
    conn: &mut PgConnection,
    service_id: i64,
) -> Result<GateVerdict, CompileError> {
    let (mig, roles, ws, tags) = gate_inputs(conn, service_id).await?;
    Ok(ep_gate(&mig, roles, ws, tags))
}

/// The EP's verdict, read inside a transaction.
pub async fn ep_gate_via_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<GateVerdict, CompileError> {
    ep_gate_via(tx, service_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::EpMigrationStatus;

    fn done() -> EpMigrationStatus {
        EpMigrationStatus {
            complete: true,
            migrated: 3,
            pending: 0,
            blocked: vec![],
        }
    }

    #[test]
    fn all_inputs_clean_is_complete() {
        let v = ep_gate(&done(), 0, 0, vec![]);
        assert!(v.complete);
        assert!(v.refusals.is_empty());
    }

    #[test]
    fn a_pending_operator_policy_blocks_completion() {
        let mig = EpMigrationStatus {
            complete: false,
            migrated: 1,
            pending: 2,
            blocked: vec![],
        };
        let v = ep_gate(&mig, 0, 0, vec![]);
        assert!(!v.complete);
        assert_eq!(
            v.refusals,
            vec![GateRefusal::OperatorPolicies {
                pending: 2,
                blocked: 0
            }]
        );
    }

    #[test]
    fn role_workspace_and_clearance_each_produce_their_refusal_in_order() {
        let v = ep_gate(&done(), 2, 1, vec!["pii".into(), "phi".into()]);
        assert!(!v.complete);
        assert_eq!(
            v.refusals,
            vec![
                GateRefusal::RolePrincipals { count: 2 },
                GateRefusal::WorkspaceGrants { count: 1 },
                GateRefusal::ClearanceTags {
                    tags: vec!["pii".into(), "phi".into()]
                },
            ]
        );
    }

    #[test]
    fn each_refusal_has_a_nonempty_message() {
        for r in [
            GateRefusal::OperatorPolicies {
                pending: 1,
                blocked: 1,
            },
            GateRefusal::RolePrincipals { count: 1 },
            GateRefusal::WorkspaceGrants { count: 1 },
            GateRefusal::ClearanceTags {
                tags: vec!["pii".into()],
            },
        ] {
            assert!(!r.message().is_empty());
        }
    }

    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn uniq(p: &str) -> String {
        format!("{p}_{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
    }

    #[tokio::test]
    async fn an_ep_with_legacy_inputs_is_incomplete_then_complete_after_migration() {
        let Some(dsn) = egret_test_db::url().await else {
            return;
        };
        let _g = SERIAL.lock().await;
        let db = egret_metastore::Db::connect(&dsn, 5)
            .await
            .expect("connect");
        db.migrate().await.expect("migrate");
        let pool = &db.pool;

        let policy = egret_policy::PolicyStore::new(db.clone());
        egret_policy::seed_starrocks(&policy).await.expect("seed");
        let service = policy
            .create_service(&uniq("sr_epgate"), "starrocks", &serde_json::json!({}))
            .await
            .expect("service");
        let engine_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
             VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
        )
        .bind(uniq("epgate-eng"))
        .bind(service.id)
        .fetch_one(pool)
        .await
        .expect("engine");
        let catalog_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO registry.catalogs (name, kind, config) \
             VALUES ($1,'iceberg','{}'::jsonb) RETURNING id",
        )
        .bind(uniq("epgate_cat"))
        .fetch_one(pool)
        .await
        .expect("catalog");
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .execute(pool)
        .await
        .expect("attach");
        let ws_id: uuid::Uuid = {
            let slug = uniq("epgate-ws").replace('_', "-");
            sqlx::query_scalar(
                "INSERT INTO public.workspaces (slug, name) VALUES ($1,$1) RETURNING id",
            )
            .bind(slug)
            .fetch_one(pool)
            .await
            .expect("workspace")
        };

        // A legacy role-principal grant.
        let role_grant: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO perm.grants (effect, actions, catalog_id) \
             VALUES ('allow'::perm.effect, ARRAY['read'], $1) RETURNING id",
        )
        .bind(catalog_id)
        .fetch_one(pool)
        .await
        .expect("role grant");
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
             VALUES ($1,'role'::perm.principal_kind,$2,'include'::perm.principal_mode)",
        )
        .bind(role_grant)
        .bind(uniq("epgate-role"))
        .execute(pool)
        .await
        .expect("role principal");
        // A legacy implicit workspace grant.
        let ws_grant: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO perm.grants (effect, actions, catalog_id, origin, origin_workspace_id) \
             VALUES ('allow'::perm.effect, ARRAY['read'], $1, 'workspace_assignment', $2) \
             RETURNING id",
        )
        .bind(catalog_id)
        .bind(ws_id)
        .fetch_one(pool)
        .await
        .expect("ws grant");
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
             VALUES ($1,'workspace_members'::perm.principal_kind,$2,'include'::perm.principal_mode)",
        )
        .bind(ws_grant)
        .bind(ws_id.to_string())
        .execute(pool)
        .await
        .expect("ws principal");
        // A gated clearance with no withhold rule yet.
        let tag_key = uniq("epgate-tag");
        let tag_id: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO public.tags (key, name) VALUES ($1,$1) RETURNING id")
                .bind(&tag_key)
                .fetch_one(pool)
                .await
                .expect("tag");
        sqlx::query(
            "INSERT INTO public.classification_policies (tag_id, required_roles, required_groups) \
             VALUES ($1, ARRAY['epgate-clearance-role'], '{}')",
        )
        .bind(tag_id)
        .execute(pool)
        .await
        .expect("gated clearance");

        let mut conn = pool.acquire().await.expect("conn");
        let v = ep_gate_via(&mut conn, service.id).await.expect("gate");
        assert!(!v.complete);
        assert!(v
            .refusals
            .iter()
            .any(|r| matches!(r, GateRefusal::RolePrincipals { count: 1 })));
        assert!(v
            .refusals
            .iter()
            .any(|r| matches!(r, GateRefusal::WorkspaceGrants { count: 1 })));
        assert!(v
            .refusals
            .iter()
            .any(|r| matches!(r, GateRefusal::ClearanceTags { tags } if tags.contains(&tag_key))));

        // Apply each migration's resulting row shape (the real migrations are
        // installation-wide and equivalence-proving; the gate only reads rows).
        sqlx::query(
            "UPDATE perm.grant_principals SET kind = 'group'::perm.principal_kind, ref = 'role:x holders' \
              WHERE grant_id = $1",
        )
        .bind(role_grant)
        .execute(pool)
        .await
        .expect("migrate role");
        sqlx::query("UPDATE perm.grants SET origin = 'workspace_assignment_group' WHERE id = $1")
            .bind(ws_grant)
            .execute(pool)
            .await
            .expect("migrate ws origin");
        sqlx::query(
            "UPDATE perm.grant_principals SET kind = 'group'::perm.principal_kind, ref = 'ws:x members' \
              WHERE grant_id = $1",
        )
        .bind(ws_grant)
        .execute(pool)
        .await
        .expect("migrate ws principal");
        sqlx::query(
            "INSERT INTO perm.rules (name, kind, selector_kind, tag_key) \
             VALUES ($1, 'withhold', 'tag', $2)",
        )
        .bind(format!("clearance:{tag_key}"))
        .bind(&tag_key)
        .execute(pool)
        .await
        .expect("withhold rule");

        let v = ep_gate_via(&mut conn, service.id).await.expect("gate");
        // Only this EP's own refusals matter: other tests' unmigrated gated
        // clearances (installation-wide) may linger, so check ours is gone.
        assert!(
            !v.refusals.iter().any(|r| matches!(
                r,
                GateRefusal::RolePrincipals { .. }
                    | GateRefusal::WorkspaceGrants { .. }
                    | GateRefusal::OperatorPolicies { .. }
            )),
            "{:?}",
            v.refusals
        );
        assert!(!v
            .refusals
            .iter()
            .any(|r| matches!(r, GateRefusal::ClearanceTags { tags } if tags.contains(&tag_key))));
        let mut tx = pool.begin().await.expect("tx");
        let vt = ep_gate_via_tx(&mut tx, service.id).await.expect("gate tx");
        assert_eq!(vt, v);
    }
}
