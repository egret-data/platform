//! The golden corpus' fixtures (governance sub-project 1, Task 1). Each one
//! builds a model inside one transaction, captures what its profiles compile
//! to and serve, rolls back, and compares with (or blesses) its golden.
//!
//! **Every id is fixed.** A row's id is derived from its role name
//! ([`uid`]), never minted by the database: policy names carry grant, rule
//! and tag ids, the desired set is ordered by name and the served set by
//! insertion, so a random id would reorder the output from run to run.
//! Profiles are the exception (`policy.authz_services.id` is a sequence and
//! `create_service` commits); their ids never reach the output, their fixed
//! names do. The test database is private to the process
//! (`egret_test_db`), so the committed profiles never meet another run's.

use std::future::Future;
use std::pin::Pin;

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Principal, PrincipalKind, PrincipalSet, RuleKind, Scope, Selector,
};
use serde_json::{json, Value};
use sha2::Digest;
use uuid::Uuid;

use crate::Names;

pub type Tx<'c> = sqlx::Transaction<'c, sqlx::Postgres>;
type Run = for<'a> fn(&'a Db) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub struct Fixture {
    pub name: &'static str,
    pub run: Run,
}

pub const ALL: &[Fixture] = &[
    Fixture {
        name: "G01",
        run: g01_trino_allow_read_catalog,
    },
    Fixture {
        name: "G02",
        run: g02_trino_deny_schema_with_exception,
    },
    Fixture {
        name: "G03",
        run: g03_starrocks_allow_write_with_exception,
    },
    Fixture {
        name: "G04",
        run: g04_workspace_grants,
    },
    Fixture {
        name: "G05",
        run: g05_group_role_public_principals,
    },
    Fixture {
        name: "G06",
        run: g06_path_masks,
    },
    Fixture {
        name: "G07",
        run: g07_path_row_filters,
    },
    Fixture {
        name: "G08",
        run: g08_tag_mask_trino_and_starrocks,
    },
    Fixture {
        name: "G09",
        run: g09_tag_row_filter,
    },
    Fixture {
        name: "G10",
        run: g10_missing_tag_service,
    },
    Fixture {
        name: "G11",
        run: g11_trino_clearance,
    },
    Fixture {
        name: "G12",
        run: g12_starrocks_clearance,
    },
    Fixture {
        name: "G13",
        run: g13_engine_grants,
    },
    Fixture {
        name: "G14",
        run: g14_trino_metadata_tables,
    },
    Fixture {
        name: "G15",
        run: g15_starrocks_hidden_columns,
    },
    Fixture {
        name: "G16",
        run: g16_hand_written_and_infra,
    },
    Fixture {
        name: "G17",
        run: g17_several_engines,
    },
    Fixture {
        name: "G18",
        run: g18_held_rename,
    },
    Fixture {
        name: "G19",
        run: g19_ceilings,
    },
    Fixture {
        name: "G20",
        run: g20_overlap_refusal,
    },
    Fixture {
        name: "G21",
        run: g21_large_profile,
    },
];

// ── ids and names ───────────────────────────────────────────────────────────

/// A fixed id for `role`, registered under it.
pub fn uid(names: &mut Names, role: &str) -> Uuid {
    let digest = sha2::Sha256::digest(format!("egret-golden:{role}").as_bytes());
    let id = Uuid::from_slice(&digest[..16]).expect("16 bytes");
    names.id(role, id)
}

fn slug(role: &str) -> String {
    role.replace('.', "_")
}

// ── principals and scopes ───────────────────────────────────────────────────

fn principal(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}
pub fn user_ref(r: &str) -> Principal {
    principal(PrincipalKind::User, r)
}
pub fn group_ref(r: &str) -> Principal {
    principal(PrincipalKind::Group, r)
}
pub fn role_ref(r: &str) -> Principal {
    principal(PrincipalKind::Role, r)
}
pub fn members_ref(workspace: Uuid) -> Principal {
    principal(PrincipalKind::WorkspaceMembers, &workspace.to_string())
}
pub fn set(include: &[Principal], exclude: &[Principal]) -> PrincipalSet {
    PrincipalSet {
        include: include.to_vec(),
        exclude: exclude.to_vec(),
    }
}
pub fn scope(
    catalog: Uuid,
    schema: Option<&str>,
    table: Option<&str>,
    column: Option<&str>,
) -> Scope {
    Scope {
        catalog_id: catalog,
        schema: schema.map(str::to_string),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    }
}
pub fn mask(mask_type: &str, value_expr: Option<&str>) -> RuleKind {
    RuleKind::Mask {
        mask_type: mask_type.to_string(),
        value_expr: value_expr.map(str::to_string),
    }
}
pub fn row_filter(expr: &str) -> RuleKind {
    RuleKind::RowFilter {
        filter_expr: expr.to_string(),
    }
}
pub fn by_tag(key: &str) -> Selector {
    Selector::Tag {
        key: key.to_string(),
    }
}

// ── committed setup ─────────────────────────────────────────────────────────

pub async fn seed_service_defs(db: &Db) {
    let policy = egret_policy::PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    // The stand-in for the embedded catalog's tables (`table_columns` reads
    // them); DDL only, the rows go in each fixture's transaction.
    for ddl in [
        "CREATE SCHEMA IF NOT EXISTS lakekeeper",
        "CREATE TABLE IF NOT EXISTS lakekeeper.tabular ( \
             tabular_id uuid NOT NULL, warehouse_id uuid NOT NULL, name text NOT NULL, \
             typ text NOT NULL, metadata_location text, deleted_at timestamptz, \
             tabular_namespace_name text[] NOT NULL, PRIMARY KEY (warehouse_id, tabular_id))",
        "CREATE TABLE IF NOT EXISTS lakekeeper.table_schema ( \
             warehouse_id uuid NOT NULL, table_id uuid NOT NULL, schema_id integer NOT NULL, \
             schema jsonb NOT NULL, PRIMARY KEY (warehouse_id, table_id, schema_id))",
        "CREATE TABLE IF NOT EXISTS lakekeeper.table_current_schema ( \
             warehouse_id uuid NOT NULL, table_id uuid NOT NULL, schema_id integer NOT NULL, \
             PRIMARY KEY (warehouse_id, table_id))",
    ] {
        sqlx::query(ddl).execute(&db.pool).await.expect("stub ddl");
    }
}

/// A profile (committed: `create_service` uses its own pool) and, for Trino, its
/// tag service. Fixed names so the output is stable; one fixture, one prefix.
pub async fn profile(
    db: &Db,
    prefix: &str,
    engine: &str,
    tag: bool,
    names: &mut Names,
) -> (i64, Option<i64>, String) {
    let policy = egret_policy::PolicyStore::new(db.clone());
    let name = format!("gold_{prefix}_{engine}");
    names.text(&format!("{prefix}.{engine}"), &name);
    let tag_id = if tag {
        let tag_name = format!("{name}_tag");
        let t = policy
            .create_service(&tag_name, "tag", &json!({}))
            .await
            .expect("tag svc");
        Some(t.id)
    } else {
        None
    };
    let cfg = if tag {
        json!({ "tagService": format!("{name}_tag") })
    } else {
        json!({})
    };
    let p = policy
        .create_service(&name, engine, &cfg)
        .await
        .expect("profile");
    (p.id, tag_id, name)
}

/// A profile under an explicit name and config (the `tagService` edge cases).
pub async fn profile_with_config(
    db: &Db,
    role: &str,
    engine: &str,
    config: Value,
    names: &mut Names,
) -> i64 {
    let policy = egret_policy::PolicyStore::new(db.clone());
    let name = format!("gold_{}", slug(role));
    names.text(role, &name);
    policy
        .create_service(&name, engine, &config)
        .await
        .expect("profile")
        .id
}

// ── builders on the fixture's transaction ───────────────────────────────────

