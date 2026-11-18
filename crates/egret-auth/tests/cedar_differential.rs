//! EO2 decision-equivalence differential: the egret-cedar-compiled PolicySet
//! must return the SAME Cedar decision as the live PDP for every
//! (principal, object, action). Oracle = `CedarPdp` (today). Both sides run
//! through the identical request path (`decide_with_for_test`), so only the
//! template set differs. DB-gated: fails without a DB unless EGRET_ALLOW_DB_SKIP.
//!
//! Reproduce: `EGRET_CEDAR_DIFF_SEED=<n> cargo test -p egret-auth --test cedar_differential`.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{ObjectRef, Resource, Subject};
use egret_core::rbac::{ADMIN_GRAPH_ACTIONS, PERMISSIONS};
use egret_core::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use egret_perm_ir::egret_object::{classify, PermissionMapping};
use uuid::Uuid;

struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        // SplitMix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next_u64() as f64 / u64::MAX as f64) < p
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

const SEED: u64 = 0xED2_0000_0001;
const STATES: usize = 24;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

struct Case {
    subject: Subject,
    resource: Resource,
    action: String,
    note: String,
}

async fn seed_user(db: &Db) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    UserId(id)
}

async fn seed_workspace(db: &Db) -> WorkspaceId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    WorkspaceId(id)
}

async fn seed_role(db: &Db, scope: &str, parent: Option<Uuid>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description, parent_role_id) \
         VALUES ($1, $2, 'eo2 diff', $3) RETURNING id",
    )
    .bind(scope)
    .bind(format!("eo2_{}", Uuid::new_v4().simple()))
    .bind(parent)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Every action a question may ask: the whole catalog (object-data + typed) plus
/// the admin-graph actions, plus one unknown key. Returned with its bucket label.
fn action_universe() -> Vec<(String, &'static str)> {
    let mut v: Vec<(String, &'static str)> = PERMISSIONS
        .iter()
        .map(|p| {
            let bucket = match classify(p.key) {
                Some(PermissionMapping::ObjectGrant { .. }) => "object-data",
                Some(_) => "typed",
                None => "unclassified",
            };
            (p.key.to_string(), bucket)
        })
        .collect();
    v.extend(
        ADMIN_GRAPH_ACTIONS
            .iter()
            .map(|a| (a.to_string(), "admin-graph")),
    );
    v.push(("nonexistent:action".into(), "unknown"));
    v
}

/// Seed one random state and return its (subject, resource, action) questions.
async fn draw_state(db: &Db, rng: &mut Rng, idx: usize) -> Vec<Case> {
    let universe = action_universe();
    let perm_keys: Vec<&str> = PERMISSIONS.iter().map(|p| p.key).collect();

    let ws = [seed_workspace(db).await, seed_workspace(db).await];
    let users = [
        seed_user(db).await,
        seed_user(db).await,
        seed_user(db).await,
    ];

    // Roles: fresh ones (some with a parent in the same scope) plus the builtins.
    let mut roles: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, scope FROM roles ORDER BY name")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    for _ in 0..(2 + rng.below(4)) {
        let scope = if rng.chance(0.4) {
            "platform"
        } else {
            "workspace"
        };
        let same: Vec<Uuid> = roles
            .iter()
            .filter(|(_, s)| s == scope)
            .map(|(i, _)| *i)
            .collect();
        let parent = (rng.chance(0.4) && !same.is_empty()).then(|| *rng.pick(&same));
        let id = seed_role(db, scope, parent).await;
        for _ in 0..rng.below(8) {
            let perm = *rng.pick(&perm_keys);
            sqlx::query(
                "INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(perm)
            .execute(&db.pool)
            .await
            .unwrap();
        }
        roles.push((id, scope.to_string()));
    }
    let plat: Vec<Uuid> = roles
        .iter()
        .filter(|(_, s)| s == "platform")
        .map(|(i, _)| *i)
        .collect();
    let wsr: Vec<Uuid> = roles
        .iter()
        .filter(|(_, s)| s == "workspace")
        .map(|(i, _)| *i)
        .collect();

    // Objects (ids need no backing row: grants/decisions key on the id only).
    let kinds = ["dashboard", "chart", "dataset", "folder"];
    let folder = Uuid::new_v4();
    let objects: Vec<ObjectRef> = (0..3)
        .map(|i| ObjectRef {
            kind: rng.pick(&kinds).to_string(),
            id: Uuid::new_v4(),
            folder_id: (i % 2 == 0).then_some(folder),
            workspace_id: ws[rng.below(2)],
        })
        .collect();

    // Grants.
    for u in &users {
        for _ in 0..rng.below(5) {
            let effect = if rng.chance(0.3) { "deny" } else { "allow" };
            match rng.below(3) {
                0 => {
                    sqlx::query(
                        "INSERT INTO access_grants \
                         (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
                         VALUES ('user', $1, $2, 'platform', NULL, $3) ON CONFLICT DO NOTHING",
                    )
                    .bind(u.0)
                    .bind(*rng.pick(&plat))
                    .bind(effect)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                }
                1 => {
                    sqlx::query(
                        "INSERT INTO access_grants \
                         (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
                         VALUES ('user', $1, $2, 'workspace', $3, $4) ON CONFLICT DO NOTHING",
                    )
                    .bind(u.0)
                    .bind(*rng.pick(&wsr))
                    .bind(rng.pick(&ws).0)
                    .bind(effect)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                }
                _ => {
                    let o = rng.pick(&objects);
                    let target = if o.folder_id.is_some() && rng.chance(0.5) {
                        (folder, "folder".to_string())
                    } else {
                        (o.id, o.kind.clone())
                    };
                    sqlx::query(
                        "INSERT INTO access_grants \
                         (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect) \
                         VALUES ('user', $1, $2, 'object', $3, $4, $5) ON CONFLICT DO NOTHING",
                    )
                    .bind(u.0)
                    .bind(*rng.pick(&wsr))
                    .bind(target.1)
                    .bind(target.0)
                    .bind(effect)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                }
            }
        }
    }

    // Questions: every user x every resource shape x a sample of actions.
    let mut resources: Vec<(String, Resource)> = vec![
        ("platform".into(), Resource::Platform),
        ("ws0".into(), Resource::Workspace(ws[0])),
        ("ws1".into(), Resource::Workspace(ws[1])),
    ];
    for (i, o) in objects.iter().enumerate() {
        resources.push((format!("obj{i}:{}", o.kind), Resource::Object(o.clone())));
    }
    let mut cases = Vec::new();
    for (ui, u) in users.iter().enumerate() {
        for (rname, res) in &resources {
            for _ in 0..14 {
                let (action, bucket) = rng.pick(&universe);
                cases.push(Case {
                    subject: Subject {
                        user_id: *u,
                        group_ids: vec![],
                    },
                    resource: res.clone(),
                    action: action.clone(),
                    note: format!("state{idx} user{ui} {rname} [{bucket}]"),
                });
            }
        }
    }
    cases
}

/// Deterministic regression cases independent of the RNG: the platform-admin
/// exemption of the admin-graph forbid, and its denial for everyone else.
async fn anchors(db: &Db) -> Vec<Case> {
    let admin = seed_user(db).await;
    let plain = seed_user(db).await;
    let w = seed_workspace(db).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
         WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(admin.0)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'workspace', $2 FROM roles \
         WHERE scope = 'workspace' AND name = 'ws_admin'",
    )
    .bind(plain.0)
    .bind(w.0)
    .execute(&db.pool)
    .await
    .unwrap();
    let mut out = Vec::new();
    for (who, u) in [("admin", admin), ("plain", plain)] {
        for a in ADMIN_GRAPH_ACTIONS {
            out.push(Case {
                subject: Subject {
                    user_id: u,
                    group_ids: vec![],
                },
                resource: Resource::Platform,
                action: a.to_string(),
                note: format!("anchor {who} admin-graph"),
            });
        }
        for (res, tag) in [
            (Resource::Workspace(w), "ws"),
            (Resource::Platform, "platform"),
        ] {
            for a in ["dashboard:read", "engine:create", "workspace:read"] {
                out.push(Case {
                    subject: Subject {
                        user_id: u,
                        group_ids: vec![],
                    },
                    resource: res.clone(),
                    action: a.to_string(),
                    note: format!("anchor {who} {tag}"),
                });
            }
        }
    }
    out
}

