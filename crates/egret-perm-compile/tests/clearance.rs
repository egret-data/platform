//! SP4c **Task 2**: a classification's CLEARANCE must be refused at the ENGINE,
//! not only when someone clicks the object.
//!
//! The shape is what D0 measured, and it is not the obvious one. A Ranger policy
//! item naming a ROLE matches nobody on the Trino path egret provisions, and an
//! unmatched item in `denyExceptions` excepts NOBODY — so *deny `{public}`
//! except role `analyst`* would have refused the tagged column to every user
//! INCLUDING the cleared ones. The clearing role is therefore expanded into its
//! members' STORED usernames at compile time
//! (`docs/en/plans/2026-09-28-sp4c-d0-result.md`).
//!
//! Every assertion here is value-level, not shape-level: WHICH tag the deny
//! names, WHICH usernames are excepted, and — for the collision case — that the
//! username is read rather than re-derived. A shape-level test passes against
//! every defect this file exists to catch.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test clearance
//!
//! **Scoping.** A clearance is installation-wide, exactly like a Trino tag rule:
//! every gated tag in the database compiles into every Trino profile that has a
//! tag service. So a concurrently running test's gated tag also lands in this
//! test's profile and its skip also appears in this test's summary — every
//! assertion below is therefore keyed to the `(service_id, name)` pair or the
//! `tags.id` this test itself created, never to a count of everything.

use egret_metastore::Db;
use egret_perm::eval::evaluate;
use egret_perm::{
    Action, ClearanceGate, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind,
    PrincipalSet, Reason, Scope, Subject as EvalSubject,
};
use egret_perm_compile::{recompile_service, SourceKind};
use egret_policy::{clearance_gates_for_tags_via, PolicyStore, COMPILER_CLEARANCE_PREFIX};
use serde_json::Value;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    perm: PermStore,
    policy: PolicyStore,
    profile_id: i64,
    tag_service_id: i64,
}

/// A Trino profile with a real tag service — the only profile shape that can
/// host a tag policy at all. No catalog and no engine binding: a clearance
/// carries no catalog resource (the tag plugin resolves the tag wherever it is
/// assigned), so nothing here needs one.
async fn seed(db: &Db) -> Fixture {
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(db.pool.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_clr");
    let tag_service_name = format!("{profile_name}_tag");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create trino profile");

    Fixture {
        perm,
        policy,
        profile_id: profile.id,
        tag_service_id: tag_service.id,
    }
}

/// A tag, returning `(id, name)`.
async fn new_tag(db: &Db) -> (Uuid, String) {
    let key = unique("clr-tag");
    let name = unique("ClearanceTag");
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(&key)
        .bind(&name)
        .fetch_one(&db.pool)
        .await
        .expect("insert tag");
    (id, name)
}

/// Bind an engine to `service_id`, which is what makes a profile
/// COMPILER-TARGETED. `compiler_targeted_services_via` joins
/// `engine_mgmt.engines` on `authz_service_id`, so a profile with no engine is
/// reached by `recompile_service` called by name but by NO affected-service
/// trigger — which is why `seed()` alone is not enough for a test that exercises
/// a trigger rather than calling the recompile itself.
async fn bind_engine(db: &Db, service_id: i64) {
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2)",
    )
    .bind(unique("clr-eng"))
    .bind(service_id)
    .execute(&db.pool)
    .await
    .expect("bind engine");
}

/// A platform role, returning its id.
async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

/// Insert a user. `username` of `None` leaves the column NULL, which is what
/// every user-insert path in egret actually does.
async fn new_user(db: &Db, email: &str, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,$2,$3) RETURNING id",
    )
    .bind(email)
    .bind("Test User")
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

/// Grant `role_id` to `user_id` platform-wide, the way the access-control
/// console does.
async fn grant_role(db: &Db, user_id: Uuid, role_id: Uuid) {
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(user_id)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");
    // A helper that silently inserted nothing would turn every assertion below
    // into a test of the no-holders path wearing the name of the cleared path.
    assert_eq!(res.rows_affected(), 1, "the role grant row must exist");
}

/// Gate `tag_id` on `roles` (names) and `groups` (ids) — the row
/// `ClassificationPolicyRepo::set_policy` writes.
async fn gate(db: &Db, tag_id: Uuid, roles: &[&str], groups: &[Uuid]) {
    let roles: Vec<String> = roles.iter().map(|r| r.to_string()).collect();
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles, \
                                            required_groups = EXCLUDED.required_groups",
    )
    .bind(tag_id)
    .bind(&roles)
    .bind(groups)
    .execute(&db.pool)
    .await
    .expect("classification policy");
}

