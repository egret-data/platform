//! What a table or column rename carries of the permission model (grants
//! follow spec §5.2, D3, D4, D14; §5.9 for covers): selection over real store
//! rows, written through `create_grant_in_tx` / `create_rule_in_tx`.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-perm-compile --test follow_db

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind,
    Scope, Selector,
};
use egret_perm_compile::follow::{plan_via, Coord, Move, PermFollow, PermRow};
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fx {
    db: Db,
    catalog_id: Uuid,
}

async fn fx() -> Option<Fx> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'iceberg', '{}'::jsonb) \
         RETURNING id",
    )
    .bind(unique("fl_cat"))
    .fetch_one(&db.pool)
    .await
    .expect("catalog");
    Some(Fx { db, catalog_id })
}

fn group(name: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: name.into(),
    }
}

fn path(catalog_id: Uuid, levels: &[&str]) -> Scope {
    let at = |i: usize| levels.get(i).map(|s| s.to_string());
    Scope {
        catalog_id,
        schema: at(0),
        table: at(1),
        column: at(2),
    }
}

impl Fx {
    fn scope(&self, levels: &[&str]) -> Scope {
        path(self.catalog_id, levels)
    }

    async fn grant(&self, effect: Effect, levels: &[&str]) -> Uuid {
        self.grant_to(effect, levels, &[Action::Read], &[group("g")])
            .await
    }

    async fn grant_to(
        &self,
        effect: Effect,
        levels: &[&str],
        actions: &[Action],
        include: &[Principal],
    ) -> Uuid {
        let mut tx = self.db.pool.begin().await.expect("begin");
        let id = egret_perm::create_grant_in_tx(
            &mut tx,
            &Grant {
                id: Uuid::nil(),
                effect,
                actions: actions.to_vec(),
                scope: self.scope(levels),
                principals: PrincipalSet {
                    include: include.to_vec(),
                    exclude: Vec::new(),
                },
                origin: GrantOrigin::Explicit,
            },
        )
        .await
        .expect("grant");
        tx.commit().await.expect("commit");
        id
    }

    async fn assignment_grant(&self) -> Uuid {
        let ws: Uuid =
            sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'FL') RETURNING id")
                .bind(unique("fl-ws"))
                .fetch_one(&self.db.pool)
                .await
                .expect("workspace");
        let mut tx = self.db.pool.begin().await.expect("begin");
        let id = egret_perm::create_grant_in_tx(
            &mut tx,
            &Grant {
                id: Uuid::nil(),
                effect: Effect::Allow,
                actions: vec![Action::Read],
                scope: self.scope(&[]),
                principals: PrincipalSet {
                    include: vec![Principal {
                        kind: PrincipalKind::WorkspaceMembers,
                        r#ref: ws.to_string(),
                    }],
                    exclude: Vec::new(),
                },
                origin: GrantOrigin::WorkspaceAssignment { workspace_id: ws },
            },
        )
        .await
        .expect("assignment grant");
        tx.commit().await.expect("commit");
        id
    }

    async fn rule(&self, kind: RuleKind, levels: &[&str]) -> Uuid {
        self.rule_named(&unique("fl-rule"), kind, levels, PrincipalSet::default())
            .await
    }

    async fn rule_named(
        &self,
        name: &str,
        kind: RuleKind,
        levels: &[&str],
        exempt: PrincipalSet,
    ) -> Uuid {
        let mut tx = self.db.pool.begin().await.expect("begin");
        let id = egret_perm::create_rule_in_tx(
            &mut tx,
            &Rule {
                id: Uuid::nil(),
                name: name.to_string(),
                kind,
                selector: Selector::Path(self.scope(levels)),
                exempt,
            },
        )
        .await
        .expect("rule");
        tx.commit().await.expect("commit");
        id
    }

    async fn plan(&self, from: (&str, &str), to: (&str, &str)) -> PermFollow {
        self.plan_move(Move {
            catalog_id: self.catalog_id,
            from: Coord {
                schema: from.0.into(),
                table: from.1.into(),
                column: None,
            },
            to: Coord {
                schema: to.0.into(),
                table: to.1.into(),
                column: None,
            },
        })
        .await
    }

    async fn plan_move(&self, mv: Move) -> PermFollow {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        plan_via(&mut conn, &mv).await.expect("plan")
    }
}