/// A workspace, a storage connection and an egret warehouse in it.
pub async fn warehouse(tx: &mut Tx<'_>, role: &str, names: &mut Names) -> Uuid {
    let ws = uid(names, &format!("{role}.wh_workspace"));
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(format!("gold-{}-wh", slug(role)))
        .bind("Golden warehouse workspace")
        .execute(&mut **tx)
        .await
        .expect("workspace");
    let sc = uid(names, &format!("{role}.storage"));
    sqlx::query(
        "INSERT INTO storage.storage_connections (id, name, region, bucket, access_key) \
         VALUES ($1, $2, 'us-east-1', 'egret-golden', 'AKIATEST')",
    )
    .bind(sc)
    .bind(format!("gold-{}-sc", slug(role)))
    .execute(&mut **tx)
    .await
    .expect("storage connection");
    let wh = uid(names, &format!("{role}.warehouse"));
    sqlx::query(
        "INSERT INTO catalog.warehouses (id, name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(wh)
    .bind(format!("gold_{}_wh", slug(role)))
    .bind(ws)
    .bind(sc)
    .execute(&mut **tx)
    .await
    .expect("warehouse");
    wh
}

/// An engine bound to `profile`. A StarRocks engine is pinned to `image`
/// (egret's default when `None`), which decides its Ranger plugin version.
///
/// Neither spec qualifies for infrastructure policies (governance SP2a, D2):
/// the StarRocks spec has no delegation block, and the Trino one is in
/// `open` mode. A golden profile therefore holds no infra row unless its
/// fixture binds a qualifying engine with [`engine_bound_spec`] (G16).
pub async fn engine_bound(
    tx: &mut Tx<'_>,
    role: &str,
    profile: i64,
    engine: &str,
    image: Option<&str>,
    names: &mut Names,
) -> Uuid {
    let spec = match engine {
        "starrocks" => json!({"starrocksConfig": {
            "feImage": image.unwrap_or(egret_perm::DEFAULT_ENGINE_IMAGES.starrocks_fe_image)
        }}),
        _ => json!({"trinoConfig": {"delegationMode": "open"}}),
    };
    engine_bound_spec(tx, role, profile, engine, spec, names).await
}

/// An engine of type `engine` bound to `profile` with the stored `spec` as
/// given.
pub async fn engine_bound_spec(
    tx: &mut Tx<'_>,
    role: &str,
    profile: i64,
    engine: &str,
    spec: Value,
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (id, name, type, namespace, spec, authz_service_id) \
         VALUES ($1, $2, $3, 'egret', $4, $5)",
    )
    .bind(id)
    .bind(format!("gold-{}", role.replace(['.', '_'], "-")))
    .bind(engine)
    .bind(spec)
    .bind(profile)
    .execute(&mut **tx)
    .await
    .expect("engine");
    id
}

/// A registry catalog `gold_{role}` over `warehouse`, attached to `engines`.
pub async fn catalog(
    tx: &mut Tx<'_>,
    role: &str,
    warehouse: Uuid,
    engines: &[Uuid],
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    sqlx::query(
        "INSERT INTO registry.catalogs (id, name, kind, config, warehouse_id) \
         VALUES ($1, $2, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $3::uuid::text)), $3)",
    )
    .bind(id)
    .bind(format!("gold_{}", slug(role)))
    .bind(warehouse)
    .execute(&mut **tx)
    .await
    .expect("registry catalog");
    for e in engines {
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) \
             VALUES ($1, $2)",
        )
        .bind(e)
        .bind(id)
        .execute(&mut **tx)
        .await
        .expect("attach catalog");
    }
    id
}

/// One engine per profile and one catalog `{prefix}.cat` attached to them all.
pub async fn catalog_attached(
    tx: &mut Tx<'_>,
    prefix: &str,
    warehouse: Uuid,
    profiles: &[(i64, &str)],
    names: &mut Names,
) -> Uuid {
    let mut engines = Vec::new();
    for (i, (pid, engine)) in profiles.iter().enumerate() {
        engines.push(
            engine_bound(
                tx,
                &format!("{prefix}.engine{i}"),
                *pid,
                engine,
                None,
                names,
            )
            .await,
        );
    }
    catalog(tx, &format!("{prefix}.cat"), warehouse, &engines, names).await
}

/// A user, `username` `None` for one the compiler has to mint for.
pub async fn user(tx: &mut Tx<'_>, role: &str, username: Option<&str>, names: &mut Names) -> Uuid {
    let id = uid(names, role);
    if let Some(u) = username {
        names.text(&format!("{role}.username"), u);
    }
    sqlx::query(
        "INSERT INTO users (id, provider, email, name, username) VALUES ($1, 'local', $2, $3, $4)",
    )
    .bind(id)
    .bind(format!("{}@gold.test", slug(role)))
    .bind("Golden User")
    .bind(username)
    .execute(&mut **tx)
    .await
    .expect("user");
    id
}

/// A platform role named `name`.
pub async fn platform_role(tx: &mut Tx<'_>, name: &str, names: &mut Names) -> Uuid {
    let id = uid(names, &format!("role.{name}"));
    sqlx::query("INSERT INTO roles (id, scope, name) VALUES ($1, 'platform', $2)")
        .bind(id)
        .bind(name)
        .execute(&mut **tx)
        .await
        .expect("role");
    id
}

/// `user` holds the platform role `role_name` (an existing role by name).
pub async fn role_held(tx: &mut Tx<'_>, user: Uuid, role_name: &str) {
    let n = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'platform', NULL FROM roles r \
          WHERE r.scope = 'platform' AND r.name = $2",
    )
    .bind(user)
    .bind(role_name)
    .execute(&mut **tx)
    .await
    .expect("role grant")
    .rows_affected();
    assert_eq!(n, 1, "role {role_name} must exist");
}

pub async fn group(tx: &mut Tx<'_>, name: &str, names: &mut Names) -> Uuid {
    let id = uid(names, &format!("group.{name}"));
    sqlx::query("INSERT INTO groups (id, name, source) VALUES ($1, $2, 'local')")
        .bind(id)
        .bind(name)
        .execute(&mut **tx)
        .await
        .expect("group");
    id
}

pub async fn group_member(tx: &mut Tx<'_>, user: Uuid, group: Uuid) {
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user)
        .bind(group)
        .execute(&mut **tx)
        .await
        .expect("group member");
}

pub async fn workspace(tx: &mut Tx<'_>, role: &str, names: &mut Names) -> Uuid {
    let id = uid(names, role);
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("gold-{}", role.replace(['.', '_'], "-")))
        .bind("Golden workspace")
        .execute(&mut **tx)
        .await
        .expect("workspace");
    id
}

pub async fn workspace_member(tx: &mut Tx<'_>, user: Uuid, workspace: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(user)
    .bind(workspace)
    .execute(&mut **tx)
    .await
    .expect("workspace member");
}

/// A tag whose key and name are both `key`.
pub async fn tag(tx: &mut Tx<'_>, key: &str, names: &mut Names) -> Uuid {
    let id = uid(names, key);
    sqlx::query("INSERT INTO tags (id, key, name) VALUES ($1, $2, $2)")
        .bind(id)
        .bind(key)
        .execute(&mut **tx)
        .await
        .expect("tag");
    id
}

pub async fn tag_column(
    tx: &mut Tx<'_>,
    tag: Uuid,
    wh: Uuid,
    schema: &str,
    table: &str,
    column: &str,
) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tag)
    .bind(wh)
    .bind(schema)
    .bind(table)
    .bind(column)
    .execute(&mut **tx)
    .await
    .expect("tag column");
}

/// A whole-table attachment (`column_name = ''`).
pub async fn tag_table(tx: &mut Tx<'_>, tag: Uuid, wh: Uuid, schema: &str, table: &str) {
    tag_column(tx, tag, wh, schema, table, "").await;
}

