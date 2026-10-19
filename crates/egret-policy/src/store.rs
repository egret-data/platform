use crate::PolicyStoreError;
use egret_metastore::Db;
use egret_policy_api::infra_names::is_egret_infrastructure_policy;
use egret_policy_api::TAG_SERVICE_CONFIG_KEY;
use egret_ranger_eval::resource::MatchType;
use serde_json::Value;

#[derive(Clone)]
pub struct PolicyStore {
    db: Db,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ServiceDefRow {
    pub id: i64,
    pub name: String,
    pub def_jsonb: Value,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ServiceRow {
    pub id: i64,
    pub name: String,
    #[sqlx(rename = "type")]
    pub r#type: String,
    pub config_jsonb: Value,
    pub policy_version: i64,
    /// Monotonic tag-assignment version; the Ranger tag-download plugin polls this
    /// (parallel to `policy_version`) to know when to re-pull catalog tags.
    pub tag_version: i64,
    /// `"manual"` (hand-authored through the policy editor, the default — see
    /// migration 0155) or `"generated"` (owned by SP4a's permission-model
    /// compiler, `egret-perm-compile::trigger::apply_diff_in_tx`, which stamps
    /// this the moment it first compiles a policy for the service).
    ///
    /// Task 8 fix round 2 (I-1): the guard this drives is scoped PER POLICY,
    /// not per profile — `create_policy` and `seed_default_trino_policies`
    /// refuse outright once a profile is `"generated"` (see
    /// [`refuse_if_generated`]; adding brand-new content to a compiled
    /// profile is the confusion Task 8 exists to prevent), but
    /// `update_policy`/`delete_policy`/`set_policy_enabled` only refuse a
    /// SPECIFIC policy whose own name is compiler-owned (see
    /// [`refuse_if_compiler_owned`] and [`is_compiler_owned_policy_name`]) —
    /// a policy the compiler's sweep never touches is not protected by
    /// "read-only", since read-only-ness exists only to guard what the
    /// compiler would otherwise silently overwrite. `delete_service` mirrors
    /// that: refused only while a compiler-owned policy still exists under
    /// it (see [`refuse_if_compiler_owned_policies_remain`]), not merely
    /// because the profile is (or was ever) `"generated"`.
    pub origin: String,
}

/// One catalog resource → tag pairing for a service's warehouse(s), the row
/// feeding `ServiceTags::from_assignments`. Column order matches the tuple the
/// builder consumes: `(catalog, schema, table, column, tag_id, tag_name)`.
#[derive(Debug, sqlx::FromRow)]
pub struct TagAssignmentRow {
    pub catalog: String,
    pub schema_name: String,
    pub table_name: String,
    pub column_name: String,
    pub tag_id: String,
    pub tag_name: String,
}

/// Postgres' `23505 unique_violation`, whichever index raised it.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Name prefix every ACCESS policy SP4a's permission-model compiler
/// (`egret-perm-compile::grants::compile_grant`) produces —
/// `format!("{COMPILER_GRANT_PREFIX}{}", grant.id)`.
///
/// Defined HERE, not in `egret-perm-compile`, even though that crate is the
/// one that actually mints names with it: `egret-perm-compile` depends on
/// `egret-policy` (for `PolicyStore` and the Ranger JSON renderers), never
/// the reverse, so a constant only `egret-perm-compile` owned could not be
/// consumed by this crate's own guards. `egret-perm-compile`'s
/// `trigger::apply_diff_in_tx` (the sweep that reaps any
/// `policy.authz_policies` row under this prefix that its current compile no
/// longer produces) and `grants.rs`'s name-builder both import THIS constant
/// rather than keeping their own copy — see that crate's `trigger.rs` module
/// doc. Two independent copies of this string would drift, and a drift makes
/// a policy the sweep reaps look editable to the guards below, or the
/// reverse.
pub const COMPILER_GRANT_PREFIX: &str = "perm-grant-";
/// Name prefix every MASKING/ROW-FILTER policy the compiler
/// (`egret-perm-compile::rules::compile_rule`) produces. Same rationale as
/// [`COMPILER_GRANT_PREFIX`].
pub const COMPILER_RULE_PREFIX: &str = "perm-rule-";
/// Name prefix every CLEARANCE policy the compiler
/// (`egret-perm-compile::clearance::compile_clearance`) produces —
/// `format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}")`, one per gated tag. Same
/// rationale for living here as [`COMPILER_GRANT_PREFIX`].
///
/// **This one is a tag ACCESS policy, and that AMENDS SP4a's D2** ("tags drive
/// rules only, never access" — see `admin`'s re-export of
/// `tag_access_to_ranger_json`, which stayed unreachable until SP4c, and
/// `egret-perm-compile::clearance`'s module doc for the whole argument). A
/// classification exists to WITHHOLD data, and neither of D2's permitted tag
/// shapes can do that: a mask and a row filter both still answer the query. So
/// a clearance compiles to a tag-service DENY whose `denyExceptions` name the
/// cleared users. A future reader who finds a compiled tag access policy under
/// this prefix is looking at that amendment, not at a bug.
///
/// It needs a prefix of its OWN rather than riding on the grant prefix because
/// its name is keyed by `tags.id`, not by a `perm.grants` id: the editor's
/// reserved-name guard and `apply_diff_in_tx`'s existing-state sweep both key
/// on these prefixes, and a clearance filed under `perm-grant-` would be
/// indistinguishable from a grant whose row no longer exists.
pub const COMPILER_CLEARANCE_PREFIX: &str = "perm-clearance-";
/// Name prefix every ENGINE GRANT policy the compiler
/// (`egret-perm-compile::engine_grants::compile_engine_grant`) produces —
/// `format!("{COMPILER_ENGINE_GRANT_PREFIX}{engine_grant_id}")`, one per
/// `perm.engine_grants` row (F11, spec E7). Its own prefix for the reason the
/// clearance has one: its name is keyed by a `perm.engine_grants` id, and a
/// row filed under `perm-grant-` would read as a grant whose row is gone.
/// Migration 0195 renamed every hand-written policy that carried it before the
/// reservation (spec E19).
pub const COMPILER_ENGINE_GRANT_PREFIX: &str = "perm-engine-";

/// Whether `name` identifies a policy the SP4a permission-model compiler
/// owns (produced by `compile_grant`/`compile_rule`/`compile_clearance` and
/// reachable by `apply_diff_in_tx`'s existing-state sweep) — see
/// [`COMPILER_GRANT_PREFIX`] / [`COMPILER_RULE_PREFIX`] /
/// [`COMPILER_CLEARANCE_PREFIX`] / [`COMPILER_ENGINE_GRANT_PREFIX`].
pub fn is_compiler_owned_policy_name(name: &str) -> bool {
    name.starts_with(COMPILER_GRANT_PREFIX)
        || name.starts_with(COMPILER_RULE_PREFIX)
        || name.starts_with(COMPILER_CLEARANCE_PREFIX)
        || name.starts_with(COMPILER_ENGINE_GRANT_PREFIX)
}

/// SP4a Task 8 fix round 3: refuse a human write that would put (or leave) a
/// policy NAME under the compiler's reserved prefix (`COMPILER_GRANT_PREFIX`/
/// `COMPILER_RULE_PREFIX`) — in ANY profile, `manual` or `generated`. Pure
/// (no DB access): the fact under test is the string alone.
///
/// This exists because the profile/policy-origin guards above cannot close
/// the round-2 report's finding: a human names a policy `perm-grant-foo` in
/// a still-`manual` profile (`create_policy`'s origin check does not yet
/// fire — the profile isn't `generated` yet), and the FIRST real recompile
/// that ever reaches this service silently deletes it in the same
/// transaction that stamps the profile `generated` (`apply_diff_in_tx`'s
/// existing-state sweep matches on name alone, with or without a
/// `perm.compiled` provenance row — Task 7 fix round 1's own Critical fix,
/// which this reservation deliberately does NOT try to re-litigate: making
/// the collision UNREPRESENTABLE at the only human entry point is simpler
/// and strictly stronger than teaching the sweep to defend against it).
///
/// Deliberately does not require `origin = 'generated'`: a `manual` profile
/// is exactly the case the round-2 report showed getting silently
/// steamrolled, so gating this on `origin` would reopen the same hole.
fn refuse_reserved_name(name: &str) -> Result<(), PolicyStoreError> {
    if is_compiler_owned_policy_name(name) {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "policy name {name:?} starts with a prefix reserved for the SP4a permission model \
             ({COMPILER_GRANT_PREFIX:?} / {COMPILER_RULE_PREFIX:?} / \
             {COMPILER_CLEARANCE_PREFIX:?} / {COMPILER_ENGINE_GRANT_PREFIX:?}) — only its \
             compiler writes policies there; rename this policy instead of retrying"
        )));
    }
    Ok(())
}