fn mask() -> RuleKind {
    RuleKind::Mask {
        mask_type: "MASK_HASH".into(),
        value_expr: None,
    }
}

fn filter(expr: &str) -> RuleKind {
    RuleKind::RowFilter {
        filter_expr: expr.into(),
    }
}

fn ids_of_grants(gs: &[Grant]) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = gs.iter().map(|g| g.id).collect();
    v.sort();
    v
}

fn ids_of_rules(rs: &[Rule]) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = rs.iter().map(|r| r.id).collect();
    v.sort();
    v
}

fn refused_ids(p: &PermFollow) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = p
        .refused
        .iter()
        .map(|r| match &r.row {
            PermRow::Grant(g) => g.id,
            PermRow::Rule(r) => r.id,
        })
        .collect();
    v.sort();
    v
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

/// D3: literal rows follow by effect; selection folds; a workspace
/// assignment grant (catalog-wide) goes nowhere.
#[tokio::test]
async fn literal_rows_follow_by_effect() {
    let Some(f) = fx().await else { return };
    let deny = f.grant(Effect::Deny, &["sales", "orders"]).await;
    let allow = f.grant(Effect::Allow, &["sales", "orders"]).await;
    let column_allow = f.grant(Effect::Allow, &["Sales", "ORDERS", "email"]).await;
    let m = f.rule(mask(), &["sales", "orders", "email"]).await;
    let rf = f.rule(filter("region = 'eu'"), &["sales", "orders"]).await;
    f.assignment_grant().await;
    // Another table's rows stay out of it.
    f.grant(Effect::Deny, &["sales", "events"]).await;

    let p = f.plan(("sales", "orders"), ("sales", "orders_v2")).await;
    assert_eq!(ids_of_grants(&p.copy_grants), vec![deny]);
    assert_eq!(
        ids_of_grants(&p.move_grants),
        sorted(vec![allow, column_allow])
    );
    assert_eq!(ids_of_rules(&p.copy_rules), sorted(vec![m, rf]));
    assert!(p.refused.is_empty(), "{:?}", p.refused);
    assert!(p.left.is_empty(), "{:?}", p.left);
}

/// D4, G4: a restrictive pattern that matched the old name and does not
/// match the new one refuses; one that matches both changes nothing.
#[tokio::test]
async fn restrictive_patterns_that_stop_matching_refuse() {
    let Some(f) = fx().await else { return };
    // Another subject than `prefix`'s, so neither covers the other (G5).
    let both = f
        .grant_to(
            Effect::Deny,
            &["sales", "ord*"],
            &[Action::Read],
            &[group("other")],
        )
        .await;
    let prefix = f.grant(Effect::Deny, &["sales", "orders*"]).await;
    let p = f.plan(("sales", "orders"), ("sales", "archive")).await;
    assert_eq!(refused_ids(&p), sorted(vec![both, prefix]));
    let p = f.plan(("sales", "orders"), ("sales", "ordinal")).await;
    assert_eq!(refused_ids(&p), vec![prefix]);
    let p = f.plan(("sales", "orders"), ("sales", "orders_v2")).await;
    assert!(p.refused.is_empty(), "{both} matches both: {:?}", p.refused);
    assert!(p.copy_grants.is_empty());

    let Some(f) = fx().await else { return };
    let schema_filter = f.rule(filter("region = 'eu'"), &["sales"]).await;
    let p = f.plan(("sales", "orders"), ("finance", "orders")).await;
    assert_eq!(refused_ids(&p), vec![schema_filter]);
    // Within the schema it reaches both names.
    let p = f.plan(("sales", "orders"), ("sales", "orders_v2")).await;
    assert!(p.is_empty(), "{p:?}");

    let Some(f) = fx().await else { return };
    let schema_pattern = f.rule(mask(), &["*", "orders", "email"]).await;
    let p = f.plan(("sales", "orders"), ("sales", "x")).await;
    assert_eq!(refused_ids(&p), vec![schema_pattern]);
}

