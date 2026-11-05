//! Renders a grant's [`Statement`] into a Ranger `AccessPolicyInput`
//! (governance sub-project 1, Task 5). Every decision — scope, skip,
//! refusal, principal checks — is `egret_perm_ir::lower::grants`'s; this
//! module renders shape only. Read that module's doc for why a `User` ref is
//! emitted verbatim and why groups, roles and workspaces arrive expanded.
//!
//! One thing a reader can't check by eye, pinned by `tests/compile_grants.rs`:
//!
//! - **The principal subtract compiles to `allowExceptions`** (or
//!   `denyExceptions` for a deny grant). That four-way Ranger structure
//!   (items / denyItems / allowExceptions / denyExceptions) is a
//!   COMPILATION TARGET ONLY — it appears nowhere in `perm`'s model types,
//!   egret's API, or any UI-facing name (global constraint). `PrincipalSet`
//!   (`include` minus `exclude`) is the only shape a human or the OpenAPI
//!   schema ever sees, and the IR's `Audience` (`include` / `except`) is its
//!   expanded form; this module is where that subtract is realized as
//!   Ranger's exception block, and only here.

use std::collections::BTreeMap;

// `egret-policy` keeps `admin::ranger_json` module-private (D2: egret must
// never emit a tag *access* policy, and that module's tag builders sit next
// to the ones used here) and re-exports just these three struct types from
// `admin` itself.
use egret_policy::admin::{
    access_to_ranger_json, AccessItemInput, AccessPolicyInput, ResourceLevel,
};
use egret_policy::COMPILER_GRANT_PREFIX;

use egret_perm::{engine_accesses, Action, CompileError, Effect, EngineType, Grant, Scope};
use egret_perm_ir::audience::{Member, MemberName};
use egret_perm_ir::lower::grants::grant_statement;
use egret_perm_ir::statement::{self, Level, Privilege, Source, Statement, Target};

use super::{Rendered, SourceKind};

/// Sort `members` into a Ranger item's `users`/`groups` buckets, each in
/// member order. The members were checked by the lowering; a `Role` or
/// `WorkspaceMembers` still present (only reachable from a statement built by
/// hand) is refused rather than dropped, for the reason
/// `egret_perm_ir::lower::grants`' module doc gives. Shared with the rule
/// items (`crate::ranger::rules`), so an access item and a mask or row-filter
/// item bucket a principal alike.
pub(crate) fn member_buckets(
    members: &[Member],
) -> Result<(Vec<String>, Vec<String>), CompileError> {
    let mut users = Vec::new();
    let mut groups = Vec::new();
    for m in members {
        match &m.name {
            MemberName::User(n) => users.push(n.clone()),
            MemberName::Everyone => groups.push(egret_perm::PUBLIC_GROUP.to_string()),
            // After expansion only `public` reaches here; a direct
            // `compile_grant`/`compile_rule` caller (the grammar differential)
            // may still name another group, which is the input being compared.
            MemberName::Unexpanded(p) if p.kind == egret_perm::PrincipalKind::Group => {
                groups.push(p.r#ref.clone())
            }
            MemberName::Unexpanded(p) => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind });
            }
        }
    }
    Ok((users, groups))
}

/// [`member_buckets`] as an access item, with `effect`/`accesses` set on it.
/// Shared with an engine privilege's item (`crate::ranger::engine_privileges`),
/// whose grantees the lowering checked exactly as a grant's.
pub(crate) fn item_from_members(
    members: &[Member],
    effect: Effect,
    accesses: &[String],
) -> Result<AccessItemInput, CompileError> {
    let (users, groups) = member_buckets(members)?;
    Ok(AccessItemInput {
        users,
        groups,
        // **Always empty, deliberately** (SP4c Task 2b). Ranger's `roles`
        // bucket is never populated on the Trino path egret provisions (D0), so
        // a name written here matches nobody: in an item it grants nobody, and
        // in an exception block it excepts nobody and the deny stands for
        // everyone. Roles reach the engine as their expanded members instead.
        roles: Vec::new(),
        accesses: accesses.to_vec(),
        effect: effect.as_str().to_string(),
    })
}