/// Tag `count` columns `col_1..` of one table.
pub async fn tag_columns_bulk(
    tx: &mut Tx<'_>,
    tag: Uuid,
    wh: Uuid,
    schema: &str,
    table: &str,
    count: i64,
) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, \
         column_name) SELECT $1, $2, $3, $4, 'col_' || gs::text FROM generate_series(1, $5) gs",
    )
    .bind(tag)
    .bind(wh)
    .bind(schema)
    .bind(table)
    .bind(count)
    .execute(&mut **tx)
    .await
    .expect("bulk tag columns");
}

/// A classification clearance on `tag`.
pub async fn gate(tx: &mut Tx<'_>, tag: Uuid, roles: &[&str], groups: &[Uuid]) {
    let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, $3)",
    )
    .bind(tag)
    .bind(&roles)
    .bind(groups)
    .execute(&mut **tx)
    .await
    .expect("classification policy");
}

const GRANT_PRINCIPAL_SQL: &str = "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
     VALUES ($1, $2::perm.principal_kind, $3, $4::perm.principal_mode)";
const RULE_EXEMPTION_SQL: &str = "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
     VALUES ($1, $2::perm.principal_kind, $3, $4::perm.principal_mode)";

/// `perm.grant_principals` or `perm.rule_exemptions` rows for `set`.
async fn principals(tx: &mut Tx<'_>, sql: &'static str, id: Uuid, set: &PrincipalSet) {
    for (mode, list) in [("include", &set.include), ("exclude", &set.exclude)] {
        for p in list {
            sqlx::query(sql)
                .bind(id)
                .bind(p.kind.as_str())
                .bind(&p.r#ref)
                .bind(mode)
                .execute(&mut **tx)
                .await
                .expect("principal");
        }
    }
}

/// A grant under a fixed id: `egret_perm::store::create_grant_in_tx`'s SQL,
/// with the id supplied rather than minted.
pub async fn grant(
    tx: &mut Tx<'_>,
    role: &str,
    effect: Effect,
    actions: &[Action],
    scope: Scope,
    who: PrincipalSet,
    names: &mut Names,
) -> Uuid {
    grant_with_origin(tx, role, effect, actions, scope, who, None, names).await
}

#[allow(clippy::too_many_arguments)]
pub async fn grant_with_origin(
    tx: &mut Tx<'_>,
    role: &str,
    effect: Effect,
    actions: &[Action],
    scope: Scope,
    who: PrincipalSet,
    assignment_of: Option<Uuid>,
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    let actions: Vec<&str> = actions.iter().map(|a| a.as_str()).collect();
    let origin = if assignment_of.is_some() {
        "workspace_assignment"
    } else {
        "explicit"
    };
    sqlx::query(
        "INSERT INTO perm.grants \
           (id, effect, actions, catalog_id, schema_name, table_name, column_name, \
            origin, origin_workspace_id) \
         VALUES ($1, $2::perm.effect, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(effect.as_str())
    .bind(&actions)
    .bind(scope.catalog_id)
    .bind(&scope.schema)
    .bind(&scope.table)
    .bind(&scope.column)
    .bind(origin)
    .bind(assignment_of)
    .execute(&mut **tx)
    .await
    .expect("grant");
    principals(tx, GRANT_PRINCIPAL_SQL, id, &who).await;
    id
}

/// A rule named `role` under a fixed id: `create_rule_in_tx`'s SQL.
pub async fn rule(
    tx: &mut Tx<'_>,
    role: &str,
    kind: RuleKind,
    selector: Selector,
    exempt: PrincipalSet,
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    let (k, mask_type, value_expr, filter_expr) = match &kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => ("mask", Some(mask_type.clone()), value_expr.clone(), None),
        RuleKind::RowFilter { filter_expr } => {
            ("row_filter", None, None, Some(filter_expr.clone()))
        }
    };
    let (sk, tag_key, s) = match &selector {
        Selector::Tag { key } => ("tag", Some(key.clone()), None),
        Selector::Path(s) => ("path", None, Some(s.clone())),
    };
    sqlx::query(
        "INSERT INTO perm.rules \
           (id, name, kind, mask_type, value_expr, filter_expr, selector_kind, \
            tag_key, catalog_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, $3::perm.rule_kind, $4, $5, $6, $7::perm.selector_kind, \
                 $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(role)
    .bind(k)
    .bind(mask_type)
    .bind(value_expr)
    .bind(filter_expr)
    .bind(sk)
    .bind(tag_key)
    .bind(s.as_ref().map(|s| s.catalog_id))
    .bind(s.as_ref().and_then(|s| s.schema.clone()))
    .bind(s.as_ref().and_then(|s| s.table.clone()))
    .bind(s.as_ref().and_then(|s| s.column.clone()))
    .execute(&mut **tx)
    .await
    .expect("rule");
    principals(tx, RULE_EXEMPTION_SQL, id, &exempt).await;
    id
}

/// An engine grant under a fixed id.
#[allow(clippy::too_many_arguments)]
pub async fn engine_grant(
    tx: &mut Tx<'_>,
    role: &str,
    engine: Uuid,
    capability: &str,
    values: &[&str],
    target: Option<Uuid>,
    who: &[Principal],
    names: &mut Names,
) -> Uuid {
    let id = uid(names, role);
    let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    sqlx::query(
        "INSERT INTO perm.engine_grants (id, engine_id, capability, resource_values, target_user_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(engine)
    .bind(capability)
    .bind(&values)
    .bind(target)
    .execute(&mut **tx)
    .await
    .expect("engine grant");
    for p in who {
        sqlx::query(
            "INSERT INTO perm.engine_grant_principals (engine_grant_id, kind, ref) \
             VALUES ($1, $2::perm.principal_kind, $3)",
        )
        .bind(id)
        .bind(p.kind.as_str())
        .bind(&p.r#ref)
        .execute(&mut **tx)
        .await
        .expect("engine grant principal");
    }
    id
}

/// A stored policy written the way its owner writes it
/// (`insert_policy_owned_by_via`: the same guards), switched off when
/// `enabled` is false.
pub async fn stored_policy(
    tx: &mut Tx<'_>,
    profile: i64,
    name: &str,
    body: Value,
    owner: &str,
    enabled: bool,
) {
    let id = egret_policy::store::insert_policy_owned_by_via(tx, profile, name, &body, owner)
        .await
        .expect("stored policy");
    if !enabled {
        sqlx::query("UPDATE policy.authz_policies SET enabled = false WHERE id = $1")
            .bind(id)
            .execute(&mut **tx)
            .await
            .expect("disable");
    }
}

/// The hand-written operator policy shapes.
pub fn level(v: &str) -> Value {
    json!({"values": [v], "isExcludes": false, "isRecursive": false})
}

pub fn hand_mask(
    name: &str,
    service: &str,
    service_type: &str,
    resources: Value,
    who: (&[&str], &[&str]),
    mask_type: &str,
) -> Value {
    json!({
        "name": name, "service": service, "serviceType": service_type,
        "policyType": 1, "isEnabled": true, "resources": resources,
        "dataMaskPolicyItems": [{
            "users": who.0, "groups": who.1, "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": mask_type}
        }],
        "policyItems": [], "denyPolicyItems": []
    })
}

pub fn hand_deny(
    name: &str,
    service: &str,
    service_type: &str,
    resources: Value,
    users: &[&str],
) -> Value {
    json!({
        "name": name, "service": service, "serviceType": service_type,
        "policyType": 0, "isEnabled": true, "resources": resources,
        "policyItems": [],
        "denyPolicyItems": [{
            "users": users, "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}], "delegateAdmin": false
        }],
        "allowExceptions": [], "denyExceptions": [], "isDenyAllElse": false
    })
}

/// A hand-written allow on `trinouser` etc.: the delegator's shape
/// (`egret_engine_mgmt::delegator_policy`, which this crate cannot depend on).
fn delegator_shape(
    service: &str,
    name: &str,
    resources: Value,
    users: Value,
    access: &str,
) -> Value {
    json!({
        "name": name, "service": service, "serviceType": "trino",
        "policyType": 0, "isEnabled": true, "resources": resources,
        "policyItems": [{
            "users": users, "groups": [], "roles": [],
            "accesses": [{ "type": access, "isAllowed": true }], "delegateAdmin": false
        }],
        "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": [],
        "isDenyAllElse": false, "policyPriority": 0
    })
}

/// The delegator's impersonation policy and the per-user baseline, as engine
/// apply stores them (`managed_by = 'infra'`).
pub async fn delegator_policies(tx: &mut Tx<'_>, profile: i64, service: &str) {
    use egret_policy::infra_names::{
        EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
        SELF_IMPERSONATE_POLICY_NAME,
    };
    let delegator = egret_core::engine_principals::DELEGATOR_PRINCIPAL;
    let rows = [
        (
            IMPERSONATE_POLICY_NAME,
            delegator_shape(
                service,
                IMPERSONATE_POLICY_NAME,
                json!({"trinouser": level("*")}),
                json!([delegator]),
                "impersonate",
            ),
        ),
        (
            SELF_IMPERSONATE_POLICY_NAME,
            delegator_shape(
                service,
                SELF_IMPERSONATE_POLICY_NAME,
                json!({"trinouser": level("{USER}")}),
                json!(["{USER}"]),
                "impersonate",
            ),
        ),
        (
            EXECUTE_QUERY_POLICY_NAME,
            delegator_shape(
                service,
                EXECUTE_QUERY_POLICY_NAME,
                json!({"queryid": level("*")}),
                json!(["{USER}"]),
                "execute",
            ),
        ),
        (
            QUERY_TIMEOUT_POLICY_NAME,
            delegator_shape(
                service,
                QUERY_TIMEOUT_POLICY_NAME,
                json!({"systemproperty": level("query_max_run_time")}),
                json!(["{USER}"]),
                "alter",
            ),
        ),
    ];
    for (name, body) in rows {
        stored_policy(tx, profile, name, body, "infra", true).await;
    }
}

// ── the embedded catalog's stand-in ─────────────────────────────────────────

/// Give `warehouse` a Lakekeeper warehouse id.
pub async fn lk_link(tx: &mut Tx<'_>, warehouse: Uuid, role: &str, names: &mut Names) -> Uuid {
    let lk = uid(names, role);
    sqlx::query("UPDATE catalog.warehouses SET lakekeeper_warehouse_id = $2 WHERE id = $1")
        .bind(warehouse)
        .bind(lk.to_string())
        .execute(&mut **tx)
        .await
        .expect("link warehouse");
    lk
}

async fn lk_tabular(
    tx: &mut Tx<'_>,
    lk: Uuid,
    schema: &str,
    name: &str,
    typ: &str,
    names: &mut Names,
) -> Uuid {
    let id = uid(names, &format!("lk.{lk}.{schema}.{name}"));
    sqlx::query(
        "INSERT INTO lakekeeper.tabular (tabular_id, warehouse_id, name, typ, metadata_location, \
         tabular_namespace_name) VALUES ($1, $2, $3, $4, 's3://stub/metadata.json', $5)",
    )
    .bind(id)
    .bind(lk)
    .bind(name)
    .bind(typ)
    .bind(vec![schema.to_string()])
    .execute(&mut **tx)
    .await
    .expect("stub tabular");
    id
}

/// An active table whose current schema has `columns`.
pub async fn lk_table(
    tx: &mut Tx<'_>,
    lk: Uuid,
    schema: &str,
    name: &str,
    columns: &[&str],
    names: &mut Names,
) {
    let id = lk_tabular(tx, lk, schema, name, "table", names).await;
    let fields: Vec<_> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"id": i + 1, "name": c, "type": "string", "required": false}))
        .collect();
    sqlx::query(
        "INSERT INTO lakekeeper.table_schema (warehouse_id, table_id, schema_id, schema) \
         VALUES ($1, $2, 0, $3)",
    )
    .bind(lk)
    .bind(id)
    .bind(json!({"type": "struct", "schema-id": 0, "fields": fields}))
    .execute(&mut **tx)
    .await
    .expect("stub schema");
    sqlx::query(
        "INSERT INTO lakekeeper.table_current_schema (warehouse_id, table_id, schema_id) \
         VALUES ($1, $2, 0)",
    )
    .bind(lk)
    .bind(id)
    .execute(&mut **tx)
    .await
    .expect("stub current schema");
}