/// D4, G4: an allow pattern that stops matching is left (reported); one
/// that newly matches the destination is the author's rule.
#[tokio::test]
async fn allow_patterns_are_left() {
    let Some(f) = fx().await else { return };
    let prefix = f.grant(Effect::Allow, &["sales", "orders*"]).await;
    let p = f.plan(("sales", "orders"), ("sales", "archive")).await;
    assert_eq!(ids_of_grants(&p.left), vec![prefix]);
    assert!(p.refused.is_empty() && p.move_grants.is_empty());

    let Some(f) = fx().await else { return };
    let schema = f.grant(Effect::Allow, &["sales"]).await;
    let p = f.plan(("sales", "orders"), ("finance", "orders")).await;
    assert_eq!(ids_of_grants(&p.left), vec![schema]);

    let Some(f) = fx().await else { return };
    f.grant(Effect::Allow, &["sales", "tmp_*"]).await;
    let p = f.plan(("sales", "x"), ("sales", "tmp_x")).await;
    assert!(p.is_empty(), "{p:?}");
}

/// D14: no perm row can name a namespace of several levels: a following
/// restriction refuses, a following allow is left.
#[tokio::test]
async fn a_multi_level_destination_refuses_restrictions_and_leaves_allows() {
    let Some(f) = fx().await else { return };
    let deny = f.grant(Effect::Deny, &["sales", "orders"]).await;
    let allow = f.grant(Effect::Allow, &["sales", "orders"]).await;
    let m = f.rule(mask(), &["sales", "orders", "email"]).await;
    let p = f
        .plan(("sales", "orders"), ("sales\u{1f}eu", "orders"))
        .await;
    assert_eq!(refused_ids(&p), sorted(vec![deny, m]));
    assert_eq!(ids_of_grants(&p.left), vec![allow]);
    assert!(!p.copies() && p.move_grants.is_empty());
}

/// D3, D4 one level down: a column move selects column-level rows only.
#[tokio::test]
async fn a_column_move_selects_column_rows() {
    let Some(f) = fx().await else { return };
    let deny = f.grant(Effect::Deny, &["sales", "orders", "email"]).await;
    let m = f.rule(mask(), &["sales", "orders", "EMAIL"]).await;
    let allow = f.grant(Effect::Allow, &["sales", "orders", "email"]).await;
    // Another mask type than the copied one, which would cover it (G5, D21).
    let pattern = f
        .rule(
            RuleKind::Mask {
                mask_type: "MASK_NULL".into(),
                value_expr: None,
            },
            &["sales", "orders", "e*"],
        )
        .await;
    f.grant(Effect::Deny, &["sales", "orders"]).await;
    f.rule(filter("region = 'eu'"), &["sales", "orders"]).await;
    // Another column's rows stay.
    f.grant(Effect::Deny, &["sales", "orders", "phone"]).await;

    let at = |c: &str| Coord {
        schema: "sales".into(),
        table: "orders".into(),
        column: Some(c.into()),
    };
    let p = f
        .plan_move(Move {
            catalog_id: f.catalog_id,
            from: at("email"),
            to: at("mail"),
        })
        .await;
    assert_eq!(ids_of_grants(&p.copy_grants), vec![deny]);
    assert_eq!(ids_of_rules(&p.copy_rules), vec![m]);
    assert_eq!(ids_of_grants(&p.move_grants), vec![allow]);
    assert_eq!(refused_ids(&p), vec![pattern]);
    assert!(p.left.is_empty());
}

// ── Task 3: hand-written policies, copies, moves, undo (D1, D2, D5, D13) ──

use egret_perm_compile::follow::{
    copy_in_tx, delete_copies_in_tx, handwritten_via, move_allows_in_tx, refusal_message, Copies,
    HandwrittenCheck,
};
use serde_json::{json, Value};

impl Fx {
    async fn catalog_name(&self) -> String {
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(self.catalog_id)
            .fetch_one(&self.db.pool)
            .await
            .expect("catalog name")
    }