/// A profile's `type`, read on the caller's connection (inside its transaction
/// where there is one), for the type-aware [`is_egret_infrastructure_policy`].
/// A missing profile is `NotFound`: every caller is about to write under it.
async fn service_type_of(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<String, PolicyStoreError> {
    sqlx::query_scalar::<_, String>("SELECT type FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("service id {service_id}")))
}

/// The infra writers' own guard: they stamp `managed_by = 'infra'`, so they may
/// only be given a name `egret_policy_api::infra_names` recognises on a profile
/// of `service_type`. A hard error, not a `debug_assert`, so the writer cannot
/// drift from the predicate. This is also what refuses a Trino-shaped
/// `all - …` name aimed at a StarRocks profile.
fn refuse_non_infrastructure_name(service_type: &str, name: &str) -> Result<(), PolicyStoreError> {
    if !is_egret_infrastructure_policy(service_type, name) {
        return Err(PolicyStoreError::BadRequest(format!(
            "policy name {name:?} is not an egret infrastructure policy name on a \
             {service_type:?} profile; refusing to stamp it managed_by = 'infra'"
        )));
    }
    Ok(())
}

/// SP4d-3 Task 2: the HUMAN paths' reservation of egret's infrastructure names.
/// A human create, or a rename INTO such a name, would mint an `'operator'` row
/// under a name [`is_egret_infrastructure_policy`] claims on this profile's
/// type, so the column and the predicate would disagree from the first write.
/// On a non-Trino profile an `all - …` name is an ordinary human name.
///
/// Its own variant rather than [`PolicyStoreError::CompilerOwned`]: that one's
/// message names the SP4a permission model's `perm-*` prefixes, and an operator
/// told to "rename this policy" about a prefix they never used would be misled.
/// Both map to 409 — the request is well-formed, the name belongs to someone
/// else. The infra writers do NOT call this; they call
/// [`refuse_non_infrastructure_name`], its inverse.
fn refuse_infrastructure_name(service_type: &str, name: &str) -> Result<(), PolicyStoreError> {
    if is_egret_infrastructure_policy(service_type, name) {
        return Err(PolicyStoreError::InfrastructureOwned(format!(
            "policy name {name:?} is reserved for egret's own plumbing (delegator and default \
             access policies) — choose a different name"
        )));
    }
    Ok(())
}

/// SP4d-3 Task 2: refuse a human `UPDATE`/`DELETE`/enable-flip of a policy
/// egret's own plumbing owns.
///
/// Keyed on `managed_by`, the COLUMN — deliberately not on the name.
/// [`refuse_if_compiler_owned`] answers a question about a NAME, which is the
/// right question for the compiler's reserved prefixes. This one answers a
/// question about a ROW, because the thing being protected is the row's
/// existence and content, and a rename must not move it out of the guard.
async fn refuse_if_infrastructure_owned(
    conn: &mut sqlx::PgConnection,
    policy_id: i64,
) -> Result<(), PolicyStoreError> {
    let managed_by: Option<String> =
        sqlx::query_scalar("SELECT managed_by FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .fetch_optional(&mut *conn)
            .await?;
    if managed_by.as_deref() == Some("infra") {
        return Err(PolicyStoreError::InfrastructureOwned(format!(
            "policy id {policy_id} is owned by egret's own plumbing (managed_by = 'infra') — \
             it is seeded or maintained by the platform, and editing it can break delegation \
             or lock every admin out of this service. Nothing in the console can change it; \
             if it is wrong, the producer that mints it has to change"
        )));
    }
    Ok(())
}

/// Whether `config` carries a [`TAG_SERVICE_CONFIG_KEY`] at all.
///
/// Deliberately the SAME predicate as Postgres' `config_jsonb ->> 'tagService'
/// IS NOT NULL`: the key is present AND its value is not JSON `null`. Migration
/// 0176's `CHECK` is written with that expression and this function is written
/// to match it, because the one failure this guard keeps reproducing in this
/// programme is **two guards that must agree, written to different shapes** —
/// here that would be a refusal keyed on `as_str()` (what `download_policies`
/// reads) beside a `CHECK` keyed on `->>` (which also answers for a number, a
/// bool, an object): a config of `{"tagService": 7}` would then pass one and
/// trip the other, and the route would answer 500 instead of 400.
///
/// `->>` on a non-object `config_jsonb` (a bare `5`, `"x"`, `[]`) yields NULL
/// rather than erroring — measured on the live PostgreSQL 16.15 — and
/// `Value::get` on a non-object likewise yields `None`, so the two agree there
/// too. Migration 0176's own clearing `UPDATE` is keyed on the SAME `->>`
/// expression, so it cannot leave behind a row its own `CHECK` then rejects, and
/// the `-` operator it applies is never reached for a scalar or array config
/// (measured: the `WHERE` excludes those rows, so `jsonb - text` never raises
/// "cannot delete from scalar").
fn carries_tag_service(config: &Value) -> bool {
    !matches!(config.get(TAG_SERVICE_CONFIG_KEY), None | Some(Value::Null))
}

/// SP4d debt Task 2: refuse a profile that pairs a non-Trino `type` with a
/// [`TAG_SERVICE_CONFIG_KEY`]. Pure (no DB access): the fact under test is the
/// type string and the config JSON alone.
///
/// # The measurement
///
/// [`crate::ranger::dto::ServiceTags::from_assignments`] (`egret-policy-api/src/dto.rs`)
/// writes the `resourceElements` keys `catalog` / `schema` / `table` / `column`,
/// hard-coded, and `download_tags` calls it with **no engine argument at all**.
/// Of the three seeded service-defs only `trino.json` declares a `schema`
/// resource; `starrocks.json`'s middle level is `database`
/// (`['catalog','database','table','column','view', …]`, measured against the
/// embedded JSON). Ranger's own `RangerTagEnricher.processServiceTags` calls
/// `createRangerServiceResourceMatcher`, which returns null when no hierarchy of
/// any policy type in the resource service-def contains every key in the
/// attachment — so for a StarRocks profile **every** attachment is DROPPED
/// (`EgretTagServiceResourceValidationTest` measures exactly that on the real
/// jar). The profile downloads the attachments, the enricher throws them away,
/// and every tag policy in its `tagPolicies` block is **silently inert**: no
/// masking, no row filtering, no error, nothing in a log.
///
/// Scoped to "only `trino` may carry one" rather than "`starrocks` may not",
/// because the property that makes it serveable is the def declaring a `schema`
/// resource, and a FOURTH service-def added without one must be refused by
/// default instead of inheriting the silence. A def added WITH a `schema`
/// resource would be wrongly refused here, which is why
/// `service_def_set_is_exactly_the_three_this_refusal_was_ruled_on` fails on an
/// ADDITION and not merely on a change. The per-engine resource-level
/// mapping (teaching everything that emits Ranger resource keys to take the
/// engine, or its service-def, as an input) is separate, deliberately
/// out-of-scope work: **block now, map later.** When it lands, this refusal and
/// migration 0176's `CHECK` are what it replaces.
///
/// # ⚠ Enumerated: every write path that can produce the state
///
/// The state is one row of `policy.authz_services` whose `type` is not `trino`
/// and whose `config_jsonb` carries a `tagService`. Measured by grepping every
/// `INSERT`/`UPDATE`/`DELETE` touching `policy.authz_services` across the
/// workspace, not by authorship:
///
/// | # | write path | writes `type`? | writes `config_jsonb`? | ruling |
/// |---|---|---|---|---|
/// | 1 | [`PolicyStore::create_service`] — the **only** `INSERT INTO policy.authz_services` in any `src/` | yes, caller-supplied | yes, caller-supplied | **guarded here**, first statement, before the insert |
/// | 1a | ↳ `create_profile`, `POST /policies/services` (`admin/mod.rs`) — arbitrary `type` AND arbitrary `config`, stored verbatim | — | — | the reachable surface; refused 400 via `PolicyStoreError::BadRequest` |
/// | 1a′ | ↳↳ the console's own caller, `frontend/src/api/policies.ts::createProfile(name, type)` | — | sends NO `config` at all | cannot reach the state; the population for 1a is a direct API caller (a script, `curl`, or a `config` field added to the form later) |
/// | 1b | ↳ `bind_trino_profile` (`egret-engine-mgmt/src/engines.rs`) — `("{name}_tag", "tag", {})` then `(name, "trino", {"tagService": …})` | — | — | passes: the only config-carrying call is `trino` |
/// | 1c | ↳ `PolicyPort::create_service` (`egret-policy-api`) — the cross-seam trait; `port_impl.rs` delegates straight to #1 | — | — | reached by #1's guard, no second copy |
/// | 2 | [`PolicyStore::delete_service`] | no | no | removes rows; cannot produce the state |
/// | 3 | the five `SET policy_version = policy_version + 1` bumps in this file, and `egret-perm-compile`'s `trigger.rs` | no | no | cannot |
/// | 4 | `egret_catalog_registry::tag_version`'s `SET tag_version = …` | no | no | cannot |
/// | 5 | `egret-perm-compile`'s `trigger.rs` `SET origin = 'generated'` | no | no | cannot |
/// | 6 | **an `UPDATE` of `type` or of `config_jsonb`** | — | — | **no such statement exists in any `src/` in this workspace.** So "change a profile's type to `starrocks` while it already carries a `tagService`" has no code path to refuse at today. Prose is not a bound, so the direction is closed in the SCHEMA instead: migration 0176's VALIDATED `CHECK` holds for every `INSERT` and every `UPDATE`, from Rust, from a future route, from `psql`, from a test's raw `sqlx::query` |
///
/// Row 6 is why this function is not the whole fix, and why a test that only
/// drives #1a would be a green from a population that cannot exercise the other
/// direction. `tests/store_services.rs` drives the raw `UPDATE`s directly.
///
/// ## Why the constraint is VALIDATED, and the key cleared, rather than `NOT VALID`
///
/// Rows 3 and 4 cannot CREATE the state, but they can be BLOCKED BY it, and that
/// is what ruled out leaving a pre-existing bad row dormant.
/// `egret_catalog_registry::tag_version::bump_bound_tag_versions` resolves every
/// profile bound to any engine on any catalog in a warehouse — its CTE filters on
/// `e.authz_service_id IS NOT NULL` and **not** on engine type — and bumps them in
/// ONE `UPDATE … WHERE id = ANY($1)`; `egret_perm_compile::trigger`'s bulk bump has
/// the same shape. Postgres enforces a `NOT VALID` CHECK on every UPDATE, so one
/// violating row in that set would make a catalog-tag write fail outright AND
/// leave a healthy Trino profile in the same warehouse un-bumped — all-or-nothing.
/// A dormant dead key turning into an outage on an unrelated healthy profile's
/// write is worse than deleting the key, so migration 0176 deletes it (the value
/// is provably inert — a non-Trino profile's own enricher discards the
/// attachments) and validates the constraint. **There is therefore no "legacy row"
/// population left**; do not reintroduce one by adding a `NOT VALID` variant.
///
/// **Falsified by** a fourth service-def, a second `INSERT INTO
/// policy.authz_services`, or an `UPDATE` of `type`/`config_jsonb` appearing in
/// `src/` — the first two must be re-ruled here, the third must come with its
/// own call to this function on top of the `CHECK`.
fn refuse_tag_service_on_non_trino_profile(
    typ: &str,
    config: &Value,
) -> Result<(), PolicyStoreError> {
    if typ == crate::service_def::TRINO_DEF_NAME || !carries_tag_service(config) {
        return Ok(());
    }
    let lacks = if typ == crate::service_def::STARROCKS_DEF_NAME {
        "StarRocks' service-def has no `schema` resource at all — its middle level is \
         `database`"
    } else {
        "only Trino's service-def declares a `schema` resource"
    };
    Err(PolicyStoreError::BadRequest(format!(
        "policy profile type {typ:?} cannot carry a {TAG_SERVICE_CONFIG_KEY:?}: the tag \
         download (`ServiceTags::from_assignments`) writes the resource levels \
         catalog/schema/table/column for every profile type, and {lacks}, so Ranger's own \
         `RangerTagEnricher.processServiceTags` drops every attachment and EVERY tag policy \
         in this profile's `tagPolicies` block would be silently inert — no masking, no row \
         filtering, no error. Create the profile without {TAG_SERVICE_CONFIG_KEY:?}; a tag \
         service is serveable only on a \"trino\" profile until the per-engine \
         resource-level mapping lands."
    )))
}

/// A NEW profile's `tagService` must name an existing profile of type `tag`.
///
/// Migration 0191 and the download/bump/overlap code treat a profile as a tag
/// service when its type is `tag` OR some other profile names it (live finding
/// I5), because the admin API used to accept only `trino`/`starrocks` and so
/// tag services made through it were `trino` rows. That tolerance is for rows
/// already stored; it is NOT a license to mint more. Only [`create_service`]
/// applies this rule: existing rows naming a non-tag profile are left alone
/// (spec D4 — no migration), and there is still no `UPDATE` writer of
/// `config_jsonb` in `src/`.
///
/// `found` is the target's stored `type`, or `None` when no such profile exists.
/// The check-then-insert is not atomic; a target deleted in between yields the
/// same dangling-name state `usable_tag_service` already tolerates (it joins on
/// the name, so a dangling one serves no tag policies), so no lock is taken.
///
/// **Falsified by** a second `INSERT INTO policy.authz_services` in `src/`, or a
/// `tag` profile type whose service-def is not seeded (the target would then be
/// uncreatable and every bind would 400).
fn refuse_tag_service_naming_a_non_tag_profile(
    target: &str,
    found: Option<&str>,
) -> Result<(), PolicyStoreError> {
    if found == Some(crate::service_def::TAG_DEF_NAME) {
        return Ok(());
    }
    let what = match found {
        Some(t) => format!("a profile of type {t:?}"),
        None => "no profile at all".to_string(),
    };
    Err(PolicyStoreError::BadRequest(format!(
        "{TAG_SERVICE_CONFIG_KEY:?} must name a `tag` profile, and {target:?} is {what}. \
         Create the tag profile first (`POST /policies/services` with type \"tag\"), then \
         name it here."
    )))
}

/// SP4d debt Task 9, step 4: refuse a [`TAG_SERVICE_CONFIG_KEY`] whose VALUE
/// names nothing. Pure, like the type rule above, and run straight after it.
///
/// # Why this is a second guard and not a change to [`carries_tag_service`]
///
/// Task 2 keyed that predicate to migration 0176's `->>` shape **on purpose**,
/// and mutation-tested the divergence: presence is the right question for the
/// TYPE rule (`{"tagService": 7}` on a StarRocks profile must be refused by the
/// route rather than surfacing the `CHECK` as a 500). Presence is the wrong
/// question for "does this value name a tag profile", so that question gets its
/// own function and **reuses** the presence predicate rather than re-spelling
/// it. One presence predicate, two rules on top of it.
///
/// # The measurement — one value, four behaviours across five readers
///
/// `{"tagService": ""}` passed every guard before this one, and the readers of
/// the field then disagreed:
///
/// | reader | how it reads the field | what `""` does |
/// |---|---|---|
/// | `ranger::api::download::download_policies` | `as_str()`, no filter, no fallback | `Some("")` → looks up a profile named `""` → **omits the `tagPolicies` block**, so the resource plugin never consults the tag policies at all |
/// | `admin::resolve_tag_service_and_body` (the tag editor's WRITE target) | `as_str()`, `filter(!is_empty)`, **then fell back to `{profile}_tag`** | **mis-filed** every tag policy into a profile the download does not serve (Task 9 removed that fallback) |
/// | `admin::list_profiles` (the Tags tab's existence) | `as_str()`, then `names.contains(candidate)` | hides the Tags tab (`""` is no profile's name) |
/// | `egret_perm_compile::trigger::recompile_service` | `as_str()`, then a name lookup it WARNS about | correctly refuses, with a log line |
/// | [`PolicyStore::usable_tag_service`] | SQL `JOIN … ON t.name = s.config_jsonb ->> 'tagService'` | correctly `None`, which is what makes `PUT /engines/{id}` refuse the bind |
///
/// Same silence as Task 2's, on the one profile type that is allowed to carry
/// the key at all. Refusing the value at the write path is what makes the
/// remaining four readers agree, because they then never see the shape.
///
/// # ⚠ That table is about `""`. The BIGGER population is an ABSENT key.
///
/// The row above for `list_profiles` holds for `""` and **inverts for an absent
/// key**, which is the shape almost every profile actually has: the fallback
/// candidate became `<name>_tag`, and if such a row existed `names.contains` was
/// **true**, so the tab was **shown** for a profile whose download carries no
/// `tagPolicies` at all. And that population is not an edge case —
/// `frontend/src/api/policies.ts::createProfile(name, type)` sends **no
/// `config`**, so **every console-created Trino profile** is in it, and nothing
/// can ever give one a `tagService` (there is no `UPDATE` of `config_jsonb` in
/// any `src/`). The fix round removed that fallback: `list_profiles` resolves
/// through [`usable_tag_services_via`], the same join this row names, so all
/// five readers now reach ONE conclusion for every shape — absent, `null`,
/// non-string, empty, dangling, or good.
///
/// # The shapes this refuses, and why a non-string is in the same class
///
/// Every reader above but `usable_tag_service` goes through `as_str()`, so a
/// number, bool, object or array reads as "unset" to four of five readers while
/// `->>` renders it as text for the fifth (`'{"tagService": 7}'::jsonb ->>
/// 'tagService'` is `'7'`, not NULL — measured on the live PostgreSQL 16.15).
/// Whitespace-only is the same: `" "` survives the editor's old
/// `filter(!is_empty)` and names no profile. So the rule is: the value must be
/// a JSON **string** that is non-empty after trimming.
///
/// JSON `null` is **deliberately permitted** and treated exactly as an absent
/// key, because that is what [`carries_tag_service`] (and `->>`, and migration
/// 0176's `CHECK`, and `Value::get`) already say it is. Refusing it here would
/// be a third reading of "present", which is the drift this programme keeps
/// re-finding.
///
/// # ⚠ Why there is NO migration for this rule, unlike Task 2's
///
/// 0176's `CHECK` exists because its state is **silently inert and has no Rust
/// writer in the `UPDATE` direction**: no `UPDATE` of `type` or `config_jsonb`
/// exists in any `src/` in this workspace (re-measured 2026-10-10 by grepping
/// every statement naming `policy.authz_services` across `crates/` and
/// `migrations/`: the only `src/` updates set `policy_version`, `tag_version`,
/// or `origin`), so the schema is that direction's only bound.
///
/// This rule's state is **not silent once the four readers above agree**: after
/// Task 9 the editor write refuses by name, `download_policies` logs a warning
/// instead of quietly omitting the block, `trigger.rs` already warns, and
/// `usable_tag_service` already refuses the engine bind. An operator who reaches
/// the state through `psql` is told, in four places, that the profile serves no
/// tag policies. A `CHECK` would also have to be written to a THIRD shape
/// (`jsonb_typeof(…) = 'string' AND btrim(…) <> ''`, since `->>` cannot
/// express "is a string"), which is exactly the two-guards-two-shapes defect
/// 0176's own doc comment warns about.
///
/// **Falsified by** a reader of this key that neither refuses nor logs an
/// unusable value, or by an `UPDATE` of `config_jsonb` appearing in `src/` —
/// either one makes the state silent again and the schema bound worth its cost.
fn refuse_unnameable_tag_service(config: &Value) -> Result<(), PolicyStoreError> {
    if !carries_tag_service(config) {
        return Ok(());
    }
    let value = config.get(TAG_SERVICE_CONFIG_KEY);
    if value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
    {
        return Ok(());
    }
    let found = value
        .map(ToString::to_string)
        .unwrap_or_else(|| "absent".into());
    Err(PolicyStoreError::BadRequest(format!(
        "{TAG_SERVICE_CONFIG_KEY:?} must be the NAME of a tag policy profile — a non-empty \
         string — and this config carries {found}. Four of the five readers of this key go \
         through `as_str()`, so a number, a bool, or an empty/whitespace-only string reads as \
         \"no tag service\" to the policy download (which then omits the `tagPolicies` block \
         entirely, leaving every tag mask and row filter inert), while the SQL join behind \
         `usable_tag_service` and migration 0176's `->>` read it as text. Omit the key \
         entirely for a profile with no tag service, or set it to an existing tag profile's \
         name."
    )))
}

/// SP4a Task 8 (D4): refuse adding brand-new content to a profile the
/// permission-model compiler has claimed (`origin = 'generated'`, migration
/// 0155) — used only by `create_policy` and
/// `default_policies::seed_default_trino_policies` (via the pooled wrapper
/// below). Unlike [`refuse_if_compiler_owned`], this is deliberately
/// profile-wide: a NEW policy cannot yet be "compiler-owned or not" (it
/// doesn't exist), so the only question create-time can ask is whether this
/// profile is one a human should be adding hand-written policies to at all —
/// Task 8 fix round 2 (I-1) kept this refusal exactly where the original
/// Task 8 left it; only the EDIT/DELETE guards below moved from profile
/// scope to policy scope.
///
/// Connection-generic so every caller can run it on its OWN already-open
/// transaction. `egret-perm-compile`'s recompile driver
/// (`trigger::apply_diff_in_tx`) never calls this — it writes
/// `policy.authz_policies` with its own raw `sqlx::query` calls directly
/// against the transaction it owns — so this refusal can never block the
/// compiler itself, only a human (or anything else) going through this
/// store's public write API.
///
/// `None`/absent service (a nonexistent `service_id`) is deliberately NOT an
/// error here — the caller's own existence check (a `NotFound` from a prior
/// lookup, or the FK violation an insert against a missing service would
/// raise) is what surfaces that, unchanged from before this guard existed.
async fn refuse_if_generated(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(), PolicyStoreError> {
    let origin: Option<String> = sqlx::query_scalar::<_, String>(
        r#"SELECT origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await?;
    if origin.as_deref() == Some("generated") {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "profile {service_id} is compiled by the SP4a permission model — edit its grants \
             and rules instead of writing a policy directly; the next recompile would silently \
             overwrite this change"
        )));
    }
    Ok(())
}

/// **Every refusal `create_policy` makes before its `INSERT`, in the one order
/// they are allowed to fire in** — SP4d debt Task 16, closing Task 6's
/// concern 5.
///
/// # Why the ORDER is a ruling and not an accident
///
/// Both guards can fire on the same call (a reserved name written into a
/// `'generated'` profile) and they say different things with different fixes:
/// [`refuse_reserved_name`] says "this NAME is off-limits, rename it" and fires
/// in a `'manual'` profile too; [`refuse_if_generated`] says "this PROFILE is
/// compiled, edit its grants and rules". SP4a Task 8 fix round 3 ruled that the
/// reserved-name reason wins, precisely because it is the one that also protects
/// `manual` profiles, and
/// `generated_profile_guard::a_generated_profile_refuses_a_reserved_name_for_the_reserved_reason`
/// pins it.
///
/// That ruling is why the order lives HERE rather than being written out twice.
/// Task 6's concern 5 wanted the route to report the 409 before its own
/// authoring guards turned it into a 400, and the first attempt at that lifted
/// `refuse_if_generated` alone into the handler — which silently inverted this
/// precedence, and the test above caught it. A second copy of the order in the
/// route would be two orderings that must agree, which is the defect this
/// programme keeps re-finding. So: one function, connection-generic, with two
/// entry points — [`PolicyStore::create_policy`] on its own transaction, and
/// [`PolicyStore::preflight_policy_insert`] on a pooled connection for a route
/// that needs the answer before it writes anything.
async fn refuse_before_policy_insert(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    name: &str,
) -> Result<(), PolicyStoreError> {
    // Pure, no DB needed, and ruled to win over the origin check below.
    refuse_reserved_name(name)?;
    refuse_if_generated(conn, service_id).await
}

/// SP4a Task 8 fix round 2 (I-1): refuse an edit/delete/enable-toggle on a
/// SPECIFIC policy only when it is itself compiler-owned (see
/// [`is_compiler_owned_policy_name`]) AND its profile is currently
/// `'generated'`. Read-only-ness protects what the compiler's sweep
/// (`apply_diff_in_tx`) would otherwise silently overwrite or reap on the
/// next recompile — a policy that sweep never looks at (a different name, no
/// `perm-grant-`/`perm-rule-` prefix) is not protected by that reasoning, so
/// it stays as editable/deletable as it would be in a `'manual'` profile.
/// This is the "moves from profile level to policy level" the user ruled on
/// for I-1: the old profile-wide refusal in `update_policy`/`delete_policy`/
/// `set_policy_enabled` is what built the one-way door (an operator could
/// never touch ANYTHING in a generated profile, including content the
/// compiler had no opinion about).
///
/// The `origin` check still matters even though the name-prefix check runs
/// first: a policy hand-named with this prefix in a still-`'manual'` profile
/// is not YET at risk from a sweep that has never run against this service —
/// see the fix-round-2 report's finding on whether that policy could later
/// be silently reaped once the profile becomes `'generated'` (a Task 7
/// property, tracked separately, not fixed by this guard).
async fn refuse_if_compiler_owned(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    policy_name: &str,
) -> Result<(), PolicyStoreError> {
    if !is_compiler_owned_policy_name(policy_name) {
        return Ok(());
    }
    let origin: Option<String> = sqlx::query_scalar::<_, String>(
        r#"SELECT origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await?;
    if origin.as_deref() == Some("generated") {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "policy {policy_name:?} is compiled by the SP4a permission model — it is produced \
             by a grant or rule, not hand-authored; edit or delete that grant/rule instead of \
             writing this policy directly, since the next recompile would otherwise silently \
             overwrite this change"
        )));
    }
    Ok(())
}