/// `tags.key` for `tag_id` — `new_tag` returns `(id, name)` because most of
/// this file addresses a tag by its NAME (what a Ranger tag policy resource
/// matches); the eval-agreement test below needs the KEY instead, the
/// identifier `tag_keys_for_resource_via`/`clearance_gates_for_tags_via` and
/// `egret_perm::eval`'s `tags_on` all key on.
async fn tag_key(db: &Db, tag_id: Uuid) -> String {
    sqlx::query_scalar("SELECT key FROM tags WHERE id = $1")
        .bind(tag_id)
        .fetch_one(&db.pool)
        .await
        .expect("tag key")
}

fn clearance_name(tag_id: Uuid) -> String {
    format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}")
}

/// The clearance policy for `tag_id` under EXACTLY `service_id`, or `None`.
/// Asking about a NAMED service is the point: a tag policy filed under the
/// resource profile deploys, is listed by egret, and is served to nobody
/// (SP4a Task 10, case T3).
async fn clearance_policy(db: &Db, service_id: i64, tag_id: Uuid) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(clearance_name(tag_id))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

fn users_of(item: &Value) -> Vec<String> {
    item["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|v| v.as_str().expect("username").to_string())
        .collect()
}

/// The Ranger access types one policy item carries, in order.
fn access_types(item: &Value) -> Vec<String> {
    item["accesses"]
        .as_array()
        .expect("accesses")
        .iter()
        .map(|a| a["type"].as_str().expect("type").to_string())
        .collect()
}

fn strings_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_str().expect("string").to_string())
        .collect()
}

/// **The headline of SP4c.** A gated tag compiles to a tag-service DENY whose
/// `denyExceptions` name the clearing role's members by the username the engine
/// actually sees — and the deny half names `{public}`, the one "everybody" D0
/// measured is expressible.
#[tokio::test]
async fn a_gated_tag_compiles_to_a_deny_excepting_its_cleared_members() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, tag_name) = new_tag(&db).await;

    let role_name = unique("clearance-high");
    let role_id = new_role(&db, &role_name).await;
    let cleared = new_user(
        &db,
        &format!("{}@egret.local", unique("cleared")),
        Some("clr-cleared"),
    )
    .await;
    let uncleared = new_user(
        &db,
        &format!("{}@egret.local", unique("uncleared")),
        Some("clr-uncleared"),
    )
    .await;
    grant_role(&db, cleared, role_id).await;
    let _ = uncleared; // deliberately holds no clearing role

    gate(&db, tag_id, &[&role_name], &[]).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    let body = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("a role-gated tag must compile to a clearance policy in the TAG service");
    assert_eq!(
        clearance_policy(&db, f.profile_id, tag_id).await,
        None,
        "a serviceType:tag policy filed under the resource profile is served to nobody"
    );

    // The tag it withholds: the tag's NAME, which is what egret's tag download
    // serves as the tag type.
    assert_eq!(body["serviceType"], "tag");
    assert_eq!(body["policyType"], 0);
    assert_eq!(
        strings_of(&body["resources"]["tag"]["values"]),
        vec![tag_name]
    );

    // It withholds from everybody…
    let deny_items = body["denyPolicyItems"].as_array().expect("denyPolicyItems");
    assert_eq!(deny_items.len(), 1, "one deny item: {body:#}");
    assert_eq!(
        strings_of(&deny_items[0]["groups"]),
        vec!["public".to_string()],
        "the deny half must name {{public}} — the only 'everybody' Ranger resolves with no \
         group provider"
    );
    assert!(
        users_of(&deny_items[0]).is_empty()
            && deny_items[0]["roles"].as_array().unwrap().is_empty(),
        "the deny item names no user and no ROLE — a role name matches nobody (D0): {body:#}"
    );
    assert_eq!(
        access_types(&deny_items[0]),
        vec![
            "select".to_string(),
            "insert".to_string(),
            "delete".to_string()
        ],
        "a classification withholds the DATA, not only the SELECT — an uncleared user who could \
         still INSERT would be writing into data they cannot see"
    );

    // …except the cleared, named by their STORED username.
    let exceptions = body["denyExceptions"].as_array().expect("denyExceptions");
    assert_eq!(exceptions.len(), 1, "one exception item: {body:#}");
    assert_eq!(
        users_of(&exceptions[0]),
        vec!["clr-cleared".to_string()],
        "the clearing role's members are enumerated, and nobody else is"
    );
    assert_eq!(
        access_types(&exceptions[0]),
        access_types(&deny_items[0]),
        "an exception that does not cover the denied access types excepts nothing for it"
    );
    assert!(
        exceptions[0]["roles"].as_array().unwrap().is_empty(),
        "naming the ROLE here excepts NOBODY and the deny stands for everyone (D0) — that is \
         the whole reason members are enumerated: {body:#}"
    );

    // Two shapes that would quietly make the whole thing inert.
    assert!(
        body["policyItems"]
            .as_array()
            .expect("policyItems")
            .is_empty(),
        "a clearance is a DENY only — an allow on the tag service short-circuits ahead of every \
         compiled resource policy: {body:#}"
    );
    assert_eq!(
        body["isDenyAllElse"], false,
        "isDenyAllElse blanks denyPolicyItems AND denyExceptions, leaving a deny nobody can be \
         excepted from"
    );
}