    async fn profile(&self) -> i64 {
        let policy = egret_policy::PolicyStore::new(self.db.clone());
        egret_policy::seed_trino(&policy).await.expect("seed trino");
        policy
            .create_service(&unique("fl_trino"), "trino", &json!({}))
            .await
            .expect("profile")
            .id
    }

    async fn policy(&self, service: i64, body: Value, enabled: bool, managed_by: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(service)
        .bind(unique("fl-pol"))
        .bind(body)
        .bind(enabled)
        .bind(managed_by)
        .fetch_one(&self.db.pool)
        .await
        .expect("policy")
    }

    fn table_move(&self, from: &str, to: &str) -> Move {
        Move {
            catalog_id: self.catalog_id,
            from: Coord {
                schema: "sales".into(),
                table: from.into(),
                column: None,
            },
            to: Coord {
                schema: "sales".into(),
                table: to.into(),
                column: None,
            },
        }
    }

    async fn grant_row(&self, id: Uuid) -> Option<Grant> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm::get_grant_via(&mut conn, id)
            .await
            .expect("grant")
    }

    async fn rule_row(&self, id: Uuid) -> Option<Rule> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm::get_rule_via(&mut conn, id).await.expect("rule")
    }

    async fn copy(&self, mv: &Move) -> Copies {
        let p = self.plan_move(mv.clone()).await;
        let mut tx = self.db.pool.begin().await.expect("begin");
        let copies = copy_in_tx(&mut tx, &p, mv).await.expect("copy");
        tx.commit().await.expect("commit");
        copies
    }

    async fn grants_on(&self, table: &str) -> Vec<Grant> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm::list_grants_via(&mut conn)
            .await
            .expect("grants")
            .into_iter()
            .filter(|g| {
                g.scope.catalog_id == self.catalog_id && g.scope.table.as_deref() == Some(table)
            })
            .collect()
    }

    async fn rules_on(&self, table: &str) -> Vec<Rule> {
        let mut conn = self.db.pool.acquire().await.expect("acquire");
        egret_perm::list_rules_via(&mut conn)
            .await
            .expect("rules")
            .into_iter()
            .filter(|r| match &r.selector {
                Selector::Path(s) => {
                    s.catalog_id == self.catalog_id && s.table.as_deref() == Some(table)
                }
                Selector::Tag { .. } => false,
            })
            .collect()
    }
}

fn access(catalog: &str, table: &[&str], excludes: bool, items: &str) -> Value {
    json!({
        "policyType": 0,
        "resources": {
            "catalog": {"values": [catalog], "isExcludes": false},
            "schema": {"values": ["sales"], "isExcludes": false},
            "table": {"values": table, "isExcludes": excludes},
            "column": {"values": ["*"], "isExcludes": false},
        },
        "policyItems": if items == "allow" { json!([{"users": ["u"], "accesses": [{"type": "select"}]}]) } else { json!([]) },
        "denyPolicyItems": if items == "deny" { json!([{"users": ["u"], "accesses": [{"type": "select"}]}]) } else { json!([]) },
    })
}