/// The access types `privileges` name on engine `e`, each once, in order:
/// `Read` and `Write` through the `Action` → access-type map
/// (`egret_perm::engine_accesses`), so a statement cannot drift from what the
/// model can express. `CreateMaterializedView` is refused: no grant or
/// withhold carries it (`caller` names the renderer in the refusal). Shared
/// with `crate::ranger::withhold`.
pub(crate) fn read_write_accesses(
    privileges: &[Privilege],
    e: EngineType,
    caller: &str,
) -> Result<Vec<String>, CompileError> {
    let mut accesses: Vec<String> = Vec::new();
    for p in privileges {
        let action = match p {
            Privilege::Read => Action::Read,
            Privilege::Write => Action::Write,
            Privilege::CreateMaterializedView => {
                return Err(CompileError::Db(format!(
                    "{caller} handed a CreateMaterializedView privilege"
                )))
            }
        };
        for acc in engine_accesses(action, e)? {
            if !accesses.iter().any(|a| a == acc) {
                accesses.push((*acc).to_string());
            }
        }
    }
    Ok(accesses)
}

/// The Ranger resource-level names the target engine's service-def declares
/// for a catalog/schema/table/column path, in that order. Trino calls the
/// second level `schema`; StarRocks calls it `database`
/// (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`) — same
/// position in the hierarchy, different declared name, so the compiler must
/// pick the name per engine rather than hard-coding Trino's.
/// `tests/compile_grants.rs`'s
/// `the_scope_becomes_the_ranger_resource_levels_the_service_def_declares`
/// re-reads both files' `resources` arrays and checks these four names
/// against each one's declared name + parent chain, so this can't drift
/// from the service-defs silently the way the write-up in round 1 could
/// have (its Trino-only test never opened a service-def).
// `pub(crate)`: `crate::ranger::rules` reuses this and `level`/
// `scope_to_resources` rather than duplicating the per-engine hierarchy
// naming a second time.
pub(crate) fn resource_level_names(e: EngineType) -> [&'static str; 4] {
    match e {
        EngineType::Trino => ["catalog", "schema", "table", "column"],
        EngineType::StarRocks => ["catalog", "database", "table", "column"],
    }
}

pub(crate) fn level(value: String) -> ResourceLevel {
    ResourceLevel {
        values: vec![value],
        is_excludes: false,
        is_recursive: false,
    }
}

/// The Ranger resource value that means "every child at this level". A
/// policy's resources map uses it to say "this table, whatever the column"
/// in a way the plugin's matcher actually evaluates — see
/// [`scope_to_resources`] for why an ABSENT level is not the same thing.
pub(crate) const WILDCARD: &str = "*";

/// The schema (Trino) / database (StarRocks) every catalog carries for its own
/// metadata. A catalog-scoped MASK excludes it ([`scope_to_resources`]),
/// because both engines request column masks for its relations exactly as for
/// a data table, and nothing in either plugin skips it:
///
/// * **Trino** (482, `trino/` at 280b81bbc4e): `StatementAnalyzer.visitTable`
///   (`core/trino-main/.../sql/analyzer/StatementAnalyzer.java:2414`) calls
///   `analyzeFiltersAndMasks` for every relation that is not a view or a
///   materialized view, which calls `accessControl.getColumnMasks` (`:2498`);
///   `AccessControlManager.getColumnMasks` (`security/AccessControlManager.java:1494-1510`)
///   forwards to the system access control with no schema check, and the
///   Ranger plugin's `getColumnMask` (`plugin/trino-ranger/.../RangerSystemAccessControl.java:804-806`)
///   builds a `catalog/schema/table/column` request for whatever it is
///   handed. `SHOW TABLES` / `SHOW COLUMNS` are rewritten into queries over
///   `information_schema` (`sql/rewrite/ShowQueriesRewrite.java:472-484`).
/// * **StarRocks** (4.1.7, `b81ff9cc`): an external catalog lists an
///   `information_schema` database of its own
///   (`connector/CatalogConnectorMetadata.java:109,123-124`); every relation
///   of a statement is marked for policy rewrite (`qe/ConnectProcessor.java:664`),
///   and `QueryAnalyzer` (`sql/analyzer/QueryAnalyzer.java:825`) calls
///   `SecurityPolicyRewriteRule.buildView` → `getColumnMaskingPolicy`
///   (`authorization/SecurityPolicyRewriteRule.java:57`,
///   `ranger/starrocks/RangerStarRocksAccessController.java:451`) with no
///   `information_schema` exception; the egret fork's patches add none.
///
/// `isExcludes` is legal on that level of both mask defs: `dataMaskDef`'s
/// entry carries no `excludesSupported`, and `mergeResourceDef` keeps the
/// access resource's `excludesSupported: true`.
pub(crate) const INFORMATION_SCHEMA: &str = "information_schema";