/// **The invisible failure.** `ensure_username` resolves a slug collision by
/// appending `-2`, `-3`, … so two humans whose email local parts slug to the
/// same base are `bob` and `bob-2` inside the engine. A compiler that
/// RE-DERIVED the username would put `bob` in `denyExceptions`: the policy
/// compiles, deploys, is listed live by egret — and the cleared person is
/// refused the column while an uncleared one is let through, with nothing
/// anywhere to say so. A user whose username happens to EQUAL their email slug
/// passes either way, which is why the cleared holder here is the suffixed one.
#[tokio::test]
async fn a_collision_suffixed_username_is_excepted_not_the_base_slug() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _) = new_tag(&db).await;

    let local = unique("dupe");
    let first = new_user(&db, &format!("{local}@egret.local"), Some(&local)).await;
    let second = new_user(
        &db,
        &format!("{local}@other.local"),
        Some(&format!("{local}-2")),
    )
    .await;
    let role_name = unique("clearance-dupe");
    let role_id = new_role(&db, &role_name).await;
    // The SECOND one — the `-2` — is the cleared holder.
    grant_role(&db, second, role_id).await;
    let _ = first;

    gate(&db, tag_id, &[&role_name], &[]).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    let body = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("policy");
    assert_eq!(
        users_of(&body["denyExceptions"].as_array().expect("denyExceptions")[0]),
        vec![format!("{local}-2")],
        "the STORED username must be read verbatim — a re-derivation would except {local:?}, \
         clearing the wrong person and refusing the right one"
    );
}

/// D7 measured that a Ranger item naming a GROUP matches nobody. A tag gated
/// ONLY by `required_groups` therefore compiles to NOTHING — and the skip is
/// REPORTED, not silent: "no policy exists" is also what a compiler that never
/// looked at classifications at all would produce, so the assertion that
/// discriminates is that the recompile SAID it skipped this tag.
#[tokio::test]
async fn a_groups_only_gated_tag_compiles_to_nothing_and_reports_one_skip() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _) = new_tag(&db).await;

    let group_id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(unique("clr-group"))
        .fetch_one(&db.pool)
        .await
        .expect("group");
    gate(&db, tag_id, &[], &[group_id]).await;

    let summary = recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    assert_eq!(
        clearance_policy(&db, f.tag_service_id, tag_id).await,
        None,
        "a group-only gate must compile to no policy — a deny whose exceptions except nobody \
         withholds the column from everybody"
    );
    assert_eq!(
        clearance_policy(&db, f.profile_id, tag_id).await,
        None,
        "and not to the resource profile either"
    );

    // Scoped to THIS test's tag: another test's gated tag is compiled by this
    // same installation-wide pass and reports its own skips.
    let mine: Vec<&egret_perm_compile::SkippedItem> =
        summary.skipped.iter().filter(|s| s.id == tag_id).collect();
    assert_eq!(
        mine.len(),
        1,
        "the skip must be REPORTED, not silent — got {:?}",
        summary.skipped
    );
    assert_eq!(mine[0].source_kind, SourceKind::Clearance);
    assert!(
        mine[0].reason.contains("required_groups"),
        "the reason must name what was skipped and why: {:?}",
        mine[0].reason
    );
}