pub async fn lk_view(tx: &mut Tx<'_>, lk: Uuid, schema: &str, name: &str, names: &mut Names) {
    lk_tabular(tx, lk, schema, name, "view", names).await;
}

// ── the fixtures ────────────────────────────────────────────────────────────

type Fut<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// G01: Trino `allow read` on a catalog, two users.
pub fn g01_trino_allow_read_catalog(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, _) = profile(db, "g01", "trino", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g01", &mut names).await;
        let cat = catalog_attached(&mut tx, "g01", wh, &[(tr, "trino")], &mut names).await;
        user(&mut tx, "g01.alice", Some("g01_alice"), &mut names).await;
        user(&mut tx, "g01.bob", Some("g01_bob"), &mut names).await;
        grant(
            &mut tx,
            "g01.read",
            Effect::Allow,
            &[Action::Read],
            scope(cat, None, None, None),
            set(&[user_ref("g01_alice"), user_ref("g01_bob")], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G01_trino_allow_read_catalog", captured, &names);
    })
}

/// G02: Trino `deny read` on a schema with an exclude user (`denyExceptions`).
pub fn g02_trino_deny_schema_with_exception(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, _) = profile(db, "g02", "trino", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g02", &mut names).await;
        let cat = catalog_attached(&mut tx, "g02", wh, &[(tr, "trino")], &mut names).await;
        user(&mut tx, "g02.carol", Some("g02_carol"), &mut names).await;
        grant(
            &mut tx,
            "g02.deny",
            Effect::Deny,
            &[Action::Read],
            scope(cat, Some("hr"), None, None),
            set(&[group_ref("public")], &[user_ref("g02_carol")]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G02_trino_deny_schema_with_exception", captured, &names);
    })
}

/// G03: StarRocks `allow write` on a table with an exclude (`allowExceptions`).
pub fn g03_starrocks_allow_write_with_exception(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, _) = profile(db, "g03", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g03", &mut names).await;
        let cat = catalog_attached(&mut tx, "g03", wh, &[(sr, "starrocks")], &mut names).await;
        user(&mut tx, "g03.dave", Some("g03_dave"), &mut names).await;
        user(&mut tx, "g03.erin", Some("g03_erin"), &mut names).await;
        grant(
            &mut tx,
            "g03.write",
            Effect::Allow,
            &[Action::Write],
            scope(cat, Some("sales"), Some("orders"), None),
            set(
                &[user_ref("g03_dave"), user_ref("g03_erin")],
                &[user_ref("g03_erin")],
            ),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G03_starrocks_allow_write_with_exception", captured, &names);
    })
}