/// Which Ranger policy type a `resources` map is being rendered for. The
/// three do not share one resource hierarchy: an ACCESS policy is matched
/// against the full `catalog/schema/table/column` chain the service-def's
/// top-level `resources` array declares, a masking policy against
/// `dataMaskDef.resources` (also down to `column`), and a row-filter policy
/// against `rowFilterDef.resources`, which stops at `table`. They also do
/// not behave the same way when a level is missing — which is the whole
/// reason this enum exists rather than one map-builder for everybody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyTarget {
    Access,
    Mask,
    RowFilter,
}

/// Render a `Scope` into the engine's `resources` map. `catalog_name` is the
/// name the caller already resolved from `scope.catalog_id` (the compiler
/// stays free of registry I/O, and does not itself check that `catalog_name`
/// actually names `scope.catalog_id` — that binding is the caller's
/// responsibility, same as the rest of the registry lookup it already owns).
///
/// # An ACCESS policy fills every level below the scope with `*`
///
/// **This is measured, and it replaces an assumption the live gate refuted.**
/// Round 1 of this file left an unnamed level OUT of the map entirely, on the
/// strength of a reading of Apache Ranger's
/// `RangerDefaultPolicyResourceMatcher.getMatchType()` (`agents-common/…/
/// policyresourcematcher/RangerDefaultPolicyResourceMatcher.java`): once the
/// matcher runs out of policy-side resource entries, deeper levels present on
/// the *request* were said to resolve to `MatchType.ANCESTOR`
/// unconditionally, so a policy stopping at `table` would cover every column
/// beneath it. That paragraph even flagged itself as **not verified against a
/// live plugin**. Task 10 verified it, and **it is false for access policies**
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, case
/// T1): against a real Trino 4.7 + Ranger plugin on kind, a grant of `read` on
/// a SCHEMA compiled to `{catalog, schema}` and produced
///
/// ```text
/// Access Denied: Cannot select from columns [v, id] in table or view sibling
/// ```
///
/// A table-level grant (`{catalog, schema, table}`) failed identically. The
/// same grant written down to `column: "*"` returned all four rows. The
/// ancestor match holds for the *catalog* check — the denial moved from
/// `Cannot access catalog` to `Cannot select from columns` — and does **not**
/// hold downward to the column-granular `select` Trino actually asks about.
/// A grant above column level therefore compiled cleanly, deployed cleanly,
/// was listed as live by egret, and granted nothing.
///
/// So an `Access` target now emits the FULL hierarchy the target engine's
/// service-def declares — `catalog/schema/table/column` on Trino (service-def
/// id 203, levels 10/20/30/40, every one of them `mandatory: true`),
/// `catalog/database/table/column` on StarRocks — with `WILDCARD` at every
/// level the scope does not name, and at every level below the first one it
/// does not name (a scope cannot name a table without naming its schema, and
/// if one ever did, `*` at the schema level is the only honest rendering of
/// "unspecified"). `tests/compile_grants.rs`'s
/// `the_scope_becomes_the_ranger_resource_levels_the_service_def_declares`
/// re-reads both service-defs and asserts the emitted map carries exactly the
/// declared chain, so this cannot drift back.
///
/// # A MASK target fills unnamed levels with `*` too, down to the column
///
/// This section used to say the opposite: that a mask "renders only the levels
/// its scope names", on the strength of the live gate measuring T3b (a mask
/// that named every level down to its column) working. That measurement never
/// covered a mask scoped ABOVE the column — and that is the shape the product
/// makes the default: a table's Rules tab seeds a table-path mask with the
/// column left empty, a catalog or schema tab seeds one on that path, and the
/// dialog says "Leave Column empty to mask every column of the table". Such a
/// mask compiled to a body with no `column` resource (and, above the table, no
/// `table` or `schema` either), and **the real Ranger engine masks nothing for
/// it**: `dataMaskDef.resources` declares `catalog`, `schema`|`database`,
/// `table` and `column` on both engines, and Ranger's
/// `ServiceDefUtil.normalizeDataMaskDef` merges each entry onto the access
/// resource of the same name — which keeps the access def's
/// `"mandatory": true` on all four and clears `isValidLeaf`, so the only mask
/// hierarchy is the full four-level chain. A body missing any of those levels
/// matches no column request. (`mergeResourceDef` starts from a copy of the
/// access resource and overwrites `mandatory` only when the `dataMaskDef`
/// entry itself says `true`; the entries carry no `mandatory` at all.)
/// Measured on the real 2.8.0 and 2.9.0 jars, before and after, by
/// `egret-ranger-eval`'s
/// `a_path_mask_naming_no_column_masks_every_column_beneath_it_on_the_real_jars`.
///
/// So a mask now renders the same `wildcard_from_here` chain as an access
/// policy: a table-path mask is `column: *`, a schema-path mask is
/// `table: *, column: *`, a catalog-path mask `*` at every level below the
/// catalog. `*` is legal in a masking policy — `dataMaskDef` declares
/// `wildCard: true` at every level, and the clearance fold's whole-table
/// wildcard mask is measured on the same jars. `tests/compile_rules.rs`'
/// `the_mask_resource_map_matches_what_the_service_def_declares` re-reads both
/// service-defs and pins this.
///
/// One exception: a CATALOG-scoped mask's schema level is
/// `{values: ["information_schema"], isExcludes: true}` rather than `*`, so the
/// mask does not reach the catalog's own metadata relations — see
/// [`INFORMATION_SCHEMA`] for the engine source that requests masks there.
///
/// A level the scope NAMES is still copied verbatim (a `*` an author typed
/// above a named level stays a `*`, product decision 2026-10-10), so this
/// changes nothing for a mask that names its column.
pub(crate) fn scope_to_resources(
    scope: &Scope,
    catalog_name: &str,
    e: EngineType,
    target: PolicyTarget,
) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let named: [Option<&str>; 4] = [
        Some(catalog_name),
        scope.schema.as_deref(),
        scope.table.as_deref(),
        scope.column.as_deref(),
    ];

    let mut map = BTreeMap::new();
    match target {
        PolicyTarget::Access | PolicyTarget::Mask => {
            // Once a level is unnamed every level below it is unnamed too:
            // `*` from there down, never a named leaf hanging under a gap.
            // For a mask this is what makes "column left empty" mean "every
            // column": `dataMaskDef` makes all four levels mandatory, so an
            // omitted level matched no column at all (see the doc above).
            let mut wildcard_from_here = false;
            for (i, name) in names.iter().enumerate() {
                let value = match named[i] {
                    Some(v) if !wildcard_from_here => v.to_string(),
                    _ => {
                        wildcard_from_here = true;
                        WILDCARD.to_string()
                    }
                };
                map.insert((*name).to_string(), level(value));
            }
            // A CATALOG-scoped mask must not reach the catalog's
            // `information_schema`: both engines ask the plugin for column
            // masks on it like on any table (see [`INFORMATION_SCHEMA`]), so
            // `schema: *` would hash the names `SHOW TABLES`, `SHOW COLUMNS`,
            // `DESCRIBE` and every JDBC metadata call return. Its schema level
            // is "every schema except `information_schema`" instead.
            if target == PolicyTarget::Mask && named[1].is_none() {
                map.insert(
                    names[1].to_string(),
                    ResourceLevel {
                        values: vec![INFORMATION_SCHEMA.to_string()],
                        is_excludes: true,
                        is_recursive: false,
                    },
                );
            }
        }
        PolicyTarget::RowFilter => {
            // **The row-filter hierarchy is three levels, and every one of
            // them is mandatory** (whole-branch review, Critical 1).
            // `rowFilterDef.resources` declares exactly
            // `catalog`/`schema`|`database`/`table`, each `"mandatory": true`,
            // and declares no `column` at all — re-read from both service-def
            // files by `tests/compile_rules.rs`'s
            // `the_row_filter_resource_map_matches_what_the_service_def_declares`,
            // not taken from this comment. So emitting a `column` (as the
            // `Access | Mask` arm above does) adds a resource the def cannot
            // match, and omitting an unnamed `schema` or `table` drops a level
            // the def marks mandatory. Both compile,
            // deploy and are listed live, and both filter NOTHING — every row
            // comes back, on a control whose whole purpose is to withhold
            // rows.
            //
            // `RULE_SCOPE` at the authoring boundary
            // (`egret_perm_ir::lower::rules::validate_rule_shape`) refuses a path row-filter
            // scope that does not name exactly catalog+schema+table, so a
            // rule written through egret's own API never reaches either
            // shape. This arm is what makes a row already in `perm.rules`
            // (written before that guard existed) compile to something the
            // plugin can still match rather than to a silent no-op: the
            // column is dropped, and an unnamed level becomes `*` — the same
            // "unspecified means every child" rendering the `Access` arm's
            // live measurement validated, and the only one that can satisfy
            // a mandatory level at all.
            let mut wildcard_from_here = false;
            for (i, name) in names.iter().take(ROW_FILTER_LEVELS).enumerate() {
                let value = match named[i] {
                    Some(v) if !wildcard_from_here => v.to_string(),
                    _ => {
                        wildcard_from_here = true;
                        WILDCARD.to_string()
                    }
                };
                map.insert((*name).to_string(), level(value));
            }
        }
    }
    map
}