/// D5, G3: enabled operator policies on a bound profile that name the table
/// literally refuse when restrictive and are reported when a plain allow;
/// everything else is neither.
#[tokio::test]
async fn handwritten_restrictions_naming_the_table_refuse() {
    let Some(f) = fx().await else { return };
    let cat = f.catalog_name().await;
    let bound = f.profile().await;
    let unbound = f.profile().await;
    let mask = f
        .policy(
            bound,
            json!({
                "policyType": 1,
                "resources": {
                    "catalog": {"values": [cat]}, "schema": {"values": ["sales"]},
                    "table": {"values": ["orders"]}, "column": {"values": ["email"]},
                },
                "dataMaskPolicyItems": [{"users": ["u"], "dataMaskInfo": {"dataMaskType": "MASK_HASH"}}],
            }),
            true,
            "operator",
        )
        .await;
    let deny = f
        .policy(
            bound,
            access(&cat, &["orders", "customers"], false, "deny"),
            true,
            "operator",
        )
        .await;
    let excludes = f
        .policy(
            bound,
            access(&cat, &["Orders"], true, "allow"),
            true,
            "operator",
        )
        .await;
    let row_filter = f
        .policy(
            bound,
            json!({
                "policyType": 2,
                "resources": {
                    "catalog": {"values": ["*"]}, "schema": {"values": ["sales"]},
                    "table": {"values": ["orders"]},
                },
                "rowFilterPolicyItems": [{"users": ["u"], "rowFilterInfo": {"filterExpr": "1=1"}}],
            }),
            true,
            "operator",
        )
        .await;
    let allow = f
        .policy(
            bound,
            access(&cat, &["orders"], false, "allow"),
            true,
            "operator",
        )
        .await;
    f.policy(
        bound,
        access(&cat, &["ord*"], false, "allow"),
        true,
        "operator",
    )
    .await;
    f.policy(
        bound,
        access(&cat, &["orders"], false, "deny"),
        false,
        "operator",
    )
    .await;
    f.policy(
        bound,
        access("*", &["orders"], false, "deny"),
        true,
        "infra",
    )
    .await;
    f.policy(
        unbound,
        access(&cat, &["orders"], false, "deny"),
        true,
        "operator",
    )
    .await;
    f.policy(
        bound,
        access("other_catalog", &["orders"], false, "deny"),
        true,
        "operator",
    )
    .await;

    let mut conn = f.db.pool.acquire().await.expect("acquire");
    let HandwrittenCheck { refuse, report } = handwritten_via(
        &mut conn,
        &[bound],
        &cat,
        &f.table_move("orders", "orders_v2"),
    )
    .await
    .expect("check");
    let mut refused: Vec<i64> = refuse.iter().map(|h| h.id).collect();
    refused.sort_unstable();
    let mut want = vec![mask, deny, excludes, row_filter];
    want.sort_unstable();
    assert_eq!(refused, want);
    assert_eq!(report.iter().map(|h| h.id).collect::<Vec<_>>(), vec![allow]);
    let message =
        refusal_message(&[], &refuse, &f.table_move("orders", "orders_v2")).expect("a refusal");
    assert!(message.contains(&refuse[0].label()), "{message}");
    assert!(message.ends_with(". Nothing was renamed."), "{message}");
}

/// D1: a copy is a new row on the destination with the same principals or
/// exemptions; a rule copy gets a unique name; the originals are untouched.
#[tokio::test]
async fn copies_get_new_ids_principals_and_unique_names() {
    let Some(f) = fx().await else { return };
    let deny = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders"],
            &[Action::Read, Action::Write],
            &[group("g1"), group("g2")],
        )
        .await;
    let pii = unique("pii-email");
    let exempt = PrincipalSet {
        include: vec![group("cleared")],
        exclude: Vec::new(),
    };
    let m = f
        .rule_named(&pii, mask(), &["sales", "orders", "email"], exempt.clone())
        .await;
    // A rule whose copy's name is taken already.
    let other = unique("other");
    let taken = format!("{other} (sales.orders_v2)");
    f.rule_named(
        &taken,
        mask(),
        &["sales", "elsewhere", "x"],
        PrincipalSet::default(),
    )
    .await;
    let o = f
        .rule_named(
            &other,
            filter("1 = 1"),
            &["sales", "orders"],
            PrincipalSet::default(),
        )
        .await;
    let before = (
        f.grant_row(deny).await,
        f.rule_row(m).await,
        f.rule_row(o).await,
    );

    let mv = f.table_move("orders", "orders_v2");
    let copies = f.copy(&mv).await;
    assert_eq!(copies.grants.len(), 1);
    assert_eq!(copies.rules.len(), 2);
    let (src, new) = copies.grants[0];
    assert_eq!(src, deny);
    assert_ne!(new, deny);
    let copy = f.grant_row(new).await.expect("copy");
    let original = before.0.clone().expect("deny");
    assert_eq!(copy.scope.table.as_deref(), Some("orders_v2"));
    assert_eq!(copy.effect, Effect::Deny);
    assert_eq!(copy.actions, original.actions);
    assert_eq!(copy.principals, original.principals);
    let copied_rules: Vec<Rule> = f.rules_on("orders_v2").await;
    let names: Vec<&str> = copied_rules.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&format!("{pii} (sales.orders_v2)").as_str()),
        "{names:?}"
    );
    assert!(
        names.contains(&format!("{other} (sales.orders_v2) #2").as_str()),
        "{names:?}"
    );
    let mask_copy = copied_rules
        .iter()
        .find(|r| r.name.starts_with(&pii))
        .expect("mask copy");
    assert_eq!(mask_copy.exempt, exempt);
    assert_eq!(
        mask_copy.selector,
        Selector::Path(path(f.catalog_id, &["sales", "orders_v2", "email"]))
    );
    // The originals are byte-for-byte what they were.
    assert_eq!(
        (
            f.grant_row(deny).await,
            f.rule_row(m).await,
            f.rule_row(o).await
        ),
        before
    );
}