/// A clearance that cannot be LIFTED is a one-way door. Removing the gate must
/// reap the deny on the next recompile — which only happens if the clearance
/// name prefix is in `apply_diff_in_tx`'s existing-state sweep.
#[tokio::test]
async fn lifting_a_clearance_reaps_the_deny() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _) = new_tag(&db).await;

    let role_name = unique("clearance-lift");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("lift")),
        Some("clr-lift"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name], &[]).await;

    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");
    assert!(
        clearance_policy(&db, f.tag_service_id, tag_id)
            .await
            .is_some(),
        "the gate must compile before removing it can prove anything"
    );

    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(tag_id)
        .execute(&db.pool)
        .await
        .expect("clear policy");
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile after lifting");

    assert_eq!(
        clearance_policy(&db, f.tag_service_id, tag_id).await,
        None,
        "the lifted clearance's deny must be reaped, not enforced forever"
    );
}

/// **SP4c Task 4/D4.** egret's own ORACLE must give a subject blocked by
/// clearance its own reason, and that reason must AGREE with the policy
/// actually compiled — not merely say something plausible. This test ties both
/// to the SAME artifact: it reads the real `denyExceptions` usernames off the
/// compiled policy (exactly as the first test in this file does) and then calls
/// `egret_perm::eval::evaluate` for the cleared username and the uncleared one,
/// checking the decision matches which side of that list each username is
/// actually on.
///
/// The grant used here would ALLOW the read on its own (D1: a clearance must
/// win over any resource grant, not merely apply where none exists), so an
/// `evaluate` that forgot to check clearance first would report
/// `AllowedByGrant` for the uncleared subject.
///
/// **Renamed in SP4d-2a Task 5 (was
/// `explain_and_the_compiled_policy_agree_about_a_blocked_subject`).**
/// `/perm/explain` no longer answers with `evaluate`; it evaluates the compiled
/// artifact the engine downloads, and on StarRocks a clearance is a per-column
/// resource DENY in that artifact. So this test is now about the oracle and the
/// compiler agreeing, which is still worth pinning — the endpoint's own
/// clearance assertion moved to `routes.rs`'s
/// `a_clearance_deny_in_the_artifact_refuses_the_uncleared_and_not_the_cleared`.
#[tokio::test]
async fn the_oracle_and_the_compiled_policy_agree_about_a_blocked_subject() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _tag_name) = new_tag(&db).await;
    let key = tag_key(&db, tag_id).await;

    let role_name = unique("clearance-explain");
    let role_id = new_role(&db, &role_name).await;
    let cleared_user = new_user(
        &db,
        &format!("{}@egret.local", unique("expl-cleared")),
        Some("expl-cleared"),
    )
    .await;
    let uncleared_user = new_user(
        &db,
        &format!("{}@egret.local", unique("expl-uncleared")),
        Some("expl-uncleared"),
    )
    .await;
    grant_role(&db, cleared_user, role_id).await;
    let _ = uncleared_user;

    gate(&db, tag_id, &[&role_name], &[]).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    // The real artifact: who the compiled deny actually excepts.
    let body = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("clearance policy must exist");
    let excepted = users_of(&body["denyExceptions"].as_array().expect("denyExceptions")[0]);
    assert_eq!(
        excepted,
        vec!["expl-cleared".to_string()],
        "sanity: the compiled deny excepts exactly the cleared username"
    );

    // The same clearance gate, resolved the way `explain`'s route resolves
    // it — through the store function, not re-derived from `gate()`'s inputs.
    let mut conn = db.pool.acquire().await.expect("conn");
    let gates = clearance_gates_for_tags_via(&mut conn, std::slice::from_ref(&key))
        .await
        .expect("clearance gates");
    assert_eq!(gates.len(), 1, "one gated tag: {gates:?}");
    let clearances: Vec<ClearanceGate> = gates
        .into_iter()
        .map(|(tag, required_roles)| ClearanceGate {
            tag,
            required_roles,
        })
        .collect();

    // A grant that would allow the read on its own, standing in for the
    // resource grant an uncleared subject might otherwise hold — D1 requires
    // clearance to win over it, not merely apply in its absence.
    let allow_everyone = Grant {
        id: Uuid::new_v4(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: Uuid::nil(),
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::Group,
                r#ref: "public".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let target = Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: Some("ssn".into()),
    };
    let tags_on = vec![key.clone()];

    let cleared_subject = EvalSubject {
        user: "expl-cleared".into(),
        groups: vec!["public".into()],
        roles: vec![role_name.clone()],
        workspaces: vec![],
    };
    let cleared_decision = evaluate(
        &cleared_subject,
        Action::Read,
        &target,
        std::slice::from_ref(&allow_everyone),
        &[],
        &tags_on,
        &clearances,
    );
    assert!(
        matches!(
            cleared_decision.reasons.as_slice(),
            [Reason::AllowedByGrant { .. }]
        ),
        "the cleared subject, whose username IS in the compiled denyExceptions, must be allowed \
         by the underlying grant, not blocked by clearance: {cleared_decision:?}"
    );

    let uncleared_subject = EvalSubject {
        user: "expl-uncleared".into(),
        groups: vec!["public".into()],
        roles: vec![],
        workspaces: vec![],
    };
    let uncleared_decision = evaluate(
        &uncleared_subject,
        Action::Read,
        &target,
        std::slice::from_ref(&allow_everyone),
        &[],
        &tags_on,
        &clearances,
    );
    assert!(
        !uncleared_decision.allowed,
        "the uncleared subject, whose username is NOT in the compiled denyExceptions, must be \
         refused: {uncleared_decision:?}"
    );
    match uncleared_decision.reasons.as_slice() {
        [Reason::ClearanceRequired {
            tag,
            required_roles,
        }] => {
            assert_eq!(tag, &key, "the reason must name the gating tag");
            assert_eq!(
                required_roles,
                &vec![role_name.clone()],
                "the reason must name the role that would clear it"
            );
        }
        other => panic!(
            "expected ClearanceRequired, not the pre-SP4c answers (no_grant_matched would say \
             nobody needs to add a grant — one exists; explicit_deny would collapse this into \
             ordinary policy) — got {other:?}"
        ),
    }
}