/// How many levels of [`resource_level_names`] a ROW-FILTER policy may name:
/// `catalog`, `schema`|`database`, `table` — and never `column`, which
/// `rowFilterDef.resources` does not declare on either engine (Trino
/// service-def id 203, StarRocks id 205). One constant, read by the emission
/// (`scope_to_resources`, `crate::ranger::rules::column_resources`) and by the
/// write-time refusal (`egret_perm_ir::lower::rules::validate_rule_shape`), so the two
/// cannot drift apart the way the mask and row-filter paths did.
pub(crate) const ROW_FILTER_LEVELS: usize = 3;

/// Render a grant statement for Ranger service `service` on engine `e`:
/// `compile_grant`'s former body from the access list onward.
///
/// Accesses expand the statement's privileges in order through
/// `egret_perm::engine_accesses`, dropping duplicates at their first
/// appearance — the same list the old compiler built action by action, since
/// the lowering keeps the grant's action order.
pub(crate) fn render_grant(
    s: &Statement,
    service: &str,
    e: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    let Source::Grant { grant_id } = &s.source else {
        return Err(CompileError::Db(format!(
            "render_grant handed a non-grant statement ({:?})",
            s.source
        )));
    };
    let (effect, privileges) = match &s.effect {
        statement::Effect::Allow { privileges } => (Effect::Allow, privileges),
        statement::Effect::Deny { privileges } => (Effect::Deny, privileges),
        other => {
            return Err(CompileError::Db(format!(
                "render_grant handed a non-grant effect ({other:?})"
            )))
        }
    };
    let Target::Path(path) = &s.target else {
        return Err(CompileError::Db(format!(
            "render_grant handed a non-path target ({:?})",
            s.target
        )));
    };

    let accesses = read_write_accesses(privileges, e, "render_grant")?;

    let item = item_from_members(&s.audience.include, effect, &accesses)?;

    let mut allow_exceptions = Vec::new();
    let mut deny_exceptions = Vec::new();
    if !s.audience.except.is_empty() {
        let exception = item_from_members(&s.audience.except, effect, &accesses)?;
        match effect {
            Effect::Allow => allow_exceptions.push(exception),
            Effect::Deny => deny_exceptions.push(exception),
        }
    }

    let named = |l: &Level| match l {
        Level::Named(v) => Some(v.clone()),
        Level::Unnamed => None,
    };
    let scope = Scope {
        catalog_id: path.catalog_id.unwrap_or_default(),
        schema: named(&path.schema),
        table: named(&path.table),
        column: named(&path.column),
    };

    Ok(AccessPolicyInput {
        name: format!("{COMPILER_GRANT_PREFIX}{grant_id}"),
        service: service.to_string(),
        is_enabled: true,
        resources: scope_to_resources(&scope, &path.catalog, e, PolicyTarget::Access),
        items: vec![item],
        allow_exceptions,
        deny_exceptions,
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    })
}

