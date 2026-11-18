//! Read the legacy control-plane tables into `CompileInputs`. The grant *source*
//! migrates to `perm.*` at EO4; EO2 reads it where it lives today.
//!
//! The effective-perm closure is a deliberate parallel copy of `egret-auth`'s
//! `load_inner` (this crate cannot depend on `egret-auth`; that copy is deleted
//! at EO3). Keep the two in step until then.
use std::collections::{BTreeSet, HashMap};

use egret_metastore::Db;
use uuid::Uuid;

use crate::compile::{ClassificationInput, CompileInputs, RoleInput};

/// Load every role (with effective perms) and every classification row. The
/// both-empty classification skip happens in `compile()`, not here.
pub async fn load_inputs(db: &Db) -> anyhow::Result<CompileInputs> {
    let roles: Vec<(Uuid, String, String, Option<Uuid>)> =
        sqlx::query_as("SELECT id, name, scope, parent_role_id FROM roles")
            .fetch_all(&db.pool)
            .await?;
    let perm_rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT role_id, permission FROM role_permissions")
            .fetch_all(&db.pool)
            .await?;

    let mut own: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (rid, perm) in perm_rows {
        own.entry(rid).or_default().push(perm);
    }
    let parent: HashMap<Uuid, Option<Uuid>> = roles.iter().map(|(id, _, _, p)| (*id, *p)).collect();

    // Effective = own ∪ ancestors' own, walking parent_role_id (cap at 64 hops,
    // matching load_inner's defensive bound on an acyclic chain).
    let effective = |start: Uuid| -> Vec<String> {
        let mut acc: BTreeSet<String> = Default::default();
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(c) = cur {
            if let Some(ps) = own.get(&c) {
                acc.extend(ps.iter().cloned());
            }
            hops += 1;
            if hops > 64 {
                break;
            }
            cur = parent.get(&c).copied().flatten();
        }
        acc.into_iter().collect()
    };

    let role_inputs = roles
        .iter()
        .map(|(id, name, scope, _)| RoleInput {
            id: *id,
            name: name.clone(),
            scope: scope.clone(),
            effective_perms: effective(*id),
        })
        .collect();

    let class_rows: Vec<(Uuid, String, Vec<String>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT cp.tag_id, t.key, cp.required_roles, cp.required_groups \
         FROM classification_policies cp JOIN tags t ON t.id = cp.tag_id",
    )
    .fetch_all(&db.pool)
    .await?;
    let classifications = class_rows
        .into_iter()
        .map(
            |(tag_id, key, required_roles, required_groups)| ClassificationInput {
                tag_id,
                key,
                required_roles,
                required_groups,
            },
        )
        .collect();

    Ok(CompileInputs {
        roles: role_inputs,
        classifications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn connect() -> Option<Db> {
        let url = egret_test_db::url().await?;
        let db = Db::connect(&url, 5).await.expect("connect");
        db.migrate().await.expect("migrate");
        Some(db)
    }

    #[tokio::test]
    async fn effective_perms_include_parent_closure() {
        let Some(db) = connect().await else {
            if std::env::var("EGRET_ALLOW_DB_SKIP").is_ok() {
                eprintln!("skip: no DB");
                return;
            }
            panic!("no DB and EGRET_ALLOW_DB_SKIP unset");
        };
        // parent holds dashboard:read; child (parent_role_id=parent) holds
        // dashboard:manage. Child's effective set must contain BOTH.
        let parent: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (name, scope) VALUES ($1, 'workspace') RETURNING id",
        )
        .bind(format!("p-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let child: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (name, scope, parent_role_id) VALUES ($1, 'workspace', $2) RETURNING id",
        )
        .bind(format!("c-{}", Uuid::new_v4().simple()))
        .bind(parent)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        for (rid, perm) in [(parent, "dashboard:read"), (child, "dashboard:manage")] {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(rid)
                .bind(perm)
                .execute(&db.pool)
                .await
                .unwrap();
        }

        let inputs = load_inputs(&db).await.expect("load");
        let child_in = inputs
            .roles
            .iter()
            .find(|r| r.id == child)
            .expect("child present");
        assert!(
            child_in
                .effective_perms
                .iter()
                .any(|p| p == "dashboard:read"),
            "parent perm inherited"
        );
        assert!(
            child_in
                .effective_perms
                .iter()
                .any(|p| p == "dashboard:manage"),
            "own perm present"
        );
    }
}
