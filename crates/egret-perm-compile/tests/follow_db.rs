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
    let both = f.grant(Effect::Deny, &["sales", "ord*"]).await;
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
    let pattern = f.rule(mask(), &["sales", "orders", "e*"]).await;
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