/// SP4a Task 8 fix round 2 (I-1): `delete_service`'s own guard — refuse only
/// while at least one compiler-owned policy (see
/// [`is_compiler_owned_policy_name`]) still exists under `service_id`,
/// regardless of the profile's `origin`. A `'generated'` profile that the
/// compiler has since reaped down to zero owned policies (every grant/rule
/// reaching it was deleted) is deletable — `origin` is sticky (Task 8's
/// original design) precisely so the EDIT guard above keeps distinguishing
/// "this profile has been compiler-managed" from "this profile has never
/// been touched", but that stickiness has no bearing on whether deleting the
/// row right now would delete anything the compiler still owns.
async fn refuse_if_compiler_owned_policies_remain(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(), PolicyStoreError> {
    let count: i64 = sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*) FROM policy.authz_policies
           WHERE service_id = $1
             AND (name LIKE $2 || '%' OR name LIKE $3 || '%' OR name LIKE $4 || '%'
                  OR name LIKE $5 || '%')"#,
    )
    .bind(service_id)
    .bind(COMPILER_GRANT_PREFIX)
    .bind(COMPILER_RULE_PREFIX)
    .bind(COMPILER_CLEARANCE_PREFIX)
    .bind(COMPILER_ENGINE_GRANT_PREFIX)
    .fetch_one(&mut *conn)
    .await?;
    if count > 0 {
        let noun = if count == 1 { "policy" } else { "policies" };
        return Err(PolicyStoreError::CompilerOwned(format!(
            "profile {service_id} still holds {count} {noun} compiled by the SP4a permission \
             model (named with the perm-grant-/perm-rule-/perm-clearance-/perm-engine- prefix) — \
             delete the grants/rules/engine grants that produce them, or the classification \
             clearance behind them, first, or wait for the next recompile to reap them, \
             before deleting the profile itself"
        )));
    }
    Ok(())
}