/// **SP4c review C1 — a Critical this branch introduced, in the fail-OPEN
/// direction.** Deleting a clearing role is a role-MEMBERSHIP write wearing a
/// different name: `access_grants.role_id` is `ON DELETE CASCADE` (migration
/// `0068_access_grants.sql`), so one `DELETE FROM roles` revokes the clearance
/// from every holder at once.
///
/// Before the fix, `RoleRepo::delete` ran that statement with no recompile. An
/// admin deleting a clearing role to revoke a clearance therefore got: every
/// `access_grants` row gone, Cedar closing control-plane object access — and the
/// compiled `denyExceptions` still naming the ex-holders, who went on reading
/// the classified column through SQL Lab and notebooks with nothing in the
/// system converging it. That is the union-not-intersection defect SP4c exists
/// to close, re-opened one write over. The enumeration that reviewed this path
/// labelled it "fails closed": true of the eventual state, false of the
/// unbounded interim, and no process was ever going to close the interim.
///
/// The assertion is on the ARTIFACT — the compiled `denyExceptions` in
/// `policy.authz_policies`, the body a Ranger plugin downloads — not on
/// `access_grants`, which was always written correctly and is exactly what made
/// the defect invisible in egret's own UI.
#[tokio::test]
async fn deleting_a_clearing_role_reaps_the_ex_holders_from_deny_exceptions() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _tag_name) = new_tag(&db).await;

    let role_name = unique("clearance-deleted");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("clr-exholder")),
        Some("clr-exholder"),
    )
    .await;
    grant_role(&db, holder, role_id).await;
    gate(&db, tag_id, &[&role_name], &[]).await;
    // The delete goes through a TRIGGER, not a recompile called by name, so the
    // profile has to be reachable by `affected_services_for_role_membership` —
    // which means an engine bound to it.
    bind_engine(&db, f.profile_id).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    // Ground truth FIRST: without this the test below passes against a
    // clearance that never named the holder at all.
    let before = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("a role-gated tag must compile to a clearance policy");
    assert_eq!(
        users_of(&before["denyExceptions"].as_array().expect("denyExceptions")[0]),
        vec!["clr-exholder".to_string()],
        "sanity: the compiled deny excepts the holder before the role is deleted"
    );

    // The write under test. Not `RoleRepo::delete` — that is the method with no
    // recompile, kept for callers with no engine consequence.
    egret_perm_compile::delete_platform_role_and_recompile(&db.pool, role_id)
        .await
        .expect("delete the clearing role");

    // The cascade did what the migration says.
    let grants_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM access_grants WHERE principal_id = $1")
            .bind(holder)
            .fetch_one(&db.pool)
            .await
            .expect("count grants");
    assert_eq!(
        grants_left, 0,
        "sanity: deleting the role cascaded its grants away"
    );

    // And the ENGINE agrees, in the same transaction the delete committed in.
    let after = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("the clearance policy must still exist — the TAG is still gated");
    let excepted: Vec<String> = after["denyExceptions"]
        .as_array()
        .expect("denyExceptions")
        .iter()
        .flat_map(users_of)
        .collect();
    assert!(
        !excepted.contains(&"clr-exholder".to_string()),
        "the ex-holder is still excepted from the deny, so they can still read the classified \
         column after their clearance was deleted: {after:#}"
    );
    assert!(
        excepted.is_empty(),
        "nobody holds the clearing role now, so the deny must except nobody — a deny standing \
         for everyone is the correct compilation of 'nobody is cleared': {after:#}"
    );
}