/// G04: the implicit workspace grant with one member who has no username
/// (minted), plus an empty workspace (empty-audience skip).
pub fn g04_workspace_grants(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, _) = profile(db, "g04", "trino", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g04", &mut names).await;
        let cat = catalog_attached(&mut tx, "g04", wh, &[(tr, "trino")], &mut names).await;
        let full = workspace(&mut tx, "g04.ws_full", &mut names).await;
        let empty = workspace(&mut tx, "g04.ws_empty", &mut names).await;
        let named = user(&mut tx, "g04.ann", Some("g04_ann"), &mut names).await;
        let unnamed = user(&mut tx, "g04.newcomer", None, &mut names).await;
        workspace_member(&mut tx, named, full).await;
        workspace_member(&mut tx, unnamed, full).await;
        for (role, ws) in [("g04.assign_full", full), ("g04.assign_empty", empty)] {
            grant_with_origin(
                &mut tx,
                role,
                Effect::Allow,
                &[Action::Read],
                scope(cat, None, None, None),
                set(&[members_ref(ws)], &[]),
                Some(ws),
                &mut names,
            )
            .await;
        }
        let captured = crate::capture(&mut tx, &[(tr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G04_workspace_grants", captured, &names);
    })
}

/// G05: group and role principals, plus `public`, plus a plain-user list
/// (captured first, alone, so the member expansion takes its no-expansion
/// early return) naming one person twice over (directly and through a group).
pub fn g05_group_role_public_principals(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, _) = profile(db, "g05", "trino", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g05", &mut names).await;
        let cat = catalog_attached(&mut tx, "g05", wh, &[(tr, "trino")], &mut names).await;
        let bob = user(&mut tx, "g05.bob", Some("g05_bob"), &mut names).await;
        user(&mut tx, "g05.carol", Some("g05_carol"), &mut names).await;
        let dan = user(&mut tx, "g05.dan", Some("g05_dan"), &mut names).await;
        let erin = user(&mut tx, "g05.erin", Some("g05_erin"), &mut names).await;
        // Phase 1: users only, so nothing is expanded.
        grant(
            &mut tx,
            "g05.plain",
            Effect::Allow,
            &[Action::Read],
            scope(cat, None, None, None),
            set(&[user_ref("g05_carol"), user_ref("g05_bob")], &[]),
            &mut names,
        )
        .await;
        let plain = crate::capture(&mut tx, &[(tr, None)]).await;
        // Phase 2: a group (bob is also named directly), a role, and public.
        let analysts = group(&mut tx, "g05_analysts", &mut names).await;
        group_member(&mut tx, bob, analysts).await;
        group_member(&mut tx, dan, analysts).await;
        platform_role(&mut tx, "g05_auditor", &mut names).await;
        role_held(&mut tx, erin, "g05_auditor").await;
        grant(
            &mut tx,
            "g05.mixed",
            Effect::Allow,
            &[Action::Read, Action::Write],
            scope(cat, Some("sales"), None, None),
            set(
                &[
                    group_ref("g05_analysts"),
                    role_ref("g05_auditor"),
                    user_ref("g05_bob"),
                ],
                &[],
            ),
            &mut names,
        )
        .await;
        grant(
            &mut tx,
            "g05.public_deny",
            Effect::Deny,
            &[Action::Write],
            scope(cat, Some("sales"), Some("ledger"), None),
            set(&[group_ref("public")], &[role_ref("g05_auditor")]),
            &mut names,
        )
        .await;
        let mixed = crate::capture(&mut tx, &[(tr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless(
            "G05_group_role_public_principals",
            json!({"plain": plain, "mixed": mixed}),
            &names,
        );
    })
}

/// G06: path masks (column) on Trino and StarRocks, `CUSTOM` with `{col}`,
/// `MASK_NULL`, exemptions by user and group.
pub fn g06_path_masks(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g06", "trino", true, &mut names).await;
        let (sr, _, _) = profile(db, "g06", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g06", &mut names).await;
        let cat = catalog_attached(
            &mut tx,
            "g06",
            wh,
            &[(tr, "trino"), (sr, "starrocks")],
            &mut names,
        )
        .await;
        user(&mut tx, "g06.alice", Some("g06_alice"), &mut names).await;
        let bob = user(&mut tx, "g06.bob", Some("g06_bob"), &mut names).await;
        let admins = group(&mut tx, "g06_admins", &mut names).await;
        group_member(&mut tx, bob, admins).await;
        rule(
            &mut tx,
            "g06.custom",
            mask("CUSTOM", Some("concat('x-', {col})")),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), Some("email"))),
            set(&[user_ref("g06_alice"), group_ref("g06_admins")], &[]),
            &mut names,
        )
        .await;
        rule(
            &mut tx,
            "g06.null",
            mask("MASK_NULL", None),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), Some("phone"))),
            set(&[], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G06_path_masks", captured, &names);
    })
}

/// G07: path row filters on a table and a schema (wildcard levels).
pub fn g07_path_row_filters(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g07", "trino", true, &mut names).await;
        let (sr, _, _) = profile(db, "g07", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g07", &mut names).await;
        let cat = catalog_attached(
            &mut tx,
            "g07",
            wh,
            &[(tr, "trino"), (sr, "starrocks")],
            &mut names,
        )
        .await;
        user(&mut tx, "g07.alice", Some("g07_alice"), &mut names).await;
        rule(
            &mut tx,
            "g07.table",
            row_filter("region = 'EU'"),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), None)),
            set(&[user_ref("g07_alice")], &[]),
            &mut names,
        )
        .await;
        rule(
            &mut tx,
            "g07.schema",
            row_filter("tenant_id = 7"),
            Selector::Path(scope(cat, Some("hr"), None, None)),
            set(&[], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G07_path_row_filters", captured, &names);
    })
}