impl PolicyStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The pool, for sibling modules that own their own tables
    /// (`plugin_downloads`).
    pub(crate) fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    /// Pool-based [`refuse_if_generated`], for a write path that has no
    /// transaction open yet at the point the check belongs
    /// (`default_policies::seed_default_trino_policies`, which must refuse
    /// BEFORE its own "already has policies" no-op check — a generated
    /// profile always has policies, so that check would otherwise silently
    /// swallow the refusal as a no-op `Ok(0)`).
    pub(crate) async fn refuse_if_generated_pooled(
        &self,
        service_id: i64,
    ) -> Result<(), PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        refuse_if_generated(&mut conn, service_id).await
    }

    /// **The pre-flight for `POST /policies`** — SP4d debt Task 16, closing
    /// Task 6's concern 5. Runs exactly the refusals
    /// [`Self::create_policy`] makes before its `INSERT`, in exactly their
    /// order, and writes nothing.
    ///
    /// The route needs this because its own authoring guards (Task 5's
    /// unreadable-table refusal, Task 6's ambiguous-arbitration refusal) run
    /// BEFORE `create_policy` is called at all, so without a pre-flight a
    /// steward adding a policy to a compiler-owned profile was told "this
    /// would be ambiguous" (400) when the operative fact was "this profile is
    /// compiled, edit its grants and rules" (409).
    ///
    /// The in-transaction call inside `create_policy` STAYS and is the real
    /// bound: it, not this one, holds for every other caller and against a
    /// recompile that claims the profile between this read and the write. This
    /// only decides which refusal the operator reads.
    ///
    /// Deliberately NOT a second reading of `origin` or of the reserved prefix —
    /// it is [`refuse_before_policy_insert`], the same function, on a pooled
    /// connection instead of a transaction. Task 6 declined to write its own
    /// `origin` read for that reason and was right to.
    pub(crate) async fn preflight_policy_insert(
        &self,
        service_id: i64,
        name: &str,
    ) -> Result<(), PolicyStoreError> {
        // First, exactly where `create_policy` puts it. Not inside
        // `refuse_before_policy_insert`: the infra writers share that one.
        let mut conn = self.db.pool.acquire().await?;
        let service_type = service_type_of(&mut conn, service_id).await?;
        refuse_infrastructure_name(&service_type, name)?;
        refuse_before_policy_insert(&mut conn, service_id, name).await
    }

    /// The `PUT` route's counterpart of [`Self::preflight_policy_insert`]:
    /// the SP4d-3 infrastructure refusals `update_policy` makes (an `'infra'`
    /// row, and a rename INTO an infrastructure name), on a pooled connection,
    /// so they win over the route's authoring guards (ambiguous-arbitration,
    /// unreadable-table) that run before `update_policy` is called. The
    /// in-transaction checks inside `update_policy` STAY and are the real
    /// bound; this only decides which refusal the operator reads.
    pub(crate) async fn preflight_policy_update(
        &self,
        id: i64,
        name: &str,
    ) -> Result<(), PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        let current: Option<(String, String)> = sqlx::query_as(
            "SELECT p.name, s.type FROM policy.authz_policies p \
             JOIN policy.authz_services s ON s.id = p.service_id WHERE p.id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some((current_name, service_type)) = current else {
            return Err(PolicyStoreError::NotFound(format!("policy {id}")));
        };
        refuse_if_infrastructure_owned(&mut conn, id).await?;
        if name != current_name {
            refuse_infrastructure_name(&service_type, name)?;
        }
        Ok(())
    }

    /// SP4d debt Task 5 — the POLICY EDITOR's entry into the one rule in
    /// [`crate::unreadable_table_mask`]: a hand-authored tag MASKING policy
    /// (`POST /api/control-plane/policies`, `kind: "tag-masking"`) is the
    /// second surface that can author the mask half, and on this development
    /// host it is the surface that produced **all four** live instances of the
    /// pair (see that module's doc for the measurement).
    ///
    /// Pooled and read-only, for the same reason
    /// [`Self::refuse_if_generated_pooled`] is: the editor's create path has no
    /// transaction open at the point the check belongs, and the check must
    /// refuse BEFORE `create_policy` commits, so that a refusal leaves nothing
    /// stored and is a plain 400 rather than a compensating delete.
    pub(crate) async fn unreadable_table_mask_for_tag_mask(
        &self,
        tag_name: &str,
        policy_name: &str,
    ) -> Result<Option<crate::unreadable_table_mask::UnreadableTableMask>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        Ok(crate::unreadable_table_mask::unreadable_table_mask_for(
            &mut conn,
            crate::unreadable_table_mask::TagWrite::Mask {
                tag: crate::unreadable_table_mask::TagRef::Name(tag_name),
                origin: crate::unreadable_table_mask::MaskOrigin::Policy,
                name: policy_name,
            },
        )
        .await?)
    }

    /// Idempotent upsert of a service-def by name (used to seed Trino).
    pub async fn upsert_service_def(
        &self,
        id: i64,
        name: &str,
        def: &Value,
    ) -> Result<(), PolicyStoreError> {
        // `authz_service_defs` has TWO unique constraints — the `id` primary key and a unique
        // `name` — and `ON CONFLICT (id)` names only the first as its arbiter. A violation on the
        // OTHER index is not absorbed by the DO UPDATE; Postgres raises it.
        //
        // That is reachable whenever two callers seed the same def at once: each engine runs this
        // at startup, and three test binaries run it in parallel against one database. The loser
        // inserts a row whose `id` is not yet visible to it but whose `name` collides on the index,
        // and gets `23505 authz_service_defs_name_key`. Reproduced locally by running those three
        // binaries concurrently — it fails roughly one round in four.
        //
        // Retrying is the whole fix: this is an IDEMPOTENT SEED, so a concurrent caller having
        // already written the row is the desired end state, and by the second attempt that row has
        // committed and `ON CONFLICT (id)` absorbs it. One retry is enough — the race is with a
        // transaction that is already committing, not an unbounded stream of writers.
        for attempt in 0..2 {
            let result = sqlx::query(
                r#"INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,$3)
                   ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, def_jsonb = EXCLUDED.def_jsonb"#,
            )
            .bind(id)
            .bind(name)
            .bind(def)
            .execute(&self.db.pool)
            .await;

            match result {
                Ok(_) => return Ok(()),
                Err(e) if attempt == 0 && is_unique_violation(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub async fn get_service_def(&self, name: &str) -> Result<ServiceDefRow, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        get_service_def_via(&mut conn, name).await
    }

    pub async fn list_service_defs(&self) -> Result<Vec<ServiceDefRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM policy.authz_service_defs ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Insert a policy profile (a Ranger service instance). The **only**
    /// `INSERT INTO policy.authz_services` in any `src/` — see
    /// [`refuse_tag_service_on_non_trino_profile`]'s enumeration, which is why
    /// this one guard covers every create path (the admin route, engine-mgmt's
    /// auto-bind, and the `PolicyPort` trait that delegates here).
    pub async fn create_service(
        &self,
        name: &str,
        typ: &str,
        config: &Value,
    ) -> Result<ServiceRow, PolicyStoreError> {
        // SP4d debt Task 2: pure, no DB needed, and it runs BEFORE the insert so
        // the route answers 400 with the reason rather than letting migration
        // 0176's `CHECK` surface as an opaque 500. The `CHECK` is not redundant
        // with this — it is the only thing holding the `UPDATE` direction, which
        // has no Rust writer to guard.
        refuse_tag_service_on_non_trino_profile(typ, config)?;
        // SP4d debt Task 9 step 4: the TYPE rule above asks whether the key may
        // be here at all; this one asks whether its value names anything. Two
        // rules, one presence predicate — see
        // `refuse_unnameable_tag_service` for the four-behaviours-across-five-readers
        // measurement and for why this one has no migration.
        refuse_unnameable_tag_service(config)?;
        // Task 5 (D): the value names something, now ask what it names.
        if let Some(target) = config.get(TAG_SERVICE_CONFIG_KEY).and_then(Value::as_str) {
            let found: Option<String> =
                sqlx::query_scalar("SELECT type FROM policy.authz_services WHERE name = $1")
                    .bind(target)
                    .fetch_optional(&self.db.pool)
                    .await?;
            refuse_tag_service_naming_a_non_tag_profile(target, found.as_deref())?;
        }
        sqlx::query_as::<_, ServiceRow>(
            r#"INSERT INTO policy.authz_services (name, type, config_jsonb)
               VALUES ($1,$2,$3) RETURNING id, name, type, config_jsonb, policy_version, tag_version, origin"#,
        )
        .bind(name)
        .bind(typ)
        .bind(config)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                PolicyStoreError::AlreadyExists(name.into())
            }
            _ => PolicyStoreError::Db(e),
        })
    }

    /// Like [`get_service`], keyed by id instead of name — SP4a Task 7's
    /// recompile driver is handed a `service_id` (the trigger's caller
    /// already resolved which services a tag/grant write reaches; see
    /// `egret_catalog_registry::services_bound_to_warehouse`) and has no
    /// service name to look it up by until it reads this row.
    pub async fn get_service_by_id(&self, id: i64) -> Result<ServiceRow, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        get_service_by_id_via(&mut conn, id).await
    }

    pub async fn get_service(&self, name: &str) -> Result<ServiceRow, PolicyStoreError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("service {name}")))
    }

    /// Like [`get_service`], but returns `None` instead of `NotFound` when the
    /// named service is absent (used by seed paths that no-op when unbound).
    pub async fn get_service_opt(
        &self,
        name: &str,
    ) -> Result<Option<ServiceRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Whether a policy of the given name already exists under a service (used to
    /// make policy seeding idempotent).
    pub async fn policy_exists(
        &self,
        service_id: i64,
        name: &str,
    ) -> Result<bool, PolicyStoreError> {
        Ok(sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(SELECT 1 FROM policy.authz_policies WHERE service_id = $1 AND name = $2)"#,
        )
        .bind(service_id)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await?)
    }

    pub async fn list_services(&self) -> Result<Vec<ServiceRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// The name of the tag service `service_id` can actually USE, or `None`.
    ///
    /// Both halves must hold: the profile carries a `tagService` in its
    /// `config_jsonb`, AND that name resolves to a real
    /// `policy.authz_services` row. The join is the same one
    /// [`services_and_tag_services_via`] uses, so the two cannot disagree about
    /// what "has a tag service" means — which matters, because that function
    /// silently contributes nothing for a dangling `tagService`, and this is
    /// the predicate callers use to refuse BEFORE reaching that silence.
    ///
    /// A profile with no usable tag service compiles ZERO tag policies, so
    /// every mask and row filter vanishes and the columns are served raw.
    /// `PUT /engines/{id}` refuses to bind an engine to such a profile.
    ///
    /// **One query, one row** — `admin::list_profiles` needs the same answer for
    /// every profile at once, so the join lives in
    /// [`usable_tag_services_via`] and this is the single-id entry point. Two
    /// entry points, one spelling; see that function.
    pub async fn usable_tag_service(
        &self,
        service_id: i64,
    ) -> Result<Option<String>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        Ok(usable_tag_services_via(&mut conn, &[service_id])
            .await?
            .pop()
            .map(|(_, name)| name))
    }

    /// Bulk [`Self::usable_tag_service`] — `service_id` → the tag service it can
    /// actually USE, for every id that has one. **SP4d debt Task 9 fix round.**
    ///
    /// `admin::list_profiles` renders one row per profile and needs this answer
    /// for all of them; asking per profile would be an N+1 against a query that
    /// is a single join, and — worse — a second spelling of the join would be
    /// free to disagree with [`Self::usable_tag_service`] about what "has a tag
    /// service" means. That disagreement is exactly the defect Task 9 closed, so
    /// the bulk form is the SAME function with a wider `WHERE`.
    ///
    /// A profile with no link, or a link naming nothing, is simply ABSENT from
    /// the map — the caller reads that as "no Tags tab", which is what the
    /// download will serve.
    ///
    /// `pub` rather than `pub(crate)` so
    /// `tests/store_services.rs::both_entry_points_onto_the_tag_service_join_agree`
    /// can assert the two entry points side by side, shape by shape. That test is
    /// the only reason this is not crate-private, and it is the right reason: the
    /// agreement is the deliverable.
    pub async fn usable_tag_services(
        &self,
        service_ids: &[i64],
    ) -> Result<std::collections::HashMap<i64, String>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        Ok(usable_tag_services_via(&mut conn, service_ids)
            .await?
            .into_iter()
            .collect())
    }

    /// DELETE a profile outright — refused (Task 8) while it still holds any
    /// compiler-owned policy (see [`refuse_if_compiler_owned_policies_remain`]):
    /// a compiled profile's row (and the `perm.compiled` provenance `ON DELETE
    /// CASCADE`s from it, migration 0154) stays owned by the compiler until
    /// the perm model stops targeting it, not until an admin deletes it out
    /// from under a still-live recompile. Fix round 2 (I-1): this is scoped to
    /// the policies actually present, not the profile's `origin` — a
    /// `'generated'` profile the compiler has since reaped down to zero owned
    /// policies is deletable.
    ///
    /// Review fix (M-1): the check and the delete run on the SAME
    /// transaction, not a pooled check followed by a separately-connected
    /// delete (the shape Task 7's `recompile_service_in_tx` exists to avoid
    /// for exactly this reason — see that function's own doc comment). Two
    /// separate connections left a window: a compile writing a NEW
    /// compiler-owned policy between the check and the delete would let a
    /// profile be deleted anyway despite that write. One transaction closes
    /// it — Postgres holds the relevant rows from the `SELECT count(*)`
    /// through the `DELETE`, so a concurrent write from `apply_diff_in_tx`
    /// either commits first (this delete then sees the new row and refuses)
    /// or blocks until this transaction ends.
    pub async fn delete_service(&self, id: i64) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        if let Err(e) = refuse_if_compiler_owned_policies_remain(&mut tx, id).await {
            tx.rollback().await?;
            return Err(e);
        }
        let n = sqlx::query(r#"DELETE FROM policy.authz_services WHERE id = $1"#)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("service id {id}")));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Total policy count per service (service_id -> count), for the profiles list.
    pub async fn policy_counts(
        &self,
    ) -> Result<std::collections::HashMap<i64, i64>, PolicyStoreError> {
        let rows = sqlx::query_as::<_, (i64, i64)>(
            "SELECT service_id, count(*) FROM policy.authz_policies GROUP BY service_id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Policy counts per service and owner (`managed_by`): service_id ->
    /// (compiler, infra, operator). DG #4a Q1.
    pub async fn policy_counts_by_owner(
        &self,
    ) -> Result<std::collections::HashMap<i64, (i64, i64, i64)>, PolicyStoreError> {
        let rows = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT service_id, \
                    count(*) FILTER (WHERE managed_by = 'compiler'), \
                    count(*) FILTER (WHERE managed_by = 'infra'), \
                    count(*) FILTER (WHERE managed_by = 'operator') \
               FROM policy.authz_policies GROUP BY service_id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, c, i, o)| (id, (c, i, o)))
            .collect())
    }

    /// Catalog resource → tag rows for a service's warehouse(s), feeding the
    /// Ranger tag download (`ServiceTags::from_assignments`).
    ///
    /// SERVICE → CATALOG-NAME MAPPING: a policy profile (`policy.authz_services`)
    /// is bound to warehouses through the engine seam —
    /// `engine_mgmt.engines.authz_service_id`. Each such warehouse's tag
    /// assignments (keyed by `warehouse_id`) are served under every Trino
    /// catalog name the engine attaches, reached through
    /// `registry.engine_catalog_attachments` (SP3 D5).
    ///
    /// One branch only, since SP3c-2: the engine attaches this warehouse's
    /// registry catalog through `registry.engine_catalog_attachments`.
    ///
    /// Until SP3c-2 a second branch read `engine_mgmt.engine_catalogs` for two
    /// populations the registry could not see: engines whose backfill failed,
    /// and a catalog renamed but not yet re-applied. Both are gone — the backfill
    /// is deleted, so no engine can be in the first state, and
    /// `registry.catalogs.name` is UNIQUE and immutable, so nothing renames a
    /// catalog at all.
    ///
    /// # ⚠ Why this is THREE statements and not one (SP4d debt Task 6, item 2)
    ///
    /// It used to be one, and it carried the engine-reachability leg
    /// (`engine_catalog_attachments` ⋈ `engines` on `authz_service_id`) **as a
    /// second copy of [`catalogs_for_service_via`]'s**. That copy is what
    /// `explain` has to agree with: the tag set `explain` hands the evaluator
    /// comes from [`resolved_tags_for_resource_via`], which scopes on
    /// `a.warehouse_id = c.warehouse_id` and applies NO engine predicate at all,
    /// and the two agreed only because `explain`'s own caller happens to narrow
    /// the catalog through `catalogs_for_service_via` first. Nothing compared
    /// them — no test, no `CHECK`, no shared helper — and a change to either leg
    /// would have made `explain` resolve tags the engine never received, i.e. a
    /// confident over-deny.
    ///
    /// So the leg is now spelled exactly ONCE, in `catalogs_for_service_via`,
    /// and this function CALLS it. The three steps are: resolve which profile's
    /// engines bind ([`engine_binding_profile_via`]), ask that one helper which
    /// catalogs they attach, then read the assignments for exactly those
    /// catalogs ([`tag_assignments_for_catalogs_via`]). `explain` and the
    /// download can no longer disagree about reachability, because there is
    /// nothing left to disagree with.
    ///
    /// **One behaviour change, and it is invisible on the wire.** The old single
    /// statement had no `DISTINCT`, so a catalog attached to TWO engines that
    /// both bind this profile yielded each assignment twice;
    /// `catalogs_for_service_via` is `SELECT DISTINCT`, so it now yields once.
    /// `ServiceTags::from_assignments` de-duplicates by `tag_id` and by
    /// `(catalog, schema, table, column)` on the way in, so both inputs produce
    /// byte-identical output — checked, not assumed, in that function.
    ///
    /// Two engines on one catalog is the ONLY way the old duplicate could
    /// arise, and that is a schema fact rather than a judgement: migration
    /// `0147` carries `CREATE UNIQUE INDEX catalogs_warehouse_id_key ON
    /// registry.catalogs (warehouse_id) WHERE warehouse_id IS NOT NULL`, so a
    /// warehouse has at most one catalog and "served under every catalog name
    /// the engine attaches" is at most one name per warehouse today. Measured:
    /// the first version of this task's set-equality fixture tried to give one
    /// warehouse a second catalog and got `23505`.
    ///
    /// **What is still spelled more than once, named so the next reader does not
    /// have to re-find it.** Three other statements apply an
    /// `authz_service_id` reachability leg and are NOT routed through the
    /// helper, because each answers a different question and merging them would
    /// be wrong rather than tidy: [`services_for_catalog_via`] walks it
    /// BACKWARDS (catalog → profiles), and
    /// `egret_catalog_registry::tag_version`'s `bump_bound_tag_versions` and
    /// `services_bound_to_warehouse` key it by WAREHOUSE rather than by
    /// catalog. The one that does ask the same question is
    /// [`tagged_columns_for_service_via`] — deliberately left alone here
    /// because it feeds the rule compiler's StarRocks expansion rather than the
    /// tag download, so changing its row set would change compiled artifacts,
    /// which is not this task's subject. It is the next candidate.
    pub async fn tag_assignments_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<TagAssignmentRow>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        let Some(profile_id) = engine_binding_profile_via(&mut conn, service_id).await? else {
            // No such `policy.authz_services` row. The old single statement's
            // `JOIN policy.authz_services s ON s.id = $1` returned no rows for
            // this too.
            return Ok(Vec::new());
        };
        // THE reachability leg, spelled once — see this function's doc.
        let catalog_ids: Vec<uuid::Uuid> = catalogs_for_service_via(&mut conn, profile_id)
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect();
        tag_assignments_for_catalogs_via(&mut conn, &catalog_ids).await
    }

    /// Registry catalogs reachable by `service_id`'s engine(s), through
    /// `registry.engine_catalog_attachments` (SP3 D5) — the DIRECT engine
    /// bind (`e.authz_service_id = service_id`), not the tag-service
    /// `COALESCE` `tag_assignments_for_service` resolves: a recompile always
    /// targets the profile's own service id, never its paired tag service
    /// (StarRocks has no tag service at all — D5).
    pub async fn catalogs_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<CatalogRow>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        catalogs_for_service_via(&mut conn, service_id).await
    }

    /// Columns carrying `tags.key = tag_key`, resolved to their Ranger-facing
    /// NAMES, restricted to catalogs `service_id`'s engine(s) reach.
    /// Table-level tags (`column_name = ''`, migration 0081) are excluded —
    /// this feeds the StarRocks per-column expansion (D5), which masks a
    /// COLUMN, never a whole table.
    pub async fn tagged_columns_for_service(
        &self,
        service_id: i64,
        tag_key: &str,
    ) -> Result<Vec<TaggedColumnRow>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        tagged_columns_for_service_via(&mut conn, service_id, tag_key).await
    }

    /// `tags.name` for a `tags.key` — D6: rules select by the stable `key`,
    /// but every engine (and the Ranger tag plugin) matches on the mutable
    /// `name`, so this is the resolution `ResolvedNames.tag_name` needs
    /// before a tag-selector rule can compile. `None` when no such key
    /// exists (a rule can outlive the tag it named, e.g. after a tag
    /// deletion — the caller decides what that means for the compile, not
    /// this lookup).
    pub async fn tag_name_for_key(&self, key: &str) -> Result<Option<String>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        tag_name_for_key_via(&mut conn, key).await
    }
}

/// Connection-generic core of [`PolicyStore::get_service_by_id`] — SP4a
/// Task 7 fix round 1: `egret-perm-compile`'s `recompile_service_in_tx`
/// reads the service row from inside the caller's OWN open transaction
/// (`egret-bi::catalog_tags`'s tag write), not a second pool-acquired
/// connection — see `egret_perm::store::list_grants_via`'s doc comment for
/// why that distinction matters (READ COMMITTED visibility of the caller's
/// own uncommitted writes).
pub async fn get_service_by_id_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<ServiceRow, PolicyStoreError> {
    sqlx::query_as::<_, ServiceRow>(
        r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| PolicyStoreError::NotFound(format!("service id {id}")))
}