/// D13: a copy whose equivalent already sits on the destination is skipped.
#[tokio::test]
async fn an_equivalent_row_on_the_destination_is_not_copied_again() {
    let Some(f) = fx().await else { return };
    f.grant(Effect::Deny, &["sales", "orders"]).await;
    f.rule(mask(), &["sales", "orders", "email"]).await;
    // The same deny on the destination, spelled in another case.
    f.grant(Effect::Deny, &["sales", "ORDERS_V2"]).await;
    let mv = f.table_move("orders", "orders_v2");
    let first = f.copy(&mv).await;
    assert!(first.grants.is_empty(), "{first:?}");
    assert_eq!(first.rules.len(), 1);
    let second = f.copy(&mv).await;
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(f.rules_on("orders_v2").await.len(), 1);
}

/// Compensation deletes exactly the copies; a moved allow keeps its id.
#[tokio::test]
async fn delete_copies_removes_exactly_the_copies() {
    let Some(f) = fx().await else { return };
    let deny = f.grant(Effect::Deny, &["sales", "orders"]).await;
    let m = f.rule(mask(), &["sales", "orders", "email"]).await;
    // A row already on the destination before the copy stays.
    let there = f.grant(Effect::Deny, &["sales", "orders_v2", "x"]).await;
    let mv = f.table_move("orders", "orders_v2");
    let copies = f.copy(&mv).await;
    assert_eq!(f.grants_on("orders_v2").await.len(), 2);
    let mut tx = f.db.pool.begin().await.expect("begin");
    delete_copies_in_tx(&mut tx, &copies).await.expect("undo");
    tx.commit().await.expect("commit");
    let left: Vec<Uuid> = f
        .grants_on("orders_v2")
        .await
        .iter()
        .map(|g| g.id)
        .collect();
    assert_eq!(left, vec![there]);
    assert!(f.rules_on("orders_v2").await.is_empty());
    assert!(f.grant_row(deny).await.is_some() && f.rule_row(m).await.is_some());
}

#[tokio::test]
async fn move_allows_keeps_the_id() {
    let Some(f) = fx().await else { return };
    let allow = f.grant(Effect::Allow, &["Sales", "orders", "email"]).await;
    let deny = f.grant(Effect::Deny, &["sales", "orders"]).await;
    let mv = f.table_move("orders", "Orders_V2");
    let mut tx = f.db.pool.begin().await.expect("begin");
    let moved = move_allows_in_tx(&mut tx, &[allow, deny], &mv)
        .await
        .expect("move");
    tx.commit().await.expect("commit");
    assert_eq!(moved.len(), 1, "only the allow moves");
    let after = f.grant_row(allow).await.expect("same id");
    // The renamed level takes the request's spelling; the others keep theirs.
    assert_eq!(
        after.scope,
        path(f.catalog_id, &["Sales", "Orders_V2", "email"])
    );
    assert_eq!(
        f.grant_row(deny)
            .await
            .expect("deny")
            .scope
            .table
            .as_deref(),
        Some("orders")
    );
}

// ── Task 9: a covered destination does not refuse (G5, spec §5.9, D21) ──

use egret_perm_compile::follow::{cover_via, Cover};