/// G08: a tag mask on Trino (tag service) and StarRocks (per-column,
/// whole-table attachment).
pub fn g08_tag_mask_trino_and_starrocks(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g08", "trino", true, &mut names).await;
        let (sr, _, _) = profile(db, "g08", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g08", &mut names).await;
        catalog_attached(
            &mut tx,
            "g08",
            wh,
            &[(tr, "trino"), (sr, "starrocks")],
            &mut names,
        )
        .await;
        let lk = lk_link(&mut tx, wh, "g08.lk", &mut names).await;
        lk_table(
            &mut tx,
            lk,
            "sales",
            "customers",
            &["id", "name"],
            &mut names,
        )
        .await;
        let pii = tag(&mut tx, "g08.pii", &mut names).await;
        tag_column(&mut tx, pii, wh, "sales", "orders", "email").await;
        tag_column(&mut tx, pii, wh, "sales", "orders", "phone").await;
        tag_table(&mut tx, pii, wh, "sales", "customers").await;
        user(&mut tx, "g08.alice", Some("g08_alice"), &mut names).await;
        rule(
            &mut tx,
            "g08.mask",
            mask("MASK_HASH", None),
            by_tag("g08.pii"),
            set(&[user_ref("g08_alice")], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G08_tag_mask_trino_and_starrocks", captured, &names);
    })
}

/// G09: a tag row filter: Trino column-only plus table-level attachments
/// (KI-287), StarRocks per-table.
pub fn g09_tag_row_filter(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g09", "trino", true, &mut names).await;
        let (sr, _, _) = profile(db, "g09", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g09", &mut names).await;
        catalog_attached(
            &mut tx,
            "g09",
            wh,
            &[(tr, "trino"), (sr, "starrocks")],
            &mut names,
        )
        .await;
        let region = tag(&mut tx, "g09.region", &mut names).await;
        tag_column(&mut tx, region, wh, "sales", "orders", "region").await;
        tag_table(&mut tx, region, wh, "sales", "customers").await;
        user(&mut tx, "g09.alice", Some("g09_alice"), &mut names).await;
        rule(
            &mut tx,
            "g09.rf",
            row_filter("region = 'EU'"),
            by_tag("g09.region"),
            set(&[user_ref("g09_alice")], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G09_tag_row_filter", captured, &names);
    })
}

/// G10: a Trino profile without `tagService` (tag rule skip, clearance skip),
/// a dangling tag key, and a `tagService` naming nothing.
pub fn g10_missing_tag_service(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (bare, _, _) = profile(db, "g10", "trino", false, &mut names).await;
        let nowhere = profile_with_config(db, "g10.nowhere", "trino", json!({}), &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        // The service API refuses a `tagService` naming nothing; the state is
        // what a deleted tag profile leaves behind, so it is written directly.
        sqlx::query("UPDATE policy.authz_services SET config_jsonb = $2 WHERE id = $1")
            .bind(nowhere)
            .bind(json!({"tagService": "gold_g10_no_such_service"}))
            .execute(&mut *tx)
            .await
            .expect("dangling tagService");
        let wh = warehouse(&mut tx, "g10", &mut names).await;
        catalog_attached(
            &mut tx,
            "g10",
            wh,
            &[(bare, "trino"), (nowhere, "trino")],
            &mut names,
        )
        .await;
        let pii = tag(&mut tx, "g10.pii", &mut names).await;
        tag_column(&mut tx, pii, wh, "sales", "orders", "email").await;
        platform_role(&mut tx, "g10_cleared", &mut names).await;
        let alice = user(&mut tx, "g10.alice", Some("g10_alice"), &mut names).await;
        role_held(&mut tx, alice, "g10_cleared").await;
        gate(&mut tx, pii, &["g10_cleared"], &[]).await;
        rule(
            &mut tx,
            "g10.mask",
            mask("MASK_NULL", None),
            by_tag("g10.pii"),
            set(&[], &[]),
            &mut names,
        )
        .await;
        // A rule whose tag key names no tag: `perm.rules.tag_key`'s foreign key
        // makes it unwritable by any egret path, so it is written with the
        // triggers that enforce it switched off for this one insert (the
        // transaction rolls back), to keep the compiler's skip in the corpus.
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await
            .expect("replica role");
        rule(
            &mut tx,
            "g10.dangling",
            mask("MASK_NULL", None),
            by_tag("g10.gone"),
            set(&[], &[]),
            &mut names,
        )
        .await;
        sqlx::query("SET LOCAL session_replication_role = origin")
            .execute(&mut *tx)
            .await
            .expect("origin role");
        let captured = crate::capture(&mut tx, &[(bare, None), (nowhere, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G10_missing_tag_service", captured, &names);
    })
}

/// G11: Trino clearance by role, groups-only clearance (skip), and a role with
/// no holders.
pub fn g11_trino_clearance(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g11", "trino", true, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g11", &mut names).await;
        catalog_attached(&mut tx, "g11", wh, &[(tr, "trino")], &mut names).await;
        let by_role = tag(&mut tx, "g11.secret", &mut names).await;
        let by_group = tag(&mut tx, "g11.grouponly", &mut names).await;
        let nobody = tag(&mut tx, "g11.unheld", &mut names).await;
        tag_column(&mut tx, by_role, wh, "sales", "orders", "ssn").await;
        tag_column(&mut tx, nobody, wh, "hr", "staff", "salary").await;
        platform_role(&mut tx, "g11_cleared", &mut names).await;
        platform_role(&mut tx, "g11_unheld", &mut names).await;
        let alice = user(&mut tx, "g11.alice", Some("g11_alice"), &mut names).await;
        let bob = user(&mut tx, "g11.bob", None, &mut names).await;
        role_held(&mut tx, alice, "g11_cleared").await;
        role_held(&mut tx, bob, "g11_cleared").await;
        let stewards = group(&mut tx, "g11_stewards", &mut names).await;
        gate(&mut tx, by_role, &["g11_cleared"], &[]).await;
        gate(&mut tx, by_group, &[], &[stewards]).await;
        gate(&mut tx, nobody, &["g11_unheld"], &[]).await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G11_trino_clearance", captured, &names);
    })
}

/// G12: StarRocks clearance (column, whole known table, view, unknown table),
/// MV deny, a view-guard fold with another rule mask and a hand-written mask.
pub fn g12_starrocks_clearance(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, sr_name) = profile(db, "g12", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g12", &mut names).await;
        let cat = catalog_attached(&mut tx, "g12", wh, &[(sr, "starrocks")], &mut names).await;
        let lk = lk_link(&mut tx, wh, "g12.lk", &mut names).await;
        lk_table(
            &mut tx,
            lk,
            "sales",
            "orders",
            &["id", "ssn", "email"],
            &mut names,
        )
        .await;
        lk_table(
            &mut tx,
            lk,
            "sales",
            "customers",
            &["id", "name"],
            &mut names,
        )
        .await;
        lk_view(&mut tx, lk, "sales", "v_orders", &mut names).await;
        let secret = tag(&mut tx, "g12.secret", &mut names).await;
        tag_column(&mut tx, secret, wh, "sales", "orders", "ssn").await;
        tag_table(&mut tx, secret, wh, "sales", "customers").await;
        tag_table(&mut tx, secret, wh, "sales", "v_orders").await;
        tag_table(&mut tx, secret, wh, "sales", "ghost").await;
        platform_role(&mut tx, "g12_cleared", &mut names).await;
        let alice = user(&mut tx, "g12.alice", Some("g12_alice"), &mut names).await;
        let bob = user(&mut tx, "g12.bob", Some("g12_bob"), &mut names).await;
        role_held(&mut tx, alice, "g12_cleared").await;
        role_held(&mut tx, bob, "g12_cleared").await;
        gate(&mut tx, secret, &["g12_cleared"], &[]).await;
        // Another rule's mask on the classified column, exempting alice: the
        // fold gives bob the rule's mask and alice none.
        rule(
            &mut tx,
            "g12.hash",
            mask("MASK_HASH", None),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), Some("ssn"))),
            set(&[user_ref("g12_alice")], &[]),
            &mut names,
        )
        .await;
        let cat_name = "gold_g12_cat";
        stored_policy(&mut tx, sr, "g12 hand mask",
            hand_mask("g12 hand mask", &sr_name, "starrocks",
                json!({"catalog": level(cat_name), "database": level("sales"), "table": level("customers"), "column": level("name")}),
                (&[], &["public"]), "MASK"),
            "operator", true).await;
        let captured = crate::capture(&mut tx, &[(sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G12_starrocks_clearance", captured, &names);
    })
}

/// G13: engine grants: Trino session property, impersonation (ok, reserved
/// `root` skip, deleted target), StarRocks blacklist, empty-principal skip.
pub fn g13_engine_grants(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, _) = profile(db, "g13", "trino", false, &mut names).await;
        let (sr, _, _) = profile(db, "g13", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g13", &mut names).await;
        let tr_engine =
            engine_bound(&mut tx, "g13.trino_engine", tr, "trino", None, &mut names).await;
        let sr_engine =
            engine_bound(&mut tx, "g13.sr_engine", sr, "starrocks", None, &mut names).await;
        catalog(&mut tx, "g13.cat", wh, &[tr_engine, sr_engine], &mut names).await;
        user(&mut tx, "g13.alice", Some("g13_alice"), &mut names).await;
        let bob = user(&mut tx, "g13.bob", Some("g13_bob"), &mut names).await;
        let root = user(&mut tx, "g13.root", Some("root"), &mut names).await;
        // `root` is left as written: replaced, it would rewrite every `root`
        // substring in the output.
        names.0.remove("root");
        let gone = user(&mut tx, "g13.gone", None, &mut names).await;
        platform_role(&mut tx, "g13_empty", &mut names).await;
        let alice = [user_ref("g13_alice")];
        engine_grant(
            &mut tx,
            "g13.session",
            tr_engine,
            "trino.set_system_session_property",
            &["query_max_memory"],
            None,
            &alice,
            &mut names,
        )
        .await;
        engine_grant(
            &mut tx,
            "g13.become_bob",
            tr_engine,
            "trino.impersonate_user",
            &[],
            Some(bob),
            &alice,
            &mut names,
        )
        .await;
        engine_grant(
            &mut tx,
            "g13.become_root",
            tr_engine,
            "trino.impersonate_user",
            &[],
            Some(root),
            &alice,
            &mut names,
        )
        .await;
        engine_grant(
            &mut tx,
            "g13.become_gone",
            tr_engine,
            "trino.impersonate_user",
            &[],
            Some(gone),
            &alice,
            &mut names,
        )
        .await;
        engine_grant(
            &mut tx,
            "g13.blacklist",
            sr_engine,
            "starrocks.manage_blacklist",
            &[],
            None,
            &alice,
            &mut names,
        )
        .await;
        engine_grant(
            &mut tx,
            "g13.nobody",
            sr_engine,
            "starrocks.use_global_function",
            &["my_udf"],
            None,
            &[role_ref("g13_empty")],
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, None), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G13_engine_grants", captured, &names);
    })
}