/// The id of the profile NAMED `name`, or `None` when no such row exists.
///
/// **SP4a Task 11, defect 2.** A compiled policy's Ranger body carries the
/// name of the service it belongs to (`"service": "<name>"`), and for a
/// tag-selected rule on Trino that name is the profile's TAG service, not the
/// profile itself (`admin::ranger_json::tagify`). `egret-perm-compile`'s
/// recompile driver stored every policy it produced under the profile's own
/// `service_id` regardless, so the tag plugin's download
/// (`/service/plugins/policies/download/<profile>_tag`) served ZERO policies
/// while the resource profile carried a `serviceType: tag` policy it can never
/// match — a mask that deployed, listed as live, and returned the real values
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, case
/// T3). This is the lookup that turns the body's own `service` name into the
/// row's `service_id`, so the two can no longer disagree.
///
/// Connection-generic for the same reason as [`get_service_by_id_via`]: the
/// driver runs inside the caller's own open transaction.
pub async fn service_id_by_name_via(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<Option<i64>, PolicyStoreError> {
    Ok(
        sqlx::query_scalar::<_, i64>(r#"SELECT id FROM policy.authz_services WHERE name = $1"#)
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// **Which profile's engine bindings decide what a polled service may serve.**
///
/// The Trino tag plugin polls the TAG service (`<profile>_tag`), but engines
/// bind to the TRINO profile — so a tag-service id has to be resolved to the
/// profile that names it in `config_jsonb->>'tagService'` before any
/// reachability question can be asked about it. `COALESCE(tp.id, s.id)` also
/// accepts a profile id directly, so a caller that already holds one (and every
/// unit test) passes it straight through. Passing the tag service id through
/// unresolved — as the HTTP `download_tags` path once did — matched no engine
/// and made tag masking silently no-op.
///
/// `None` means `service_id` names no `policy.authz_services` row at all, which
/// is the one case the caller must distinguish from "a real service that
/// reaches no catalog": the first serves nothing because it does not exist.
///
/// Split out of [`PolicyStore::tag_assignments_for_service`] (SP4d debt Task 6)
/// so that function could be composed onto [`catalogs_for_service_via`]; the
/// `tagService` key is spelled as a literal here and cannot reference
/// `egret_policy_api::TAG_SERVICE_CONFIG_KEY` (that would mean building the
/// statement with `format!`), which is exactly what
/// `store::tests::the_const_is_the_key_the_sql_and_the_migration_name` guards.
pub async fn engine_binding_profile_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT COALESCE(tp.id, s.id)
           FROM policy.authz_services s
           LEFT JOIN policy.authz_services tp
                ON tp.type = 'trino' AND tp.config_jsonb->>'tagService' = s.name
           WHERE s.id = $1"#,
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// Every catalog-tag assignment carried by the warehouses behind `catalog_ids`,
/// served under each of those catalogs' names — the second half of
/// [`PolicyStore::tag_assignments_for_service`], with **no engine predicate of
/// its own**.
///
/// That absence is the point (SP4d debt Task 6, item 2). Reachability is
/// decided once, by [`catalogs_for_service_via`], and handed in; this statement
/// is left with exactly the `a.warehouse_id = c.warehouse_id` scoping that
/// [`resolved_tags_for_resource_via`] uses, so the download writer and the
/// resolver `explain` evaluates against now apply the same two rules in the
/// same two places instead of two coincidentally-equal joins.
///
/// An empty `catalog_ids` returns no rows rather than every row: `= ANY('{}')`
/// is false for every value, which is the fail-closed direction and the same
/// answer the old single statement gave for a profile no engine binds.
pub async fn tag_assignments_for_catalogs_via(
    conn: &mut sqlx::PgConnection,
    catalog_ids: &[uuid::Uuid],
) -> Result<Vec<TagAssignmentRow>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, TagAssignmentRow>(
        r#"SELECT
               c.name           AS catalog,
               a.schema_name    AS schema_name,
               a.table_name     AS table_name,
               a.column_name    AS column_name,
               t.id::text       AS tag_id,
               t.name           AS tag_name
           FROM catalog_tag_assignments a
           JOIN tags t ON t.id = a.tag_id
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
           WHERE c.id = ANY($1)
           ORDER BY catalog, schema_name, table_name, column_name, tag_name"#,
    )
    .bind(catalog_ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// Connection-generic core of [`PolicyStore::catalogs_for_service`] — same
/// rationale as [`get_service_by_id_via`].
///
/// **⚠ This is the ONE spelling of the engine-reachability leg in the forward
/// direction, and two callers' agreement rests on that** (SP4d debt Task 6,
/// item 2): `egret_perm_compile`'s `explain` narrows a catalog through it
/// before resolving tags with [`resolved_tags_for_resource_via`], and
/// [`PolicyStore::tag_assignments_for_service`] calls it to decide what the tag
/// download may carry. Those two used to apply two separate copies of this
/// join, compared by nothing; a change to either copy made `explain` resolve
/// tags the engine never received — a confident over-deny. Change this one and
/// both move.
pub async fn catalogs_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<CatalogRow>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, CatalogRow>(
        r#"SELECT DISTINCT c.id, c.name
           FROM registry.catalogs c
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE e.authz_service_id = $1"#,
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// **The join that decides what "has a usable tag service" means, spelled
/// once** — SP4d debt Task 9 fix round. `(profile id, tag service name)` for
/// every id in `service_ids` whose `config_jsonb ->> 'tagService'` names a real
/// `policy.authz_services` row.
///
/// Both halves must hold, and a profile that fails either is simply ABSENT from
/// the result: no link, a link that is `null`/empty/non-string (so `->>` yields
/// NULL or a string no row carries), or a link naming a profile that does not
/// exist. That is the same answer `download_policies` serves — it reads the
/// field, finds nothing, and ships the download with no `tagPolicies` block —
/// which is the agreement this function exists to make unbreakable.
///
/// # Why this is a function and not two queries
///
/// Before the fix round there were THREE readers of this field in this crate
/// with three different answers: [`PolicyStore::usable_tag_service`] joined (and
/// refused an engine bind), `download_policies` read `as_str()` with no
/// fallback, and `admin::list_profiles` fell back to the `<name>_tag` NAMING
/// CONVENTION and so offered a Tags tab for profiles whose download carries no
/// tag policies at all. The convention fallback is gone and `list_profiles` now
/// reads THIS, through [`PolicyStore::usable_tag_services`]. One spelling, two
/// entry points.
///
/// [`services_and_tag_services_via`] below carries the same `ON t.name = s
/// .config_jsonb ->> 'tagService'` predicate deliberately: it answers a
/// different question (which ids must be row-locked, the profile itself
/// INCLUDED) and returns ids rather than names. If that predicate ever changes,
/// it changes in both — they are the two halves of one agreement.
async fn usable_tag_services_via(
    conn: &mut sqlx::PgConnection,
    service_ids: &[i64],
) -> Result<Vec<(i64, String)>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, (i64, String)>(
        r#"SELECT s.id, t.name
             FROM policy.authz_services s
             JOIN policy.authz_services t
               ON t.name = s.config_jsonb ->> 'tagService'
            WHERE s.id = ANY($1)"#,
    )
    .bind(service_ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// Every service in `service_ids`, **plus each one's tag service**, ascending.
///
/// SP4a Task 11, defect 2: a recompile of a Trino profile now reads and writes
/// `policy.authz_policies` rows under the profile's TAG service as well as
/// under the profile itself, and takes a `FOR UPDATE` row lock on each. The
/// lock ORDER is what keeps two overlapping recompiles queueing instead of
/// deadlocking (`egret_perm_compile::trigger::lock_services_in_tx`), and a
/// profile's tag service can perfectly well have a LOWER id than the profile —
/// the live cluster's `sp3c2-trino-live` is profile 52 with tag service 51. So
/// the set every caller locks has to be the EXPANDED one, sorted once, rather
/// than profiles first and their tag services discovered later.
///
/// A `tagService` naming no row contributes nothing (the join drops it), which
/// is the same treatment the compiler gives it.
pub async fn services_and_tag_services_via(
    conn: &mut sqlx::PgConnection,
    service_ids: &[i64],
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT q.id FROM (
               SELECT s.id
                 FROM policy.authz_services s
                WHERE s.id = ANY($1)
               UNION
               SELECT t.id
                 FROM policy.authz_services s
                 JOIN policy.authz_services t
                   ON t.name = s.config_jsonb ->> 'tagService'
                WHERE s.id = ANY($1)
           ) q
           ORDER BY 1"#,
    )
    .bind(service_ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// **The reverse of [`catalogs_for_service_via`]**: which policy profiles does
/// a change to THIS catalog affect? SP4a Task 9 needs it — a grant names a
/// catalog, so a grant write has to recompile exactly the profiles whose
/// engines reach that catalog. Same join, walked the other way, and the same
/// `authz_service_id` binding `egret_catalog_registry::services_bound_to_warehouse`
/// uses for the tag-write trigger; this one is keyed by catalog rather than
/// by warehouse because that is the level a `perm.grants.catalog_id` names.
///
/// Profiles NO engine binds are deliberately absent: nothing downloads them,
/// so nothing enforces them. `POST /api/control-plane/perm/compile/{id}` is
/// the repair trigger for a profile that later acquires a binding.
pub async fn services_for_catalog_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT e.authz_service_id
           FROM registry.engine_catalog_attachments ea
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE ea.catalog_id = $1 AND e.authz_service_id IS NOT NULL
           ORDER BY 1"#,
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// Every profile the SP4a compiler targets and an engine actually binds —
/// i.e. every `trino`/`starrocks` service some `engine_mgmt.engines` row
/// points at.
///
/// **This is deliberately installation-wide.** A tag-selected rule is
/// catalog-UNSCOPED on Trino by design (D5: one tag policy, resolved by the
/// Ranger tag plugin, covering whatever the tag is on), so creating or
/// deleting one rule changes the compiled output of EVERY Trino profile in
/// the installation — there is no narrower honest answer. StarRocks profiles
/// are included for the same mutation because the same rule compiles there
/// too, per tagged column. See `egret_perm_compile::trigger::affected_services_for_rule`.
pub async fn compiler_targeted_services_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT s.id
           FROM policy.authz_services s
           JOIN engine_mgmt.engines e ON e.authz_service_id = s.id
           WHERE s.type IN ('trino', 'starrocks')
           ORDER BY s.id"#,
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// The `tags.key`s carried by ONE exact resource (`column = ""` means the
/// table itself), resolved through the catalog's warehouse.
///
/// # ⚠ EXACT, not hierarchy-complete — do not feed this to an evaluator
///
/// `schema_name` / `table_name` / `column_name` are matched by string
/// EQUALITY, so asked about `report.country` this does not return a tag
/// attached to the table `report`. Ranger's `RangerTagEnricher` does the
/// opposite: it walks the hierarchy and emits ancestor tags, which Ranger
/// admits for a tag request. Every live assignment today is column-level, so
/// the difference is invisible in current data and would ship green.
///
/// SP4a Task 9 added this so `explain` could hand the resolved set to
/// `egret_perm::eval::evaluate`, which does no I/O of its own. **SP4d-2a Task 5
/// stopped doing that, and SP4d-2b Task 7 settled where each query belongs.**
/// `explain` evaluates the artifact each engine downloads, and the tag set it
/// hands the evaluator comes from [`resolved_tags_for_resource_via`] — which
/// rolls the hierarchy up and returns `tags.name` — gated on
/// [`tag_assignment_coordinates_via`] and `egret_perm_compile`'s
/// `stored_coordinate_is_fold_divergent`. What `explain` uses THIS for is
/// the response's informational `tags` list — "what has egret got recorded on
/// exactly this resource" — and nothing else. The two must not be merged: this
/// one returns `tags.key`, which is a different value (live they differ for one
/// of three tags) and the engine matches the name.
pub async fn tag_keys_for_resource_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Vec<String>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"SELECT DISTINCT t.key
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           JOIN tags t ON t.id = a.tag_id
           WHERE c.id = $1 AND a.schema_name = $2 AND a.table_name = $3
             AND a.column_name = $4
           ORDER BY t.key"#,
    )
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_all(&mut *conn)
    .await?)
}

/// One tag attachment the enricher would admit for a request: the tag's
/// **name** and the [`MatchType`] that attachment produces.
///
/// `name` is `tags.name`, not `tags.key`, and the distinction is the whole
/// point — see [`resolved_tags_for_resource_via`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTag {
    /// `tags.name` — the identifier the wire carries as the tag's `type`
    /// (`ServiceTags::from_assignments`) and therefore the one the engine
    /// matches tag policies on.
    pub name: String,
    /// The `MatchType` this attachment produces against the request,
    /// classified exactly as `RangerTagEnricher.findMatchingTags` does.
    pub match_type: MatchType,
}

/// The tag set an engine's `RangerTagEnricher` would resolve for ONE resource —
/// one entry per admitted attachment, carrying `tags.name` and the attachment's
/// [`MatchType`]. **This is the resolution an evaluator may act on**, unlike
/// [`tag_keys_for_resource_via`], which is exact-match attribution only.
///
/// `column = ""` in the REQUEST position means the request names the table
/// itself, mirroring migration 0081's storage convention.
///
/// # The rule, measured
///
/// `RangerTagEnricher.findMatchingTags` builds a real
/// `RangerDefaultPolicyResourceMatcher` per tag service resource and calls
/// `getMatchType` with the **tag's** resource elements in the POLICY position
/// and the **query's** resource in the request position (measured from
/// `ranger-plugins-common` bytecode, byte-identical at 2.5.0 and 2.8.0). For a
/// request naming `catalog/schema/table/column`:
///
/// | tag attached at | MatchType | admitted |
/// |---|---|---|
/// | the exact resource the request names | `SELF` | yes |
/// | the table, request names a column | `ANCESTOR` | yes |
/// | deeper than the request | `DESCENDANT` | **no** |
///
/// # Why two levels and not the enricher's four
///
/// Schema- and catalog-level attachments also yield `ANCESTOR`, and this
/// function cannot produce one because **egret's storage cannot express one**:
/// `public.catalog_tag_assignments` (migration 0081) keys on
/// `(tag_id, warehouse_id, schema_name, table_name, column_name)` with
/// `column_name text NOT NULL DEFAULT ''` and the documented convention that
/// `''` means table-level, while `schema_name` and `table_name` are `NOT NULL`
/// with **no such convention**. So column- and table-level are the only levels
/// any row here can carry (SP4d-2b D2, the user's ruling: evaluate what egret
/// can express, and refuse other payload shapes by shape). Schema- and
/// catalog-level tagging is a separate feature — a migration, a writer and a UI.
///
/// # ⚠ The comparison is CASE-INSENSITIVE, because the matcher is
///
/// **SP4d-2b Task 7 fix round 1, and the review measured the fail-open.** This
/// predicate was an exact, case-SENSITIVE `=` on all three levels. The enricher's
/// comparison is not: all four of `trino.json`'s `catalog` / `schema` / `table` /
/// `column` resources declare `RangerDefaultResourceMatcher` with
/// `{"wildCard": true, "ignoreCase": true}`, and `ignoreCase` makes
/// `RangerDefaultResourceMatcher` compare with `StringUtils.equalsIgnoreCase`.
/// Measured on the real jar for the request `hive.sales.orders.email`: a row
/// stored `('SALES','ORDERS','')` is `ANCESTOR` and admitted, and
/// `('SALES','ORDERS','EMAIL')` is `SELF` and admitted — while this query
/// returned NEITHER. egret would have reported "no tags" for a resource the
/// engine is masking, and an empty set is the licence to skip the tag phase
/// entirely.
///
/// **And that class comes out of ordinary use, not out of malformed data.**
/// `ScopePicker` renders schema / table / column as free-text inputs and
/// `request_resource` inserts them verbatim, so a steward typing `SALES` /
/// `ORDERS` against rows stored `sales` / `orders` produced it with nothing
/// wrong anywhere. Folding here REPRODUCES the matcher rather than declining
/// around it, which is why this is a widened predicate and not another gate: a
/// gate would have answered "cannot decide" to a steward who typed in the wrong
/// case, for a question egret can answer.
///
/// `tests/ranger/src/test/java/EgretTagAttachmentMatchTypeTest.java` pins the
/// Ranger half — every row above, measured against the real
/// `RangerServiceResourceMatcher.getMatchType` — so the equivalence this
/// predicate claims is measured on one side rather than asserted on both.
///
/// # ⚠ WHERE the two folds diverge is measured, and it is 25 code points
///
/// `lower()` is collation-driven FULL-STRING Unicode case mapping; Java's
/// `equalsIgnoreCase` is a PER-CHARACTER `toUpperCase`/`toLowerCase` pair after a
/// length check. Two algorithms, so this predicate reproduces the matcher only
/// where they agree — and fix round 2 measured where that is, by sweeping every
/// defined code point in Unicode:
/// `tests/ranger/src/test/java/EgretCaseFoldAgreementTest.java` finds **25**
/// divergent code points (the Turkish `İ`/`ı`, the long `ſ`, Greek `Σ`/final sigma
/// and the Greek/Cyrillic variant letters, the micro sign, the ypogegrammeni
/// pair), and **no Vietnamese letter is among them** — nor German `ß`, nor
/// Cyrillic, nor the Kelvin sign.
///
/// `egret_perm_compile`'s `fold_divergent` declines the resolution for a value
/// carrying one of those 25, on EITHER side of the comparison (the pair
/// `("ſ","s")` disagrees, so an ASCII stored value with an odd request value is
/// the same fail-open). Everything else folds here. ⚠ Fix round 1 declined every
/// non-ASCII stored value instead, which in a Vietnamese-language deployment is
/// one tag on `người` refusing a whole catalog — an over-refusal resting on an
/// un-measured premise.
///
/// The Postgres half of that measurement is a STAND-IN (`String.toLowerCase(
/// Locale.ROOT)`) and is **unverified**, exactly as this predicate itself is: no
/// Postgres runs in that harness. A collation that tailors case (`tr_TR`) would
/// move `I`/`i` into the divergent set and is the one deployment where the set is
/// too narrow.
///
/// # Three things this gets right that are each a silent fail-open
///
/// 1. **It returns `tags.name`, never `tags.key`.** The engine matches tag
///    policies on the wire tag's `type`, which `ServiceTags::from_assignments`
///    fills from `tags.name`; `tag_keys_for_resource_via` returns `tags.key`,
///    and live the two differ for one of three tags (`sp4cgate` vs
///    `sp4cgate2`). Same shape, different identifier — which is why that
///    function stays on the attribution surface and one function must not
///    return both.
/// 2. **It does not dedupe by name.** `RangerTagForEval`'s `equals`/`hashCode`
///    include `matchType`, so one tag attached at BOTH levels is TWO entries in
///    Ranger's set — and MASK evaluators are ordered by
///    `MATCH_TYPE_COMPARATOR` (ascending `SELF < SELF_AND_ALL_DESCENDANTS <
///    ANCESTOR`), so collapsing them would silently reorder masks and change
///    the masking answer. The `DISTINCT` below is over `(name, is_self)` — the
///    LEVEL as a boolean, which is exactly `RangerTagForEval`'s own equality for
///    the two levels this storage can express — never over the name alone. ⚠ Fix
///    round 1 narrowed it from `(name, column_name)`: with the comparison folded,
///    two rows differing only in case are ONE member of Ranger's set, and keeping
///    the raw value in the key would have invented a second one.
/// 3. **A column attachment is EXCLUDED from a table-level request.** That is
///    `DESCENDANT`, which the enricher rejects; returning it would fail closed
///    on `explain` and still disagree with the engine.
///
/// Rows come back ordered by match type ascending then name — deterministic,
/// and in `MATCH_TYPE_COMPARATOR`'s direction. The evaluator still applies its
/// own ordering; this order exists so the answer is stable, not so a caller can
/// skip that.
pub async fn resolved_tags_for_resource_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Vec<ResolvedTag>, PolicyStoreError> {
    // `lower(a.column_name) = lower($4)` is the SELF arm; `a.column_name = ''`
    // with a non-empty request column is the table-level ANCESTOR arm. A request
    // that names the table ($4 = '') collapses the two into the one SELF arm, and
    // a column attachment then matches NEITHER — that is the DESCENDANT
    // exclusion, and it is structural rather than a filter that could be
    // forgotten.
    //
    // **`is_self` is computed in SQL, not in Rust, and that is the point.** It
    // used to be `attached_column == column` out here, which was a SECOND
    // comparison of the same two values under a different rule — and the moment
    // the predicate folded case, the two disagreed: a row stored `EMAIL` matched
    // the SELF arm in SQL and was then labelled `Ancestor` in Rust, which is a
    // wrong match type and so a wrong mask ORDER. One comparison, one place.
    //
    // `DISTINCT` is over `(t.name, is_self)` — `RangerTagForEval`'s own equality
    // (see point 2 above) — and no longer over the raw `column_name`, because two
    // rows differing only in case are now ONE member of Ranger's set rather than
    // two.
    let rows = sqlx::query_as::<_, (String, bool)>(
        r#"SELECT DISTINCT t.name, (lower(a.column_name) = lower($4)) AS is_self
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           JOIN tags t ON t.id = a.tag_id
           WHERE c.id = $1
             AND lower(a.schema_name) = lower($2)
             AND lower(a.table_name) = lower($3)
             AND (lower(a.column_name) = lower($4) OR a.column_name = '')
           ORDER BY is_self DESC, t.name"#,
    )
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(name, is_self)| ResolvedTag {
            name,
            match_type: if is_self {
                MatchType::Self_
            } else {
                // The only other row the query can return: `column_name = ''`
                // (table level) against a request naming a column.
                MatchType::Ancestor
            },
        })
        .collect())
}

/// Every distinct `(schema_name, table_name, column_name)` this catalog's
/// `catalog_tag_assignments` rows carry — the stored coordinates, with no ruling
/// applied.
///
/// **The ruling is `egret_perm_compile`'s `stored_coordinate_is_fold_divergent`,
/// in Rust, and the split is deliberate.** Its one caller runs this BEFORE
/// [`resolved_tags_for_resource_via`] and declines the whole resolution while any
/// coordinate is outside the population that resolver reproduces, so the two
/// cannot come apart.
///
/// # ⚠ What this exists for: a MISSED row, not a returned one
///
/// **SP4d-2b Task 7.** [`resolved_tags_for_resource_via`] compares the three
/// levels for EQUALITY (case-folded since fix round 1). The enricher does not
/// compare at all — it builds a `RangerDefaultPolicyResourceMatcher` per tag
/// service resource and asks `getMatchType`, which walks the hierarchy and
/// answers `ANCESTOR` for a resource ABOVE the request. `ServiceTags::from_assignments`
/// omits an empty level, so a row stored `('sales','','')` ships as
/// `resourceElements = {catalog, schema}` — measured `ANCESTOR` of
/// `hive.sales.orders.email` on the real jar, and `findMatchingTags` admits
/// `ANCESTOR` at the default `ResourceMatchingScope.SELF`. That row can never come
/// back from an equality predicate on `table_name`. So egret would hand the
/// evaluator a set MISSING a tag the engine enforces, and an empty set is the
/// licence to skip the tag phase entirely: a wrong confident verdict.
///
/// **The missed row is invisible to the resolver's RESULT** — nothing in the
/// returned set says a broader row was skipped — so no inspection of that set can
/// detect it. It has to be detected on the stored data, which is what this reads.
///
/// **Such rows are no longer writable, and that retired two thirds of the ruling
/// but NOT this read** — SP4d debt Task 3, migration 0177
/// (`catalog_tag_assignments_levels_are_present_and_literal`, VALIDATED). It
/// forbids an empty `schema_name`/`table_name` and `*`/`?` in any level, which is
/// open item B of the SP4d-2b plan: unrepresentable beats unforgettable, and
/// `egret_bi::catalog_tags`' write-side refusal makes the same shape a 400 rather
/// than a constraint surfacing as a 500.
///
/// What a `CHECK` must NOT take is the third class, which is why this read stays: a
/// stored value may carry one of the 25 code points on which Postgres `lower()` —
/// the fold THIS module's resolver uses — and Java `equalsIgnoreCase` can disagree.
/// Those are legitimate identifiers (a table named with a Greek `Σ` is real data a
/// steward must be able to tag), and which of them diverge depends on the
/// deployment's collation, so the state has to be detected at read time and
/// declined rather than refused at the write. `stored_coordinate_is_fold_divergent`
/// is the whole surviving ruling and migration 0177's text records the argument.
///
/// # ⚠ Why the coordinates and not a `count(*)` with the predicate in SQL
///
/// **Fix round 1.** It WAS a `count(*)` whose `WHERE` clause carried the rule,
/// and the rule was wrong: it covered the empty-level class and missed the
/// wildcard and differing-case classes, both of which the review then measured as
/// the same fail-open. A predicate in SQL cannot be unit-tested or mutated
/// anywhere this branch can run — the shared test template is ahead of this
/// branch's migrations — so the miss could not have been caught by any test
/// either. The rule is now one pure Rust function with the enumeration beside it
/// and a test per class; this query returns facts.
///
/// ⚠ **Migration 0177 does put two of those classes into SQL, and that is not a
/// reversal of this paragraph.** The objection was to a rule in THIS query's
/// `WHERE` clause: unreachable from any test, because the shared test template is
/// ahead of the branch's migrations. A `CHECK` is the opposite — it is in force for
/// every writer, and `the_database_refuses_a_coordinate_the_resolver_cannot_
/// reproduce` in `tests/tag_resolution.rs` exercises it by trying the INSERTs and
/// reading back which constraint refused. What stayed in Rust is the one class that
/// must be decided per read rather than per write.
///
/// The cost is one row per distinct tagged `(schema, table, column)` in the
/// catalog's warehouse, once per `explain` request (not per profile). `explain` is
/// an interactive admin read, and `tagged_columns_for_service_via` already ships
/// per-column rows on the compile path.
///
/// Scoped exactly as the resolver is — `a.warehouse_id = c.warehouse_id` for
/// `c.id = $1` — so a coordinate this returns is one that query could have
/// matched, and a row in another warehouse (whose service resource names another
/// catalog, never an ancestor of this one) is neither returned here nor there.
pub async fn tag_assignment_coordinates_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
) -> Result<Vec<(String, String, String)>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, (String, String, String)>(
        r#"SELECT DISTINCT a.schema_name, a.table_name, a.column_name
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           WHERE c.id = $1
           ORDER BY a.schema_name, a.table_name, a.column_name"#,
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// `tags.key` for each of `tag_ids`, as `(id, key)` pairs — the reverse of the
/// lookup every other reader here needs.
///
/// **SP4d-2a Task 5.** A compiled clearance policy's NAME carries the
/// `tags.id` it was minted from (`perm-clearance-{tag_id}[-{locator}]`), and
/// `egret_perm_compile::attribute_policy_name` hands that bare uuid back
/// without touching the database. `explain` has to turn it into something a
/// steward recognises, and into the `required_roles` the gate declares — which
/// [`clearance_gates_for_tags_via`] answers by KEY, not by id. So this is the
/// one hop between them.
///
/// It returns `tags.key`, not `tags.name`: `key` is the stable identifier the
/// rest of this module's tag surface is addressed by
/// ([`tag_keys_for_resource_via`], [`clearance_gates_for_tags_via`]), and the
/// two columns are different values — `tag_name_for_key_via` exists precisely
/// because the display name has to be looked up separately.
pub async fn tag_keys_by_ids_via(
    conn: &mut sqlx::PgConnection,
    tag_ids: &[uuid::Uuid],
) -> Result<Vec<(uuid::Uuid, String)>, PolicyStoreError> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as::<_, (uuid::Uuid, String)>(r#"SELECT id, key FROM tags WHERE id = ANY($1)"#)
            .bind(tag_ids)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// The clearance gate for each of `tag_keys` that `egret_perm::evaluate` can
/// actually agree with the compiled engine about (SP4c Task 4/D4): one
/// `(tag key, required_roles)` pair per gated tag whose `required_roles` is
/// non-empty. A `required_groups`-only gate is deliberately excluded — D2/D7
/// measured that a group name matches nobody in the engine, so
/// `egret_perm_compile::clearance` compiles it to NO deny at all, and
/// `explain` reporting a denial for it would disagree with the engine in the
/// direction opposite the bug SP4c Task 4 fixes.
pub async fn clearance_gates_for_tags_via(
    conn: &mut sqlx::PgConnection,
    tag_keys: &[String],
) -> Result<Vec<(String, Vec<String>)>, PolicyStoreError> {
    if tag_keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, (String, Vec<String>)>(
        r#"SELECT t.key, cp.required_roles
             FROM classification_policies cp
             JOIN tags t ON t.id = cp.tag_id
            WHERE t.key = ANY($1) AND cardinality(cp.required_roles) > 0"#,
    )
    .bind(tag_keys)
    .fetch_all(&mut *conn)
    .await?)
}

/// Connection-generic core of [`PolicyStore::tagged_columns_for_service`] —
/// same rationale as [`get_service_by_id_via`].
pub async fn tagged_columns_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    tag_key: &str,
) -> Result<Vec<TaggedColumnRow>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, TaggedColumnRow>(
        r#"SELECT DISTINCT c.name AS catalog, a.schema_name, a.table_name, a.column_name
           FROM catalog_tag_assignments a
           JOIN tags t ON t.id = a.tag_id
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE e.authz_service_id = $1 AND t.key = $2 AND a.column_name <> ''
           ORDER BY c.name, a.schema_name, a.table_name, a.column_name"#,
    )
    .bind(service_id)
    .bind(tag_key)
    .fetch_all(&mut *conn)
    .await?)
}