/// Compare both template sets over `cases`; returns how many were compared.
async fn compare(db: &Db, pdp: &CedarPdp, cases: &[Case], diffs: &mut Vec<String>) -> usize {
    // Both sets are pure over the current DB: compute once per batch.
    let (a_set, a_names) = CedarPdp::compiled_policy_set_for_test(db).await.unwrap();
    let ir =
        egret_cedar::compile::compile(&egret_cedar::load::load_inputs(db).await.unwrap()).unwrap();
    for c in cases {
        let today = pdp
            .decide_with_for_test(&a_set, &a_names, &c.subject, &c.action, &c.resource)
            .await
            .unwrap();
        let compiled = pdp
            .decide_with_for_test(
                &ir.policy_set,
                &ir.role_names,
                &c.subject,
                &c.action,
                &c.resource,
            )
            .await
            .unwrap();
        if today.allow != compiled.allow {
            diffs.push(format!(
                "[{}] action={} pdp={} egret-cedar={}",
                c.note, c.action, today.allow, compiled.allow
            ));
        }
    }
    cases.len()
}

#[tokio::test]
async fn egret_cedar_decisions_agree_with_the_pdp() {
    let Some(db) = connect().await else {
        if std::env::var("EGRET_ALLOW_DB_SKIP").is_ok() {
            eprintln!("skip: no DB");
            return;
        }
        panic!("no DB and EGRET_ALLOW_DB_SKIP unset");
    };
    let bus = EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.expect("pdp");
    let seed: u64 = std::env::var("EGRET_CEDAR_DIFF_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SEED);
    let mut rng = Rng(seed);

    let mut diffs: Vec<String> = Vec::new();
    let mut total = 0;
    let mut allows = 0usize;
    for s in 0..STATES {
        let cases = draw_state(&db, &mut rng, s).await;
        total += compare(&db, &pdp, &cases, &mut diffs).await;
    }
    let anchor_cases = anchors(&db).await;
    total += compare(&db, &pdp, &anchor_cases, &mut diffs).await;
    // Non-vacuity: the corpus must contain both outcomes.
    let (set, names) = CedarPdp::compiled_policy_set_for_test(&db).await.unwrap();
    for c in &anchor_cases {
        if pdp
            .decide_with_for_test(&set, &names, &c.subject, &c.action, &c.resource)
            .await
            .unwrap()
            .allow
        {
            allows += 1;
        }
    }
    assert!(
        allows > 0 && allows < anchor_cases.len(),
        "anchors must mix allow and deny"
    );
    eprintln!(
        "cedar_differential: {total} cases compared, {} anchors allowed of {}",
        allows,
        anchor_cases.len()
    );
    assert!(
        diffs.is_empty(),
        "egret-cedar diverged from the PDP on {} case(s):\n{}\n(reproduce with EGRET_CEDAR_DIFF_SEED={seed})",
        diffs.len(),
        diffs.join("\n"),
    );
}