/// G14: metadata-table denies on Trino (#821): resource mask, row filter, a
/// deny grant, a tag mask, a clearance, and a hand-written restricting policy.
pub fn g14_trino_metadata_tables(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, tr_name) = profile(db, "g14", "trino", true, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g14", &mut names).await;
        let cat = catalog_attached(&mut tx, "g14", wh, &[(tr, "trino")], &mut names).await;
        user(&mut tx, "g14.alice", Some("g14_alice"), &mut names).await;
        let bob = user(&mut tx, "g14.bob", Some("g14_bob"), &mut names).await;
        rule(
            &mut tx,
            "g14.mask",
            mask("MASK_NULL", None),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), Some("email"))),
            set(&[user_ref("g14_alice")], &[]),
            &mut names,
        )
        .await;
        rule(
            &mut tx,
            "g14.filter",
            row_filter("region = 'EU'"),
            Selector::Path(scope(cat, Some("sales"), Some("customers"), None)),
            set(&[], &[]),
            &mut names,
        )
        .await;
        grant(
            &mut tx,
            "g14.deny",
            Effect::Deny,
            &[Action::Read],
            scope(cat, Some("sales"), Some("payments"), None),
            set(&[user_ref("g14_alice")], &[]),
            &mut names,
        )
        .await;
        let pii = tag(&mut tx, "g14.pii", &mut names).await;
        tag_column(&mut tx, pii, wh, "sales", "leads", "phone").await;
        rule(
            &mut tx,
            "g14.tagmask",
            mask("MASK_HASH", None),
            by_tag("g14.pii"),
            set(&[], &[]),
            &mut names,
        )
        .await;
        let secret = tag(&mut tx, "g14.secret", &mut names).await;
        tag_column(&mut tx, secret, wh, "hr", "staff", "salary").await;
        platform_role(&mut tx, "g14_cleared", &mut names).await;
        role_held(&mut tx, bob, "g14_cleared").await;
        gate(&mut tx, secret, &["g14_cleared"], &[]).await;
        stored_policy(&mut tx, tr, "g14 hand deny",
            hand_deny("g14 hand deny", &tr_name, "trino",
                json!({"catalog": level("gold_g14_cat"), "schema": level("finance"), "table": level("budget"), "column": level("*")}),
                &["g14_alice"]),
            "operator", true).await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G14_trino_metadata_tables", captured, &names);
    })
}

/// G15: hidden columns on StarRocks: an exact mask, a wildcard-table mask, and
/// a hand-written mask.
pub fn g15_starrocks_hidden_columns(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, sr_name) = profile(db, "g15", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g15", &mut names).await;
        let cat = catalog_attached(&mut tx, "g15", wh, &[(sr, "starrocks")], &mut names).await;
        user(&mut tx, "g15.alice", Some("g15_alice"), &mut names).await;
        rule(
            &mut tx,
            "g15.exact",
            mask("MASK_NULL", None),
            Selector::Path(scope(cat, Some("sales"), Some("orders"), Some("email"))),
            set(&[user_ref("g15_alice")], &[]),
            &mut names,
        )
        .await;
        stored_policy(&mut tx, sr, "g15 wildcard mask",
            hand_mask("g15 wildcard mask", &sr_name, "starrocks",
                json!({"catalog": level("gold_g15_cat"), "database": level("hr"), "table": level("*"), "column": level("ssn")}),
                (&[], &["public"]), "MASK_NULL"),
            "operator", true).await;
        stored_policy(&mut tx, sr, "g15 hand mask",
            hand_mask("g15 hand mask", &sr_name, "starrocks",
                json!({"catalog": level("gold_g15_cat"), "database": level("sales"), "table": level("customers"), "column": level("name")}),
                (&["g15_alice"], &[]), "MASK"),
            "operator", true).await;
        let captured = crate::capture(&mut tx, &[(sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G15_starrocks_hidden_columns", captured, &names);
    })
}

/// G16: hand-written plus infra: `seed_default_trino_policies`, egret-admin (an
/// engine-admin user), delegator policies, one operator policy, one disabled
/// operator policy.
pub fn g16_hand_written_and_infra(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, _, tr_name) = profile(db, "g16", "trino", false, &mut names).await;
        // Committed, as the route does; only this fixture's profile holds them.
        egret_policy::PolicyStore::new(db.clone())
            .seed_default_trino_policies(tr, &tr_name, "g16_admins")
            .await
            .expect("seed defaults");
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g16", &mut names).await;
        let cat = catalog_attached(&mut tx, "g16", wh, &[(tr, "trino")], &mut names).await;
        let admin = user(&mut tx, "g16.admin", Some("g16_admin"), &mut names).await;
        role_held(&mut tx, admin, "platform_admin").await;
        user(&mut tx, "g16.alice", Some("g16_alice"), &mut names).await;
        // The admin set starts stale (one name that is no admin); the
        // recompile's chokepoint converges it to the engine admins.
        for (name, body) in egret_policy::default_policies::admin_all_access_bodies(
            "trino",
            &tr_name,
            &["g16_stale".to_string()],
        )
        .expect("egret-admin bodies")
        {
            stored_policy(&mut tx, tr, &name, body, "infra", true).await;
        }
        delegator_policies(&mut tx, tr, &tr_name).await;
        // A qualifying, delegator-impersonating Trino engine: the profile's
        // infra rows above are exactly what convergence derives for it.
        engine_bound_spec(
            &mut tx,
            "g16.engine",
            tr,
            "trino",
            json!({"trinoConfig": {
                "coordinator": {},
                "delegationMode": "impersonate",
                "auth": {"password": {}}
            }}),
            &mut names,
        )
        .await;
        stored_policy(&mut tx, tr, "g16 operator allow",
            json!({
                "name": "g16 operator allow", "service": tr_name, "serviceType": "trino",
                "policyType": 0, "isEnabled": true,
                "resources": {"catalog": level("gold_g16_cat"), "schema": level("ops"), "table": level("*"), "column": level("*")},
                "policyItems": [{"users": ["g16_alice"], "groups": [], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}], "delegateAdmin": false}],
                "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": [], "isDenyAllElse": false
            }),
            "operator", true).await;
        stored_policy(&mut tx, tr, "g16 operator off",
            hand_deny("g16 operator off", &tr_name, "trino",
                json!({"catalog": level("gold_g16_cat"), "schema": level("ops"), "table": level("audit"), "column": level("*")}),
                &["g16_alice"]),
            "operator", false).await;
        grant(
            &mut tx,
            "g16.read",
            Effect::Allow,
            &[Action::Read],
            scope(cat, Some("sales"), None, None),
            set(&[user_ref("g16_alice")], &[]),
            &mut names,
        )
        .await;
        let captured = crate::capture(&mut tx, &[(tr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G16_hand_written_and_infra", captured, &names);
    })
}