/// Connection-generic core of [`PolicyStore::tag_name_for_key`] — same
/// rationale as [`get_service_by_id_via`].
pub async fn tag_name_for_key_via(
    conn: &mut sqlx::PgConnection,
    key: &str,
) -> Result<Option<String>, PolicyStoreError> {
    Ok(
        sqlx::query_scalar::<_, String>(r#"SELECT name FROM tags WHERE key = $1"#)
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// The `(id, name)` of each catalog in `ids` that still exists, in one round
/// trip.
///
/// **Why a batch lookup and not `catalogs_for_service_via`.** That one answers
/// "which catalogs does this profile reach", which is a different question and
/// needs an engine binding; a grant or rule names a catalog whether or not any
/// engine is attached to it yet. The perm read surface uses this to put the
/// catalog's NAME on every grant and rule it returns (SP4b-2 finding 1): a
/// client holding `policy:read` alone cannot call the catalog registry, so a
/// client-side id→name lookup degraded the one column those pages exist to
/// show into a raw uuid. The names are resolved here, where no extra
/// permission is involved.
///
/// An id with no row is simply absent from the result (a catalog deleted
/// between the grant read and this call) — never an error, and never a
/// fabricated name.
pub async fn catalog_names_for_ids_via(
    conn: &mut sqlx::PgConnection,
    ids: &[uuid::Uuid],
) -> Result<Vec<CatalogRow>, PolicyStoreError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, CatalogRow>(
        r#"SELECT id, name FROM registry.catalogs WHERE id = ANY($1)"#,
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// Is `catalog_id` assigned to `workspace_id` in `registry.catalog_workspaces`?
///
/// **This is the relation the `perm` model itself treats as "this catalog
/// belongs to that workspace".** `registry.catalogs` has no `workspace_id`
/// column — a catalog is a platform resource (migration 0147) that workspaces
/// are *assigned* — and `perm.grants`' implicit `workspace_assignment` rows are
/// derived from exactly this table by
/// `egret_perm::sync_assignment_grants_in_tx`. The other candidate shape,
/// `catalog.warehouses.workspace_id`, answers a different question (which
/// workspace OWNS the warehouse a catalog mirrors) and is `NULL` for every
/// catalog that is not an egret-metastore one, so a guard keyed to it would
/// read "belongs to no workspace" for most of the registry.
///
/// Read-only, connection-generic, and `EXISTS` rather than a row fetch: the one
/// caller ([`egret_perm_compile`]'s `explain`) needs the boolean and nothing
/// else. **Writing this table is deliberately NOT possible from here** — see
/// `egret_catalog_registry::RegistryStore::set_catalog_workspaces_in_tx`, which
/// is the only writer precisely so no path can assign a catalog without
/// syncing the implicit grant.
pub async fn catalog_is_assigned_to_workspace_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
) -> Result<bool, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (SELECT 1 FROM registry.catalog_workspaces
                           WHERE catalog_id = $1 AND workspace_id = $2)"#,
    )
    .bind(catalog_id)
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?)
}