/// `users.username` for `user_id`, NULL as `None`.
async fn stored_username(db: &Db, user_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("read username")
}

/// **2026-10-18 design R5, R6.** A clearing-role holder with no username yet
/// was left out of `denyExceptions`, so the column was refused to someone
/// Cedar clears (`classification_forbid` reads the role, not the username)
/// until some later recompile. The compile now mints their username and
/// excepts them at once.
#[tokio::test]
async fn a_clearing_role_holder_with_no_username_is_minted_into_deny_exceptions() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (tag_id, _) = new_tag(&db).await;

    let role_name = unique("clearance-mint");
    let role_id = new_role(&db, &role_name).await;
    let holder = new_user(&db, &format!("{}@egret.local", unique("clr-mint")), None).await;
    grant_role(&db, holder, role_id).await;

    gate(&db, tag_id, &[&role_name], &[]).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");

    let minted = stored_username(&db, holder)
        .await
        .expect("compiling the clearance must mint the clearing role holder's username");
    let body = clearance_policy(&db, f.tag_service_id, tag_id)
        .await
        .expect("policy");
    let exceptions = body["denyExceptions"].as_array().expect("denyExceptions");
    assert_eq!(exceptions.len(), 1, "one exception item: {body:#}");
    assert_eq!(
        users_of(&exceptions[0]),
        vec![minted],
        "the holder is excepted by the username the compile minted: {body:#}"
    );
}

/// R5's other half: `view_audit::owner_cleared` is served on GET routes, so it
/// reads and never mints. A holder still without a username stays NULL, and
/// the answer is what it was before — a username nobody carries is not a
/// holder, even the one the holder WOULD be minted.
///
/// The gate is written in a transaction that is never committed, and the audit
/// runs on that same connection. A committed gate names R in the model, so any
/// sibling test's recompile in this binary would legitimately mint R's holder
/// (the compile half of R5) before the assertion below — a flake that blamed
/// the audit for another test's compile.
#[tokio::test]
async fn owner_cleared_does_not_mint_a_holders_username() {
    let Some(db) = test_db().await else { return };
    let (tag_id, _) = new_tag(&db).await;

    let role_name = unique("clearance-audit");
    let role_id = new_role(&db, &role_name).await;
    let local = unique("clr-audit");
    let holder = new_user(&db, &format!("{local}@egret.local"), None).await;
    grant_role(&db, holder, role_id).await;

    let mut tx = db.pool.begin().await.expect("tx");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}'::uuid[])",
    )
    .bind(tag_id)
    .bind(vec![role_name.clone()])
    .execute(&mut *tx)
    .await
    .expect("classification policy, uncommitted");

    let answer = egret_perm_compile::view_audit::owner_cleared(
        &mut tx,
        &local,
        &std::collections::BTreeSet::from([tag_id]),
    )
    .await
    .expect("owner_cleared");
    let username: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(holder)
            .fetch_one(&mut *tx)
            .await
            .expect("read username");
    tx.rollback().await.expect("rollback");

    assert_eq!(
        username, None,
        "a GET-served audit must not write `users.username`"
    );
    assert_eq!(
        answer,
        Some(false),
        "with no username minted, no owner name matches the holder — minting here would have \
         answered true for {local:?}"
    );
}