/// A grant statement as the recompile's desired set holds it: the rendered
/// body as Ranger JSON, stamped with the engine's `serviceType`.
pub(crate) fn render(
    s: &Statement,
    service: &str,
    e: EngineType,
) -> Result<Rendered, CompileError> {
    let input = render_grant(s, service, e)?;
    let grant_id = match &s.source {
        Source::Grant { grant_id } => *grant_id,
        _ => unreachable!("render_grant refuses a non-grant statement"),
    };
    Ok(Rendered {
        body: super::rules::stamp_service_type(access_to_ranger_json(&input), e),
        name: input.name,
        source_kind: SourceKind::Grant,
        grant_id: Some(grant_id),
        rule_id: None,
    })
}

/// Compile one `Grant` into a Ranger `AccessPolicyInput` for engine `e`.
///
/// `catalog_name` is the caller-resolved name for `g.scope.catalog_id` —
/// nothing here checks that the two actually correspond; that binding is the
/// caller's, made when it resolved the id in the first place. `service` is
/// the target Ranger service name (e.g. `"trino-1"`) stamped onto the policy
/// body.
///
/// The decisions are `egret_perm_ir::lower::grants::grant_statement`'s, and
/// so are its refusals: `CompileError::EmptyScopeLevel` /
/// `NamedLevelUnderWildcard` for the scope, `UnusableUserRef` /
/// `ReservedEnginePrincipal` for a `User` ref, and `UnexpandedPrincipalKind`
/// for a `Role` or `WorkspaceMembers` principal (include or exclude). This
/// function renders the statement it returns.
pub fn compile_grant(
    g: &Grant,
    catalog_name: &str,
    service: &str,
    e: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    render_grant(
        &grant_statement(g, catalog_name, g.scope.catalog_id)?,
        service,
        e,
    )
}