/// The `(key, name)` of each tag in `keys` that exists — the batch form of
/// [`tag_name_for_key_via`], for the same reason
/// [`catalog_names_for_ids_via`] exists. `tags.name` is the display name;
/// `tags.key` is what a rule stores (D6), and the two differ (`pii` / `PII`).
pub async fn tag_names_for_keys_via(
    conn: &mut sqlx::PgConnection,
    keys: &[String],
) -> Result<Vec<(String, String)>, PolicyStoreError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as::<_, (String, String)>(r#"SELECT key, name FROM tags WHERE key = ANY($1)"#)
            .bind(keys)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// Connection-generic core of [`PolicyStore::get_service_def`] — SP4d-3 Task
/// 7 assembles a profile's download INSIDE its migration transaction, before
/// and after the write, and a pooled read there would not see the write.
pub async fn get_service_def_via(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<ServiceDefRow, PolicyStoreError> {
    sqlx::query_as::<_, ServiceDefRow>(
        r#"SELECT id, name, def_jsonb FROM policy.authz_service_defs WHERE name = $1"#,
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| PolicyStoreError::NotFound(format!("service-def {name}")))
}

/// Connection-generic core of [`PolicyStore::create_policy`] (and its infra
/// twin): the same owner/name guards, the same refusal order, the INSERT and
/// the `policy_version` bump, on the CALLER's transaction. Does not begin or
/// commit (SP4d-4 Task 3 fix round 1, review I1).
pub async fn insert_policy_owned_by_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    name: &str,
    policy: &Value,
    managed_by: &str,
) -> Result<i64, PolicyStoreError> {
    // Fix round 3's reserved-namespace check and Task 8's origin check, in
    // the one order they are allowed to fire in — spelled once, in
    // `refuse_before_policy_insert`, and shared with
    // `preflight_policy_insert`. The reserved-name reason wins; see that
    // function's doc comment for the ruling and for the test that pins it.
    //
    // First, the SP4d-3 owner/name agreement, which needs the profile's
    // type: a human writer may not take an infrastructure name, and the
    // infra writer may take nothing else. A refusal leaves the caller's transaction to roll back.
    let service_type = service_type_of(&mut *conn, service_id).await?;
    if managed_by == "infra" {
        refuse_non_infrastructure_name(&service_type, name)?;
    } else {
        refuse_infrastructure_name(&service_type, name)?;
    }
    refuse_before_policy_insert(&mut *conn, service_id, name).await?;
    let id: i64 = sqlx::query_scalar::<_, i64>(
        r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by)
               VALUES ($1,$2,$3,$4) RETURNING id"#,
    )
    .bind(service_id)
    .bind(name)
    .bind(policy)
    .bind(managed_by)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            PolicyStoreError::AlreadyExists(name.into())
        }
        _ => PolicyStoreError::Db(e),
    })?;
    sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// Connection-generic core of [`PolicyStore::update_policy`]: the same guards