impl Fx {
    /// The refusal `plan` gives one row for `orders → archive`, or `None`.
    async fn refusal_of(&self, id: Uuid) -> Option<Vec<String>> {
        let p = self.plan(("sales", "orders"), ("sales", "archive")).await;
        p.refused
            .iter()
            .find(|r| match &r.row {
                PermRow::Grant(g) => g.id == id,
                PermRow::Rule(r) => r.id == id,
            })
            .map(|r| r.missing.clone())
    }

    async fn rule_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM perm.rules WHERE catalog_id = $1")
            .bind(self.catalog_id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count")
    }
}

fn user(name: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: name.into(),
    }
}

/// G5: an exact-name deny on the new name with at least the actions and the
/// subjects covers a pattern deny; so does another pattern, and so do two
/// denies sharing the work.
#[tokio::test]
async fn a_deny_pattern_is_covered_by_an_equal_deny_on_the_new_name() {
    let Some(f) = fx().await else { return };
    let pattern = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders*"],
            &[Action::Read],
            &[group("contractors")],
        )
        .await;
    assert!(f.refusal_of(pattern).await.is_some(), "uncovered: refused");
    f.grant_to(
        Effect::Deny,
        &["sales", "archive"],
        &[Action::Read, Action::Write],
        &[group("contractors"), user("bob")],
    )
    .await;
    assert_eq!(f.refusal_of(pattern).await, None, "covered");

    let Some(f) = fx().await else { return };
    let pattern = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders*"],
            &[Action::Read],
            &[group("contractors")],
        )
        .await;
    f.grant_to(
        Effect::Deny,
        &["sales", "arch*"],
        &[Action::Read],
        &[group("contractors")],
    )
    .await;
    assert_eq!(
        f.refusal_of(pattern).await,
        None,
        "covered by another pattern"
    );

    let Some(f) = fx().await else { return };
    let pattern = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders*"],
            &[Action::Read, Action::Write],
            &[group("contractors")],
        )
        .await;
    f.grant_to(
        Effect::Deny,
        &["sales", "archive"],
        &[Action::Read],
        &[group("contractors")],
    )
    .await;
    assert_eq!(
        f.refusal_of(pattern).await,
        Some(vec![
            "sales.archive lacks a deny of write for group:contractors".into()
        ])
    );
    f.grant_to(
        Effect::Deny,
        &["sales", "archive"],
        &[Action::Write],
        &[group("contractors")],
    )
    .await;
    assert_eq!(
        f.refusal_of(pattern).await,
        None,
        "two denies share the work"
    );
}

/// G5: a mask of the same type and expression, exempting no more, covers.
#[tokio::test]
async fn a_mask_pattern_is_covered_by_an_equal_mask_on_the_new_name() {
    let Some(f) = fx().await else { return };
    let pattern = f
        .rule_named(
            &unique("pm"),
            mask(),
            &["sales", "orders*", "email"],
            PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::Role,
                    r#ref: "pii-cleared".into(),
                }],
                exclude: Vec::new(),
            },
        )
        .await;
    // Another type, then the same type with another expression: partial.
    let nullify = f
        .rule(
            RuleKind::Mask {
                mask_type: "MASK_NULL".into(),
                value_expr: None,
            },
            &["sales", "archive", "email"],
        )
        .await;
    assert_eq!(
        f.refusal_of(pattern).await,
        Some(vec![
            "sales.archive has no mask on email with type MASK_HASH and no expression".into()
        ])
    );
    let _ = nullify;
    let spaced = f
        .rule(
            RuleKind::Mask {
                mask_type: "MASK_HASH".into(),
                value_expr: Some(" ".into()),
            },
            &["sales", "archive", "email"],
        )
        .await;
    assert!(
        f.refusal_of(pattern).await.is_some(),
        "{spaced}: another expression"
    );
    f.rule(mask(), &["sales", "archive", "EMAIL"]).await;
    assert_eq!(f.refusal_of(pattern).await, None, "covered, column folded");
}