/// G17: several engines per profile: two StarRocks engines at 2.8.0 and 2.9.0
/// (fold agreement), two catalogs; then a profile whose engine runs an image
/// egret has not measured (the clearance mask's not-modelled skip).
pub fn g17_several_engines(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, _) = profile(db, "g17", "starrocks", false, &mut names).await;
        let unknown =
            profile_with_config(db, "g17.unmeasured", "starrocks", json!({}), &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh_a = warehouse(&mut tx, "g17.a", &mut names).await;
        let wh_b = warehouse(&mut tx, "g17.b", &mut names).await;
        let e28 = engine_bound(
            &mut tx,
            "g17.e28",
            sr,
            "starrocks",
            Some("starrocks/fe-ubuntu:4.1.4"),
            &mut names,
        )
        .await;
        let e29 = engine_bound(
            &mut tx,
            "g17.e29",
            sr,
            "starrocks",
            Some("starrocks/fe-ubuntu:4.1.6"),
            &mut names,
        )
        .await;
        let e_unknown = engine_bound(
            &mut tx,
            "g17.e_unknown",
            unknown,
            "starrocks",
            Some("starrocks/fe-ubuntu:9.9.9"),
            &mut names,
        )
        .await;
        let cat_a = catalog(&mut tx, "g17.cat_a", wh_a, &[e28, e_unknown], &mut names).await;
        catalog(&mut tx, "g17.cat_b", wh_b, &[e29], &mut names).await;
        let secret = tag(&mut tx, "g17.secret", &mut names).await;
        tag_column(&mut tx, secret, wh_a, "sales", "orders", "ssn").await;
        tag_column(&mut tx, secret, wh_b, "hr", "staff", "salary").await;
        platform_role(&mut tx, "g17_cleared", &mut names).await;
        let alice = user(&mut tx, "g17.alice", Some("g17_alice"), &mut names).await;
        let bob = user(&mut tx, "g17.bob", Some("g17_bob"), &mut names).await;
        role_held(&mut tx, alice, "g17_cleared").await;
        role_held(&mut tx, bob, "g17_cleared").await;
        rule(
            &mut tx,
            "g17.hash",
            mask("MASK_HASH", None),
            Selector::Path(scope(cat_a, Some("sales"), Some("orders"), Some("ssn"))),
            set(&[user_ref("g17_alice")], &[]),
            &mut names,
        )
        .await;
        gate(&mut tx, secret, &["g17_cleared"], &[]).await;
        let captured = crate::capture(&mut tx, &[(sr, None), (unknown, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G17_several_engines", captured, &names);
    })
}

/// G18: held renames: a table hold with leaving coordinates
/// (`sales.orders` → `sales.orders_v2`); a grant body staying on the old
/// name; tag-rule and clearance bodies on both names while the hold lasts.
pub fn g18_held_rename(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, _) = profile(db, "g18", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g18", &mut names).await;
        let cat = catalog_attached(&mut tx, "g18", wh, &[(sr, "starrocks")], &mut names).await;
        let lk = lk_link(&mut tx, wh, "g18.lk", &mut names).await;
        lk_table(&mut tx, lk, "sales", "orders", &["id", "email"], &mut names).await;
        let secret = tag(&mut tx, "g18.secret", &mut names).await;
        let pii = tag(&mut tx, "g18.pii", &mut names).await;
        for table in ["orders", "orders_v2"] {
            tag_table(&mut tx, secret, wh, "sales", table).await;
            tag_column(&mut tx, pii, wh, "sales", table, "email").await;
        }
        platform_role(&mut tx, "g18_cleared", &mut names).await;
        let alice = user(&mut tx, "g18.alice", Some("g18_alice"), &mut names).await;
        role_held(&mut tx, alice, "g18_cleared").await;
        gate(&mut tx, secret, &["g18_cleared"], &[]).await;
        rule(
            &mut tx,
            "g18.mask",
            mask("MASK_NULL", None),
            by_tag("g18.pii"),
            set(&[], &[]),
            &mut names,
        )
        .await;
        grant(
            &mut tx,
            "g18.read",
            Effect::Allow,
            &[Action::Read],
            scope(cat, Some("sales"), Some("orders"), None),
            set(&[user_ref("g18_alice")], &[]),
            &mut names,
        )
        .await;
        let overlay = json!([
            {"warehouse_id": wh, "schema": "sales", "table": "orders", "columns": null,
             "leaving": [], "leaving_table": true},
            {"warehouse_id": wh, "schema": "sales", "table": "orders_v2", "columns": ["id", "email"]}
        ]);
        sqlx::query("SELECT set_config($1, $2, true)")
            .bind(egret_perm_compile::table_columns::PENDING_COLUMNS_SETTING)
            .bind(overlay.to_string())
            .execute(&mut *tx)
            .await
            .expect("overlay");
        let captured = crate::capture(&mut tx, &[(sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G18_held_rename", captured, &names);
    })
}

/// G19: ceilings: a StarRocks tag mask on 501 columns (`ExpansionCeiling`) and
/// a clearance over the cap, each on its own profile and catalog.
pub fn g19_ceilings(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, _, _) = profile(db, "g19", "starrocks", false, &mut names).await;
        let sr_b =
            profile_with_config(db, "g19.clearance", "starrocks", json!({}), &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh_a = warehouse(&mut tx, "g19.a", &mut names).await;
        let wh_b = warehouse(&mut tx, "g19.b", &mut names).await;
        catalog_attached(&mut tx, "g19.a", wh_a, &[(sr, "starrocks")], &mut names).await;
        catalog_attached(&mut tx, "g19.b", wh_b, &[(sr_b, "starrocks")], &mut names).await;
        let wide = tag(&mut tx, "g19.wide", &mut names).await;
        tag_columns_bulk(&mut tx, wide, wh_a, "sales", "wide", 501).await;
        rule(
            &mut tx,
            "g19.mask",
            mask("MASK_NULL", None),
            by_tag("g19.wide"),
            set(&[], &[]),
            &mut names,
        )
        .await;
        let secret = tag(&mut tx, "g19.secret", &mut names).await;
        tag_columns_bulk(&mut tx, secret, wh_b, "sales", "wide", 501).await;
        platform_role(&mut tx, "g19_cleared", &mut names).await;
        gate(&mut tx, secret, &["g19_cleared"], &[]).await;
        let captured = crate::capture(&mut tx, &[(sr, None), (sr_b, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G19_ceilings", captured, &names);
    })
}

/// G20: an overlap refusal: two path row filters on one table with different
/// expressions.
pub fn g20_overlap_refusal(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (tr, tr_tag, _) = profile(db, "g20", "trino", true, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g20", &mut names).await;
        let cat = catalog_attached(&mut tx, "g20", wh, &[(tr, "trino")], &mut names).await;
        for (role, expr) in [("g20.eu", "region = 'EU'"), ("g20.us", "region = 'US'")] {
            rule(
                &mut tx,
                role,
                row_filter(expr),
                Selector::Path(scope(cat, Some("sales"), Some("orders"), None)),
                set(&[], &[]),
                &mut names,
            )
            .await;
        }
        let captured = crate::capture(&mut tx, &[(tr, tr_tag)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G20_overlap_refusal", captured, &names);
    })
}

/// G21's model, left open in the returned transaction (the timing test
/// compiles it repeatedly): StarRocks, 480 tagged columns under one mask, 400
/// under one clearance, 300 grants.
pub async fn g21_build<'a>(db: &'a Db, names: &mut Names) -> (i64, Tx<'a>) {
    let (sr, _, _) = profile(db, "g21", "starrocks", false, names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, "g21", names).await;
    let cat = catalog_attached(&mut tx, "g21", wh, &[(sr, "starrocks")], names).await;
    let alice = user(&mut tx, "g21.alice", Some("g21_alice"), names).await;
    let pii = tag(&mut tx, "g21.pii", names).await;
    tag_columns_bulk(&mut tx, pii, wh, "sales", "wide", 480).await;
    rule(
        &mut tx,
        "g21.mask",
        mask("MASK_NULL", None),
        by_tag("g21.pii"),
        set(&[user_ref("g21_alice")], &[]),
        names,
    )
    .await;
    let secret = tag(&mut tx, "g21.secret", names).await;
    tag_columns_bulk(&mut tx, secret, wh, "hr", "wide", 400).await;
    platform_role(&mut tx, "g21_cleared", names).await;
    role_held(&mut tx, alice, "g21_cleared").await;
    gate(&mut tx, secret, &["g21_cleared"], &[]).await;
    for i in 0..300 {
        grant(
            &mut tx,
            &format!("g21.grant{i:03}"),
            Effect::Allow,
            &[Action::Read],
            scope(cat, Some("lake"), Some(&format!("t{i:03}")), None),
            set(&[user_ref("g21_alice")], &[]),
            names,
        )
        .await;
    }
    (sr, tx)
}

/// G21: the large profile for timing.
pub fn g21_large_profile(db: &Db) -> Fut<'_> {
    Box::pin(async move {
        let mut names = Names::default();
        let (sr, mut tx) = g21_build(db, &mut names).await;
        let captured = crate::capture(&mut tx, &[(sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G21_large_profile", captured, &names);
    })
}