/// in the same order, the UPDATE and the `policy_version` bump, on the CALLER's
/// transaction. Does not begin or commit (SP4d-4 Task 3 fix round 1, I1).
pub async fn update_policy_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
    name: &str,
    policy: &Value,
    enabled: bool,
) -> Result<(), PolicyStoreError> {
    // Fetch the policy's CURRENT name — ownership (Task 8 fix round 2,
    // I-1) is decided by what this policy IS today, not by the new name
    // the caller is trying to write.
    let row: Option<(i64, String, String)> = sqlx::query_as::<_, (i64, String, String)>(
        r#"SELECT p.service_id, p.name, s.type FROM policy.authz_policies p
               JOIN policy.authz_services s ON s.id = p.service_id WHERE p.id = $1"#,
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((service_id, current_name, service_type)) = row else {
        return Err(PolicyStoreError::NotFound(format!("policy {id}")));
    };
    refuse_if_compiler_owned(&mut *conn, service_id, &current_name).await?;
    refuse_if_infrastructure_owned(&mut *conn, id).await?;
    // Fix round 3: reserved-namespace check on the NEW/target name, AFTER
    // the ownership check above — an already-owned policy (caught above,
    // whatever `name` the caller resent) gets the ownership-specific
    // message ("this is compiler-owned, edit the grant/rule"), which is
    // both more accurate and less confusing than "rename this" for a
    // name the compiler itself is the one using legitimately. This check
    // is what catches the round-2 finding's OTHER vector instead: a
    // human renaming an ordinary (not-yet-owned) policy INTO the
    // compiler's namespace, in ANY profile including `manual` — where
    // the ownership check above does not fire at all (it requires
    // `origin = 'generated'`, and the whole point of the round-2 finding
    // was that a `manual` profile is not yet flagged as anything).
    refuse_reserved_name(name)?;
    // SP4d-3 Task 2: and a rename INTO an infrastructure name. Only a
    // CHANGE of name: the editor resends the current name on every save,
    // and a row that already carries one (it is `'operator'` here, since an
    // `'infra'` row was refused above) must stay editable.
    if name != current_name {
        refuse_infrastructure_name(&service_type, name)?;
    }
    sqlx::query(
            r#"UPDATE policy.authz_policies SET name=$2, policy_jsonb=$3, enabled=$4, version=version+1, updated_at=now() WHERE id=$1"#,
        )
        .bind(id)
        .bind(name)
        .bind(policy)
        .bind(enabled)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// **The one spelling of "lock these profiles' rows"**: `SELECT … FOR UPDATE`
/// on each `policy.authz_services` row in `service_ids`, one statement per id,
/// in the ascending order given. Every writer that serialises on a profile
/// takes its rows through here — the compiler's
/// `egret_perm_compile::trigger::lock_services_in_tx` (after expanding the set
/// with [`services_and_tag_services_via`]) and the policy editor's overlap check
/// (SP4d-4 Task 3 fix round 1, review I1) — so every path acquires the same
/// universe of rows in the same order and two of them queue rather than
/// deadlock.
///
/// One statement per id rather than `WHERE id = ANY(...) ORDER BY id FOR
/// UPDATE`: Postgres does not promise that a sort is applied before row locks
/// are taken, and the ORDER is the entire point here.
pub async fn lock_service_rows_via(
    conn: &mut sqlx::PgConnection,
    service_ids: &[i64],
) -> Result<(), PolicyStoreError> {
    debug_assert!(
        service_ids.is_sorted(),
        "lock_service_rows_via requires ascending service ids, got {service_ids:?}"
    );
    for &service_id in service_ids {
        sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
            .bind(service_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Connection-generic core of [`PolicyStore::enabled_policies_for_service`] —
/// the ONE assembler of the policies a download serves, so the migration gate
/// (SP4d-3 Task 7) and the download route read the same rows the same way.
pub async fn enabled_policies_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<Value>, PolicyStoreError> {
    // Inject the DB-assigned numeric `id`, `version`, and `isEnabled` into each policy.
    // The Ranger policy engine NPEs on a policy whose `id` is null, and clients only send
    // these on the response/CRUD path — the stored body keeps the client's original JSON.
    let rows = sqlx::query_as::<_, (i64, i64, bool, Value)>(
        r#"SELECT id, version, enabled, policy_jsonb FROM policy.authz_policies
           WHERE service_id = $1 AND enabled = true ORDER BY id"#,
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, version, enabled, mut p)| {
            if let Value::Object(m) = &mut p {
                m.insert("id".into(), Value::from(id));
                m.insert("version".into(), Value::from(version));
                m.insert("isEnabled".into(), Value::from(enabled));
            }
            p
        })
        .collect())
}

/// Connection-generic core of [`PolicyStore::set_policy_enabled`], with the
/// SAME two guards in the same order (a compiler-owned name in a generated
/// profile, then an `infra` row). SP4d-3 Task 7 switches a migrated original
/// off — and `undo` switches it back on — inside the migration's own
/// transaction, so the flip commits or rolls back with the model write; a
/// second, unguarded `UPDATE` there would be a write path the guards do not
/// cover. Does not begin or commit.
pub async fn set_policy_enabled_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
    enabled: bool,
) -> Result<(), PolicyStoreError> {
    let row: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
        r#"SELECT service_id, name FROM policy.authz_policies WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((service_id, current_name)) = row else {
        return Err(PolicyStoreError::NotFound(format!("policy id {id}")));
    };
    refuse_if_compiler_owned(conn, service_id, &current_name).await?;
    refuse_if_infrastructure_owned(conn, id).await?;
    sqlx::query(
        r#"UPDATE policy.authz_policies SET enabled=$2, version=version+1, updated_at=now() WHERE id=$1"#,
    )
    .bind(id)
    .bind(enabled)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
    )
    .bind(service_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Delete policy `id` on the caller's connection (final review I-3: the policy
/// editor recompiles a StarRocks profile in the same transaction). The
/// refusals of [`PolicyStore::delete_policy`], in its order; a refusal leaves
/// the caller's transaction to roll back.
pub async fn delete_policy_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<(), PolicyStoreError> {
    // Look the policy up BEFORE deleting — Task 8's guard must refuse the
    // delete outright, not delete-then-refuse (which would leave the policy
    // gone despite the 409).
    let row: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
        r#"SELECT service_id, name FROM policy.authz_policies WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((service_id, current_name)) = row else {
        return Err(PolicyStoreError::NotFound(format!("policy {id}")));
    };
    refuse_if_compiler_owned(conn, service_id, &current_name).await?;
    refuse_if_infrastructure_owned(conn, id).await?;
    // SP4d-3 Task 7 (review I-3): a migrated original is referenced by
    // `perm.migrated_from.policy_id` (ON DELETE NO ACTION, migration 0181).
    // Deleting it would make the undo impossible, so the violation is
    // answered as an ownership refusal (409) naming the remedy, not a 500.
    if let Err(e) = sqlx::query(r#"DELETE FROM policy.authz_policies WHERE id = $1"#)
        .bind(id)
        .execute(&mut *conn)
        .await
    {
        if matches!(&e, sqlx::Error::Database(d)
            if d.is_foreign_key_violation()
                && d.constraint() == Some("migrated_from_policy_id_fkey"))
        {
            return Err(PolicyStoreError::CompilerOwned(format!(
                "policy {current_name:?} was migrated into the permission model and is \
                 kept, switched off, so the migration can be undone; undo the migration \
                 before deleting it"
            )));
        }
        return Err(e.into());
    }
    sqlx::query(
        r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
    )
    .bind(service_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// One registry catalog: `(id, name)`.
#[derive(Debug, sqlx::FromRow)]
pub struct CatalogRow {
    pub id: uuid::Uuid,
    pub name: String,
}

/// One tagged column, Ranger-facing names only (see `tagged_columns_for_service`).
#[derive(Debug, sqlx::FromRow)]
pub struct TaggedColumnRow {
    pub catalog: String,
    pub schema_name: String,
    pub table_name: String,
    pub column_name: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct PolicyRow {
    pub id: i64,
    pub service_id: i64,
    pub name: String,
    pub policy_jsonb: Value,
    pub enabled: bool,
    pub version: i64,
    /// Who owns the row: `'operator'`, `'compiler'` or `'infra'` (migration
    /// 0180). The profile page reads it to withhold edit controls the server
    /// would refuse.
    pub managed_by: String,
}

impl PolicyStore {
    /// Create a policy under a service and bump policy_version atomically. Returns the new policy id.
    pub async fn create_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        self.insert_policy_owned_by(service_id, name, policy, "operator")
            .await
    }

    /// [`create_policy`](Self::create_policy) for egret's OWN infrastructure
    /// policies (the delegator's two): same guards and errors, but the row is
    /// stamped `managed_by = 'infra'`. Refuses a name that
    /// [`is_egret_infrastructure_policy`] does not recognise on the profile's
    /// type, so this writer cannot drift from the predicate that defines the
    /// owner.
    pub async fn create_infra_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        self.insert_policy_owned_by(service_id, name, policy, "infra")
            .await
    }

    async fn insert_policy_owned_by(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
        managed_by: &str,
    ) -> Result<i64, PolicyStoreError> {
        // A refusal drops `tx` (rollback).
        let mut tx = self.db.pool.begin().await?;
        let id = insert_policy_owned_by_via(&mut tx, service_id, name, policy, managed_by).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Begin a transaction on the store's pool — for a caller that has to run
    /// a check and a `*_via` write under one set of row locks (SP4d-4 Task 3
    /// fix round 1, review I1: the policy editor's overlap check).
    pub async fn begin(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, PolicyStoreError> {
        Ok(self.db.pool.begin().await?)
    }

    /// [`upsert_policy`](Self::upsert_policy) for egret's OWN infrastructure
    /// policies (`infra_names`): identical, except the row is stamped
    /// `managed_by = 'infra'` rather than the human default `'operator'`.
    /// On conflict the owner is re-stamped too, so a row an operator
    /// pre-created under an infrastructure name becomes infrastructure.
    pub async fn upsert_infra_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        refuse_reserved_name(name)?;
        let mut tx = self.db.pool.begin().await?;
        let service_type = service_type_of(&mut tx, service_id).await?;
        refuse_non_infrastructure_name(&service_type, name)?;
        let id: i64 = sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by)
               VALUES ($1,$2,$3,'infra')
               ON CONFLICT (service_id, name) DO UPDATE
                 SET policy_jsonb = EXCLUDED.policy_jsonb,
                     managed_by = 'infra',
                     version = policy.authz_policies.version + 1,
                     updated_at = now()
               RETURNING id"#,
        )
        .bind(service_id)
        .bind(name)
        .bind(policy)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Insert a policy, or update its body in place if `(service_id, name)`
    /// already exists, bumping the policy's `version` and the service's
    /// `policy_version` atomically. Returns the policy id.
    ///
    /// **Guarded by the reserved-name check and by nothing else, deliberately**
    /// (whole-branch review, Important 6). [`refuse_if_generated`] must NOT be
    /// added here: this method's only caller is
    /// `default_policies::seed_default_trino_policies`, `engines.rs` propagates
    /// a seed failure as a 400, and a profile the compiler has claimed is
    /// exactly the one an engine update would then be unable to save — the
    /// exemption is load-bearing, not an oversight. [`refuse_reserved_name`] is
    /// pure and name-only, so it cannot fail that seed (its
    /// `egret-admin-*` names collide with nothing), and it closes the one way
    /// this path could drift: a future second caller passing a
    /// `perm-grant-`/`perm-rule-` name would silently overwrite compiled
    /// output, which the next recompile restores — an edit that appears to
    /// work and then vanishes.
    pub async fn upsert_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        refuse_reserved_name(name)?;
        let mut tx = self.db.pool.begin().await?;
        let service_type = service_type_of(&mut tx, service_id).await?;
        refuse_infrastructure_name(&service_type, name)?;
        let id: i64 = sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb)
               VALUES ($1,$2,$3)
               ON CONFLICT (service_id, name) DO UPDATE
                 SET policy_jsonb = EXCLUDED.policy_jsonb,
                     version = policy.authz_policies.version + 1,
                     updated_at = now()
               RETURNING id"#,
        )
        .bind(service_id)
        .bind(name)
        .bind(policy)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn update_policy(
        &self,
        id: i64,
        name: &str,
        policy: &Value,
        enabled: bool,
    ) -> Result<(), PolicyStoreError> {
        // A refusal drops `tx` (rollback).
        let mut tx = self.db.pool.begin().await?;
        update_policy_via(&mut tx, id, name, policy, enabled).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Flip a policy's enabled flag without rewriting its body; bumps versions.
    pub async fn set_policy_enabled(&self, id: i64, enabled: bool) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        // A refusal drops `tx`, which rolls it back — nothing was written yet.
        set_policy_enabled_via(&mut tx, id, enabled).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_policy(&self, id: i64) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        // A refusal drops `tx`, which rolls it back.
        delete_policy_via(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_policy(&self, id: i64) -> Result<PolicyRow, PolicyStoreError> {
        sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version, managed_by FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("policy {id}")))
    }

    /// All policies for a service (enabled and disabled), newest first — the
    /// admin list view. Returns typed rows; unlike `enabled_policies_for_service`
    /// this does not inject id/version into the body (the caller/summary does that).
    pub async fn list_policies(&self, service_id: i64) -> Result<Vec<PolicyRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version, managed_by
               FROM policy.authz_policies WHERE service_id = $1 ORDER BY id DESC"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Enabled policies of a service (for download assembly).
    pub async fn enabled_policies_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<Value>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        enabled_policies_for_service_via(&mut conn, service_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_def::{
        STARROCKS_SERVICE_DEF_JSON, TAG_SERVICE_DEF_JSON, TRINO_SERVICE_DEF_JSON,
    };
    use serde_json::json;

    fn resource_names(def_json: &str) -> Vec<String> {
        let def: Value = serde_json::from_str(def_json).expect("parse service-def");
        def["resources"]
            .as_array()
            .expect("resources array")
            .iter()
            .map(|r| r["name"].as_str().expect("resource name").to_owned())
            .collect()
    }

    /// **The measurement the refusal rests on, asserted against the embedded
    /// JSON rather than quoted from a comment.** `ServiceTags::from_assignments`
    /// writes `catalog`/`schema`/`table`/`column` for every profile type, so the
    /// rule "only a `trino` profile may carry a `tagService`" is sound exactly
    /// while `trino` is the only seeded def with a `schema` resource. A def
    /// gaining or losing one fails HERE, which is the point: the next reader
    /// gets a failing test, not a stale sentence.
    #[test]
    fn trino_is_the_only_seeded_service_def_with_a_schema_resource() {
        let trino = resource_names(TRINO_SERVICE_DEF_JSON);
        let starrocks = resource_names(STARROCKS_SERVICE_DEF_JSON);
        let tag = resource_names(TAG_SERVICE_DEF_JSON);
        assert!(
            trino.iter().any(|r| r == "schema"),
            "trino def must declare `schema`; got {trino:?}"
        );
        assert!(
            !starrocks.iter().any(|r| r == "schema"),
            "starrocks def must NOT declare `schema`; got {starrocks:?}"
        );
        assert!(
            starrocks.iter().any(|r| r == "database"),
            "starrocks' middle level is `database`; got {starrocks:?}"
        );
        assert!(
            !tag.iter().any(|r| r == "schema"),
            "the tag def must NOT declare `schema`; got {tag:?}"
        );
    }

    /// Four SQL statements and migration 0176 name this key as a literal and
    /// cannot reference the const (interpolating it would mean building the
    /// statement with `format!`, the dynamic-SQL shape this repository avoids), so
    /// changing the const's VALUE would silently split the Rust readers from the
    /// SQL ones — `usable_tag_service` would join on a key nothing writes, and the
    /// `CHECK` would stop guarding the field the refusal guards. This is the only
    /// assertion that can catch that, and it is why the test fixtures below spell
    /// the literal out instead of using the const: an oracle built from the thing
    /// it checks inherits its blind spot.
    ///
    /// Changing the value means rewriting all five statements and a new migration.
    #[test]
    fn the_const_is_the_key_the_sql_and_the_migration_name() {
        assert_eq!(
            TAG_SERVICE_CONFIG_KEY, "tagService",
            "the const no longer matches the literal in `usable_tag_service`, \
             `tag_assignments_for_service`, `services_and_tag_services_via`, \
             `bump_bound_tag_versions` and migration 0176's CHECK/UPDATE — none of \
             which can reference it"
        );
    }

    /// **The tripwire the three-const test above does not give you.** That test
    /// asserts the three defs it NAMES, so editing one fails — but ADDING a
    /// fourth `src/service-defs/*.json` passes it in silence, and an addition is
    /// the dangerous direction: a def that declares a `schema` resource could
    /// legitimately carry a `tagService`, and
    /// [`refuse_tag_service_on_non_trino_profile`] would refuse it anyway because
    /// its rule is "only `trino`". So this reads the DIRECTORY rather than the
    /// three `include_str!` consts, and holds two things at once:
    ///
    /// 1. the file set is exactly the three the refusal was ruled on — so any
    ///    addition fails here and gets re-ruled, whatever it contains;
    /// 2. for every file actually present, declaring a `schema` resource implies
    ///    the def is named `trino` — the property the refusal rests on, stated
    ///    over whatever is on disk instead of over a list.
    ///
    /// Assertion 2 is not redundant with assertion 1: it is what survives when
    /// someone updates the list in 1 without re-reading the rule, which is the
    /// usual way a tripwire gets defused.
    #[test]
    fn service_def_set_is_exactly_the_three_this_refusal_was_ruled_on() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service-defs");
        let mut found: Vec<(String, Value)> = std::fs::read_dir(&dir)
            .expect("read src/service-defs")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .map(|p| {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .expect("utf-8 filename")
                    .to_owned();
                let def: Value = serde_json::from_str(
                    &std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}")),
                )
                .unwrap_or_else(|e| panic!("parse {p:?}: {e}"));
                (name, def)
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));

        let files: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            files,
            vec!["starrocks.json", "tag.json", "trino.json"],
            "a service-def was ADDED or REMOVED. `{}` refuses a `tagService` on \
             every type but `trino`, and that is only right while `trino` is the \
             only def declaring a `schema` resource. Re-rule the new def: if it \
             has no `schema` resource the refusal is already correct for it (add \
             it to this list and to the per-file check below); if it HAS one, the \
             refusal is now an OVER-refusal and the rule has to change, not the \
             list.",
            "refuse_tag_service_on_non_trino_profile"
        );

        for (file, def) in &found {
            let def_name = def["name"].as_str().expect("def name");
            let has_schema = def["resources"]
                .as_array()
                .expect("resources array")
                .iter()
                .any(|r| r["name"].as_str() == Some("schema"));
            if has_schema {
                assert_eq!(
                    def_name, "trino",
                    "{file} declares a `schema` resource, so a `tagService` on a \
                     {def_name:?} profile WOULD be servable — but \
                     `refuse_tag_service_on_non_trino_profile` refuses every type \
                     but `trino`. That is an over-refusal; widen the rule."
                );
            }
        }
    }

    /// Step 1's failing test: the refusal fires for a StarRocks profile and the
    /// message NAMES the level StarRocks lacks, so an operator reading the 400
    /// learns why rather than being told no.
    #[test]
    fn tag_service_on_a_starrocks_profile_is_refused_and_names_schema() {
        let err = refuse_tag_service_on_non_trino_profile(
            "starrocks",
            &json!({ "tagService": "sr_tag" }),
        )
        .expect_err("a starrocks profile carrying a tagService must be refused");
        assert!(
            matches!(err, PolicyStoreError::BadRequest(_)),
            "must be BadRequest (400), not a 409/500: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("schema"), "message must name `schema`: {msg}");
        assert!(
            msg.contains("database"),
            "message must name StarRocks' actual middle level: {msg}"
        );
        assert!(
            msg.contains("starrocks") && msg.contains("tagService"),
            "message must name the type and the config key: {msg}"
        );
    }

    /// The only combination that is serveable today.
    #[test]
    fn tag_service_on_a_trino_profile_is_allowed() {
        refuse_tag_service_on_non_trino_profile("trino", &json!({ "tagService": "p_tag" })).expect(
            "a trino profile carrying a tagService is exactly what bind_trino_profile writes",
        );
    }

    /// Not an over-refusal: every StarRocks profile egret has today carries `{}`,
    /// and `bind_trino_profile` writes `{}` for the `tag` service itself.
    #[test]
    fn a_non_trino_profile_without_a_tag_service_is_allowed() {
        for config in [
            json!({}),
            json!({ "jdbc.url": "jdbc:mysql://fe:9030" }),
            // JSON `null` is the one shape `config_jsonb ->> 'tagService'`
            // answers NULL for even though the KEY is present — the refusal and
            // the CHECK must agree on it, so it is asserted, not assumed.
            json!({ "tagService": null }),
        ] {
            refuse_tag_service_on_non_trino_profile("starrocks", &config)
                .unwrap_or_else(|e| panic!("must be allowed for {config}: {e}"));
            refuse_tag_service_on_non_trino_profile("tag", &config)
                .unwrap_or_else(|e| panic!("must be allowed for {config}: {e}"));
        }
    }

    /// A `tag`-typed service pointing at another tag service is the same silent
    /// inertness (the tag def has no `schema` either — see the measurement test),
    /// and an unknown type is refused by DEFAULT rather than inheriting the
    /// silence. `type` is `REFERENCES authz_service_defs(name)` so only a seeded
    /// def can reach the row, but the predicate must not depend on that.
    #[test]
    fn every_non_trino_type_is_refused_not_just_starrocks() {
        for typ in ["starrocks", "tag", "hive", ""] {
            let err = match refuse_tag_service_on_non_trino_profile(
                typ,
                &json!({ "tagService": "whatever" }),
            ) {
                Ok(()) => panic!("type {typ:?} carrying a tagService must be refused"),
                Err(e) => e,
            };
            assert!(
                matches!(err, PolicyStoreError::BadRequest(_)),
                "type {typ:?}: must be BadRequest, got {err:?}"
            );
            assert!(
                err.to_string().contains("schema"),
                "type {typ:?}: message must name the missing level: {err}"
            );
        }
    }

    /// The shape agreement with migration 0176's `CHECK (… config_jsonb ->>
    /// 'tagService' IS NULL)`. `->>` answers non-NULL for a number, a bool, an
    /// object and an array — none of which `download_policies`' `as_str()`
    /// reads — so the Rust predicate must refuse them too, or a config of
    /// `{"tagService": 7}` passes the route and trips the constraint as a 500.
    #[test]
    fn the_predicate_is_key_presence_exactly_like_postgres_right_arrow_arrow() {
        for v in [
            json!(7),
            json!(true),
            json!(""),
            json!({ "nested": 1 }),
            json!([1, 2]),
        ] {
            assert!(
                carries_tag_service(&json!({ "tagService": v.clone() })),
                "`->>` is non-NULL for {v}, so the refusal must see it too"
            );
            refuse_tag_service_on_non_trino_profile("starrocks", &json!({ "tagService": v }))
                .expect_err("must be refused");
        }
        // A non-object `config_jsonb`: `->>` yields NULL rather than erroring
        // (measured on PostgreSQL 16.15), and `Value::get` yields `None`.
        for config in [json!(5), json!("x"), json!([]), json!(null)] {
            assert!(
                !carries_tag_service(&config),
                "a non-object config carries no tagService: {config}"
            );
        }
    }

    /// SP4d debt Task 9 step 4 — the VALUE rule, and the **accept control is in
    /// the same test** so an over-refusal cannot hide behind the refusals.
    ///
    /// Every refused shape here reads as "no tag service" through `as_str()`,
    /// which is how `download_policies`, `trigger.rs`, `list_profiles` and the
    /// tag editor's write target all read the field — so the profile would serve
    /// a download with no `tagPolicies` block while three other readers quietly
    /// disagreed about it. The accepted shape is exactly what
    /// `bind_trino_profile` writes.
    #[test]
    fn a_tag_service_value_must_name_something() {
        for v in [
            json!(""),
            json!(" "),
            json!("\t\n"),
            json!(7),
            json!(true),
            json!({ "name": "p_tag" }),
            json!(["p_tag"]),
        ] {
            let err = match refuse_unnameable_tag_service(&json!({ "tagService": v.clone() })) {
                Ok(()) => panic!("a tagService of {v} names no policy profile and must be refused"),
                Err(e) => e,
            };
            assert!(
                matches!(err, PolicyStoreError::BadRequest(_)),
                "{v}: must be BadRequest (a 400 with the reason), got {err:?}"
            );
            assert!(
                err.to_string().contains("tagService"),
                "{v}: the message must name the key to fix: {err}"
            );
        }
        // ACCEPT control: the shape `bind_trino_profile` writes, plus a config
        // that carries other keys alongside it.
        refuse_unnameable_tag_service(&json!({ "tagService": "trino-prod_tag" }))
            .expect("a real tag profile name must be accepted");
        refuse_unnameable_tag_service(&json!({ "tagService": "p_tag", "jdbc.url": "x" }))
            .expect("other config keys are none of this rule's business");
    }

    /// JSON `null` and an absent key are the SAME thing to every other reader of
    /// this field — [`carries_tag_service`], Postgres' `->>`, migration 0176's
    /// `CHECK`, and `Value::get` all say so — so the value rule reuses that
    /// presence predicate instead of spelling a third reading of "present".
    /// Refusing `null` here would make `{"tagService": null}` a 400 while the
    /// `CHECK` and the type rule both treat it as unset.
    #[test]
    fn an_absent_or_null_tag_service_is_not_this_rules_business() {
        for config in [
            json!({}),
            json!({ "tagService": null }),
            json!({ "jdbc.url": "x" }),
            // Non-object configs: `Value::get` yields `None`, same as `->>`.
            json!(5),
            json!("x"),
            json!([]),
            json!(null),
        ] {
            refuse_unnameable_tag_service(&config)
                .unwrap_or_else(|e| panic!("{config} carries no tagService, but was refused: {e}"));
        }
    }

    /// F11 (spec E7): compiled engine grants are `perm-engine-{uuid}`. The
    /// prefix is reserved like the other three, and only with its dash: a
    /// policy an operator calls `perm-engines` or `perm-engine` is theirs.
    #[test]
    fn the_engine_grant_prefix_is_compiler_owned_and_reserved() {
        assert_eq!(COMPILER_ENGINE_GRANT_PREFIX, "perm-engine-");
        assert!(is_compiler_owned_policy_name(
            "perm-engine-0f0e0d0c-0b0a-0908-0706-050403020100"
        ));
        assert!(!is_compiler_owned_policy_name("perm-engines"));
        assert!(!is_compiler_owned_policy_name("perm-engine"));
        match refuse_reserved_name("perm-engine-x") {
            Err(PolicyStoreError::CompilerOwned(msg)) => assert!(
                msg.contains("\"perm-engine-\""),
                "the refusal must name the reserved prefix: {msg}"
            ),
            other => panic!("expected CompilerOwned, got {other:?}"),
        }
    }
}