/// G5: a row filter of the same text and exemptions covers; another text
/// does not.
#[tokio::test]
async fn a_row_filter_pattern_is_covered_by_an_equal_filter_on_the_new_name() {
    let Some(f) = fx().await else { return };
    let pattern = f.rule(filter("region = 'eu'"), &["sales", "orders*"]).await;
    f.rule(filter("region='eu'"), &["sales", "archive"]).await;
    assert_eq!(
        f.refusal_of(pattern).await,
        Some(vec![
            "sales.archive has no row filter \"region = 'eu'\"".into()
        ])
    );
    f.rule(filter("region = 'eu'"), &["sales", "archive"]).await;
    assert_eq!(f.refusal_of(pattern).await, None);
}

/// G5: a partial cover refuses and names what is missing; a deny on a member
/// does not cover a deny on the group.
#[tokio::test]
async fn a_partial_cover_refuses_and_names_what_is_missing() {
    let Some(f) = fx().await else { return };
    let pattern = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders*"],
            &[Action::Read],
            &[group("contractors"), group("vendors")],
        )
        .await;
    f.grant_to(
        Effect::Deny,
        &["sales", "archive"],
        &[Action::Read],
        &[group("contractors")],
    )
    .await;
    f.grant_to(
        Effect::Deny,
        &["sales", "archive"],
        &[Action::Read],
        &[user("vera")],
    )
    .await;
    assert_eq!(
        f.refusal_of(pattern).await,
        Some(vec![
            "sales.archive lacks a deny of read for group:vendors".into()
        ])
    );
    let p = f.plan(("sales", "orders"), ("sales", "archive")).await;
    let mv = f.table_move("orders", "archive");
    let message = refusal_message(&p.refused, &[], &mv).expect("refused");
    assert!(
        message.contains("sales.archive lacks a deny of read for group:vendors"),
        "{message}"
    );
}

/// D21: a copy this rename makes counts as a cover.
#[tokio::test]
async fn a_copy_this_rename_makes_counts_as_a_cover() {
    let Some(f) = fx().await else { return };
    let pattern = f
        .grant_to(
            Effect::Deny,
            &["sales", "orders*"],
            &[Action::Read],
            &[group("contractors")],
        )
        .await;
    f.grant_to(
        Effect::Deny,
        &["sales", "orders"],
        &[Action::Read],
        &[group("contractors")],
    )
    .await;
    assert_eq!(f.refusal_of(pattern).await, None);
}

/// G5 guards: a tag-selected mask and a hand-written policy on the new name
/// are never candidates, and deciding a cover writes nothing.
#[tokio::test]
async fn tag_rules_and_handwritten_policies_never_cover_and_nothing_is_written() {
    let Some(f) = fx().await else { return };
    let pattern = f.rule(mask(), &["sales", "orders*", "email"]).await;
    let key = unique("pii");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&key)
        .execute(&f.db.pool)
        .await
        .expect("tag");
    let mut tx = f.db.pool.begin().await.expect("begin");
    egret_perm::create_rule_in_tx(
        &mut tx,
        &Rule {
            id: Uuid::nil(),
            name: unique("tag-mask"),
            kind: mask(),
            selector: Selector::Tag { key },
            exempt: PrincipalSet::default(),
        },
    )
    .await
    .expect("tag rule");
    tx.commit().await.expect("commit");
    let cat = f.catalog_name().await;
    let profile = f.profile().await;
    f.policy(
        profile,
        json!({"policyType": 1, "resources": {
            "catalog": {"values": [cat]}, "schema": {"values": ["sales"]},
            "table": {"values": ["archive"]}, "column": {"values": ["email"]}},
            "dataMaskPolicyItems": [{"users": ["u"], "dataMaskInfo": {"dataMaskType": "MASK_HASH"}}]}),
        true,
        "operator",
    )
    .await;
    let before = f.rule_count().await;
    let p = f.plan(("sales", "orders"), ("sales", "archive")).await;
    let row = p.refused.first().expect("refused").row.clone();
    let mut conn = f.db.pool.acquire().await.expect("acquire");
    let c = cover_via(&mut conn, &row, &f.table_move("orders", "archive"), &p)
        .await
        .expect("cover");
    assert!(matches!(c, Cover::Partial { .. }), "{c:?} {pattern}");
    assert_eq!(f.rule_count().await, before, "egret inserts no cover");
}
