//! `decide`, `filter` and `decide_privilege` over a [`Snapshot`] (plan D2).
//!
//! Every refusal fails closed: an operation the 483 table does not model, a
//! resource of the wrong shape, or nothing that allows it, is a deny.

use std::collections::BTreeSet;
use std::fmt;

use egret_perm_ir::audience::{Audience, MemberName};
use egret_perm_ir::lineage::{kept_accesses, kept_only_on, KeptOn};
use egret_perm_ir::lower::grants::level_is_a_wildcard_run;
use egret_perm_ir::statement::{
    Effect, InfraObject, Level, Path, Privilege, Source, Statement, Target,
};

use crate::pattern;
use crate::snapshot::Snapshot;
use crate::trino_ops::{row, DataPrivilege, OpClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    None,
    Catalog(String),
    Schema {
        catalog: String,
        schema: String,
    },
    Table {
        catalog: String,
        schema: String,
        table: String,
        columns: Vec<String>,
    },
    Column {
        catalog: String,
        schema: String,
        table: String,
        column: String,
        column_type: String,
    },
    User(String),
    SystemProperty(String),
    CatalogSessionProperty {
        catalog: String,
        property: String,
    },
    Function {
        catalog: String,
        schema: String,
        function: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// `by` names the deciding statement's source, e.g. "grant 6f1c…", "infra ExecuteQuery".
    Allow { by: String },
    /// `explicit`: a Deny/Withhold refused it (vs nothing allowed it).
    Deny { reason: String, explicit: bool },
}

/// A data object for `explain`: absent levels match only statements covering the whole level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectPath {
    pub catalog: String,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub column: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("two masks apply to column {column} for user {user}")]
    TwoMasks { user: String, column: String },
}

/// The engine capabilities whose first resource key is `catalog`
/// (`egret-perm-compile/src/engine_capabilities.rs`). `trino.impersonate_user`
/// is not one: its first value is a username.
const CATALOG_KEYED: &[&str] = &[
    "trino.set_catalog_session_property",
    "trino.execute_procedure",
    "trino.execute_function",
];

/// Of [`CATALOG_KEYED`], the ones whose second resource key is `schema`.
const SCHEMA_KEYED: &[&str] = &["trino.execute_procedure", "trino.execute_function"];

/// The engine-capability operations the all-access statement does not allow:
/// the two that ask `execute` on a `schemafunction`. egret's all-access
/// policies take their access types from each leaf's `accessTypeRestrictions`
/// (`egret_policy::default_policies::admin_all_access_bodies`), and the Trino
/// service-def restricts `schemafunction` to `create`, `drop` and `show`, so
/// the Ranger artifact gives the all-access holder no `execute` there; only a
/// `trino.execute_function` engine grant does. `FilterFunctions` asks `_any`,
/// which `show` answers, so the all-access still allows it. Measured against
/// the real Ranger engine by `egret-perm-compile/tests/opa_differential.rs`.
const NOT_BY_WHOLE_EP: &[&str] = &["ExecuteFunction", "CreateViewWithExecuteFunction"];

/// A data object. A level is present only when every level above it is
/// (`Obj::new` refuses anything else).
#[derive(Debug, Clone, Copy)]
struct Obj<'a> {
    catalog: &'a str,
    schema: Option<&'a str>,
    table: Option<&'a str>,
    column: Option<&'a str>,
}

impl<'a> Obj<'a> {
    fn new(
        catalog: &'a str,
        schema: Option<&'a str>,
        table: Option<&'a str>,
        column: Option<&'a str>,
    ) -> Option<Obj<'a>> {
        let gap = (schema.is_none() && table.is_some()) || (table.is_none() && column.is_some());
        (!gap).then_some(Obj {
            catalog,
            schema,
            table,
            column,
        })
    }
}

impl fmt::Display for Obj<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.catalog)?;
        for l in [self.schema, self.table, self.column].into_iter().flatten() {
            write!(f, ".{l}")?;
        }
        Ok(())
    }
}

fn deny(reason: String) -> Decision {
    Decision::Deny {
        reason,
        explicit: false,
    }
}

fn refused(reason: String) -> Decision {
    Decision::Deny {
        reason,
        explicit: true,
    }
}

fn malformed(op: &str) -> Decision {
    deny(format!("malformed resource for {op}"))
}

fn privilege_of_data(p: DataPrivilege) -> Privilege {
    match p {
        DataPrivilege::Read => Privilege::Read,
        DataPrivilege::Write => Privilege::Write,
    }
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The statement's source, as `Decision`s name it.
fn label(s: &Statement) -> String {
    match &s.source {
        Source::Grant { grant_id } => format!("grant {grant_id}"),
        Source::Rule { rule_name, .. } => format!("rule {rule_name}"),
        Source::Withhold {
            classification_name,
            ..
        } => format!("withhold {classification_name}"),
        Source::EnginePrivilege { engine_grant_id } => {
            format!("engine privilege {engine_grant_id}")
        }
        Source::Infra { kind } => format!("infra {kind:?}"),
    }
}

/// Whether the lineage principal is refused `access` on `leaf` (for a
/// narrowed access, on `resource`): the access is not one
/// `egret_perm_ir::lineage` keeps on the leaf, or is kept only on another
/// value. An `_any` check is refused only on a leaf that keeps nothing; on
/// the data chain it follows the grants (`check_reach` counts only Read
/// allows for the principal).
fn lineage_refuses(leaf: &str, access: &str, user: &str, resource: Option<&Resource>) -> bool {
    let kept = kept_accesses(leaf);
    if access == "_any" {
        return kept.is_empty();
    }
    if !kept.contains(&access) {
        return true;
    }
    match kept_only_on(leaf) {
        Some((a, KeptOn::Itself)) if a == access => {
            !matches!(resource, Some(Resource::User(target)) if target == user)
        }
        Some((a, KeptOn::Value(v))) if a == access => {
            !matches!(resource, Some(Resource::SystemProperty(p)) if eq_ci(p, v))
        }
        _ => false,
    }
}

/// D2: the statement applies to `user` when `include` names them or everyone,
/// and `except` names neither. Usernames compare exactly.
pub(crate) fn applies(a: &Audience, user: &str) -> bool {
    let names = |m: &MemberName| match m {
        MemberName::User(u) => u == user,
        MemberName::Everyone => true,
        MemberName::Unexpanded(_) => false,
    };
    a.include.iter().any(|m| names(&m.name)) && !a.except.iter().any(|m| names(&m.name))
}

/// A named level against a present request value. A value that is literally
/// `*` or empty asks for every value, so only a match-any pattern (a run of
/// `*`) answers it, as Ranger's `requests_all_values` does.
fn named_matches(p: &str, v: &str) -> bool {
    if v.is_empty() || v == "*" {
        level_is_a_wildcard_run(p)
    } else {
        pattern::matches(p, v)
    }
}

/// One level against a request level. A present value matches `Unnamed` or
/// the pattern; an absent one matches only a statement covering the whole
/// level: `Unnamed` or a run of `*` (Ranger's `SelfAndAllDescendants`; the
/// compiler's own predicate, so `**` is the same as `*`).
pub(crate) fn level_covers(level: &Level, value: Option<&str>) -> bool {
    match (level, value) {
        (Level::Unnamed, _) => true,
        (Level::Named(p), Some(v)) => named_matches(p, v),
        (Level::Named(p), None) => level_is_a_wildcard_run(p),
    }
}

/// The statement's path is the object or one of its ancestors: the data
/// match, and the deny side of reach.
fn path_covers(p: &Path, o: &Obj) -> bool {
    named_matches(&p.catalog, o.catalog)
        && level_covers(&p.schema, o.schema)
        && level_covers(&p.table, o.table)
        && level_covers(&p.column, o.column)
}

/// The statement's path is the object or one of its descendants: the allow
/// side of reach. The object's named levels must match; deeper levels are free.
fn path_reaches(p: &Path, o: &Obj) -> bool {
    let free = |l: &Level, v: Option<&str>| v.is_none_or(|v| level_covers(l, Some(v)));
    named_matches(&p.catalog, o.catalog)
        && free(&p.schema, o.schema)
        && free(&p.table, o.table)
        && free(&p.column, o.column)
}

/// The levels a metadata-table restriction is looked for on: catalog, schema
/// and table; any column.
pub(crate) fn path_on_table(p: &Path, catalog: &str, schema: &str, table: &str) -> bool {
    named_matches(&p.catalog, catalog)
        && level_covers(&p.schema, Some(schema))
        && level_covers(&p.table, Some(table))
}

/// A deny or withhold, and the privileges it carries.
fn denied(e: &Effect) -> Option<&[Privilege]> {
    match e {
        Effect::Deny { privileges } | Effect::Withhold { privileges } => Some(privileges),
        _ => None,
    }
}

/// D2/M17: what makes a user restricted on a base table.
fn restricts(e: &Effect) -> bool {
    match e {
        Effect::Deny { privileges } | Effect::Withhold { privileges } => {
            privileges.contains(&Privilege::Read)
        }
        Effect::Mask { mask_type, .. } => mask_type != "MASK_NONE",
        Effect::WithholdMask => true,
        Effect::RowFilter { expression } => !expression.trim().is_empty(),
        Effect::Allow { .. } | Effect::EnginePrivilege { .. } => false,
    }
}

impl Snapshot {
    pub fn decide(&self, user: &str, operation: &str, resource: &Resource) -> Decision {
        let Some(op) = row(operation) else {
            return deny(format!("operation not modelled: {operation}"));
        };
        if let Some(d) = self.lineage_gate(user, operation, op.leaf, &[op.access], Some(resource)) {
            return d;
        }
        match op.class {
            OpClass::Data(p) => self.decide_data(user, operation, p, resource),
            OpClass::Reach => self.decide_reach(user, operation, resource),
            OpClass::AdminOnly => match self.whole_ep(user) {
                Some(by) => Decision::Allow { by },
                None => deny(format!(
                    "{operation} needs the all-access engine administration grant"
                )),
            },
            OpClass::EngineCapability(id) => {
                let values: Vec<&str> = match resource {
                    Resource::Function {
                        catalog,
                        schema,
                        function,
                    } => vec![catalog, schema, function],
                    Resource::CatalogSessionProperty { catalog, property } => {
                        vec![catalog, property]
                    }
                    _ => return malformed(operation),
                };
                self.engine_object(user, id, &values)
                    .or_else(|| {
                        (!NOT_BY_WHOLE_EP.contains(&operation))
                            .then(|| self.whole_ep(user))
                            .flatten()
                    })
                    .map_or_else(
                        || deny(format!("no {id} grant on {}", values.join("."))),
                        |by| Decision::Allow { by },
                    )
            }
            OpClass::Impersonation => {
                let Resource::User(target) = resource else {
                    return malformed(operation);
                };
                self.infra_where(user, |o| match o {
                    InfraObject::AnyUser => true,
                    InfraObject::SessionUser => eq_ci(target, user),
                    _ => false,
                })
                .or_else(|| self.engine_object(user, "trino.impersonate_user", &[target]))
                .or_else(|| self.whole_ep(user))
                .map_or_else(
                    || deny(format!("{user} may not act on the queries of {target}")),
                    |by| Decision::Allow { by },
                )
            }
            OpClass::ExecuteQuery => self
                .infra_where(user, |o| matches!(o, InfraObject::AnyQuery))
                .or_else(|| self.whole_ep(user))
                .map_or_else(
                    || deny(format!("{user} may not execute queries")),
                    |by| Decision::Allow { by },
                ),
            OpClass::SystemSessionProperty => {
                let Resource::SystemProperty(p) = resource else {
                    return malformed(operation);
                };
                self.infra_where(
                    user,
                    |o| matches!(o, InfraObject::SystemProperty(q) if eq_ci(q, p)),
                )
                .or_else(|| self.engine_object(user, "trino.set_system_session_property", &[p]))
                .or_else(|| self.whole_ep(user))
                .map_or_else(
                    || deny(format!("no grant to set the system session property {p}")),
                    |by| Decision::Allow { by },
                )
            }
            OpClass::ValueQuestion => deny(format!("{operation} is asked on a value route")),
        }
    }

    /// The indices `i` for which `decide(user, operation, &resources[i])` allows, ascending.
    pub fn filter(&self, user: &str, operation: &str, resources: &[Resource]) -> Vec<usize> {
        resources
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(self.decide(user, operation, r), Decision::Allow { .. }))
            .map(|(i, _)| i)
            .collect()
    }

    /// The Data rule on one object. An absent level matches only statements
    /// whose level is `Unnamed` or exactly `*`.
    pub fn decide_privilege(
        &self,
        user: &str,
        privilege: DataPrivilege,
        object: &ObjectPath,
    ) -> Decision {
        match Obj::new(
            &object.catalog,
            object.schema.as_deref(),
            object.table.as_deref(),
            object.column.as_deref(),
        ) {
            Some(o) => self.check_data(user, privilege_of_data(privilege), &o),
            None => deny("malformed object path: a level is named below an absent one".into()),
        }
    }

    /// The lineage gate of `decide`: refused when any of `accesses` on `leaf`
    /// is one the reserved principal is never allowed.
    fn lineage_gate(
        &self,
        user: &str,
        what: &str,
        leaf: &str,
        accesses: &[&str],
        resource: Option<&Resource>,
    ) -> Option<Decision> {
        if self.lineage_principal != user {
            return None;
        }
        let a = accesses
            .iter()
            .find(|a| lineage_refuses(leaf, a, user, resource))?;
        Some(refused(format!(
            "{what} asks {a} on {leaf}, which the lineage principal {user} is never allowed"
        )))
    }

    fn decide_data(&self, user: &str, op: &str, p: DataPrivilege, resource: &Resource) -> Decision {
        let Resource::Table {
            catalog,
            schema,
            table,
            columns,
        } = resource
        else {
            return malformed(op);
        };
        let table_obj = Obj {
            catalog,
            schema: Some(schema),
            table: Some(table),
            column: None,
        };
        // Write checks the table; a Read of no columns (`count(*)`) is one
        // table-level check, never a vacuous allow.
        if p == DataPrivilege::Write || columns.is_empty() {
            return self.check_data(user, privilege_of_data(p), &table_obj);
        }
        self.every_column(columns, |c| {
            self.check_data(
                user,
                Privilege::Read,
                &Obj {
                    column: Some(c),
                    ..table_obj
                },
            )
        })
    }

    fn decide_reach(&self, user: &str, op: &str, resource: &Resource) -> Decision {
        match resource {
            Resource::Catalog(catalog) => self.check_reach(
                user,
                &Obj {
                    catalog,
                    schema: None,
                    table: None,
                    column: None,
                },
            ),
            Resource::Schema { catalog, schema } => self.check_reach(
                user,
                &Obj {
                    catalog,
                    schema: Some(schema),
                    table: None,
                    column: None,
                },
            ),
            Resource::Table {
                catalog,
                schema,
                table,
                columns,
            } => {
                let table_obj = Obj {
                    catalog,
                    schema: Some(schema),
                    table: Some(table),
                    column: None,
                };
                if columns.is_empty() {
                    self.check_reach(user, &table_obj)
                } else {
                    self.every_column(columns, |c| {
                        self.check_reach(
                            user,
                            &Obj {
                                column: Some(c),
                                ..table_obj
                            },
                        )
                    })
                }
            }
            _ => malformed(op),
        }
    }

    /// Every column must be allowed. An explicit refusal of any column wins
    /// over a column nothing allows, so the reason names the deny.
    fn every_column(&self, columns: &[String], check: impl Fn(&str) -> Decision) -> Decision {
        let mut first_allow = None;
        let mut first_refusal = None;
        for c in columns {
            match check(c) {
                d @ Decision::Deny { explicit: true, .. } => return d,
                d @ Decision::Deny { .. } => {
                    first_refusal.get_or_insert(d);
                }
                d @ Decision::Allow { .. } => {
                    first_allow.get_or_insert(d);
                }
            }
        }
        first_refusal
            .or(first_allow)
            .unwrap_or_else(|| deny("no columns".into()))
    }

    /// D2's Data rule: deny wins; then the metadata-table rule; then some
    /// allow on the object, or the all-access statement.
    fn check_data(&self, user: &str, p: Privilege, o: &Obj) -> Decision {
        let classes = self.classes_of(o);
        let candidates = self.candidates(o.catalog, &classes);
        let mut allow = None;
        for &i in &candidates {
            let s = &self.statements[i];
            if !applies(&s.audience, user) {
                continue;
            }
            if let Some(privs) = denied(&s.effect) {
                if privs.contains(&p) && self.target_hits(s, o, &classes) {
                    return refused(format!("{} refuses {p:?} on {o}", label(s)));
                }
            }
            if let (Effect::Allow { privileges }, Target::Path(path)) = (&s.effect, &s.target) {
                if allow.is_none() && privileges.contains(&p) && path_covers(path, o) {
                    allow = Some(label(s));
                }
            }
        }
        if p == Privilege::Read {
            if let Some(d) = self.metadata_refusal(user, o) {
                return d;
            }
        }
        match allow.or_else(|| self.whole_ep(user)) {
            Some(by) => Decision::Allow { by },
            None => deny(format!("no statement allows {p:?} on {o} to {user}")),
        }
    }

    /// D2's conservative `_any`: some allow on the object or below it, and no
    /// deny or withhold carrying any privilege on the object or above it.
    fn check_reach(&self, user: &str, o: &Obj) -> Decision {
        // The lineage principal keeps only `select` and `use` on the data
        // chain (`egret_perm_ir::lineage::kept_accesses`), so only a Read
        // allow reaches for it, and no engine capability does.
        let lineage = self.lineage_principal == user;
        let classes = self.classes_of(o);
        let candidates = self.candidates(o.catalog, &classes);
        let mut allow = None;
        for &i in &candidates {
            let s = &self.statements[i];
            if !applies(&s.audience, user) {
                continue;
            }
            if let Some(privs) = denied(&s.effect) {
                if !privs.is_empty() && self.target_hits(s, o, &classes) {
                    return refused(format!("{} hides {o}", label(s)));
                }
            }
            if let (Effect::Allow { privileges }, Target::Path(path)) = (&s.effect, &s.target) {
                let reaches = if lineage {
                    privileges.contains(&Privilege::Read)
                } else {
                    !privileges.is_empty()
                };
                if allow.is_none() && reaches && path_reaches(path, o) {
                    allow = Some(label(s));
                }
            }
        }
        if let Some(d) = self.metadata_refusal(user, o) {
            return d;
        }
        match allow
            .or_else(|| self.whole_ep(user))
            .or_else(|| (!lineage).then(|| self.engine_reach(user, o)).flatten())
        {
            Some(by) => Decision::Allow { by },
            None => deny(format!("nothing on or below {o} is granted to {user}")),
        }
    }

    /// The statement's target is the object or an ancestor, by path or by a
    /// classification the object carries.
    fn target_hits(&self, s: &Statement, o: &Obj, classes: &BTreeSet<String>) -> bool {
        match &s.target {
            Target::Path(p) => path_covers(p, o),
            Target::Classification { name } => classes.contains(&name.to_lowercase()),
            Target::EngineObject { .. } | Target::Infra(_) => false,
        }
    }

    /// M17: a `$` table is refused to a user restricted on any column of its
    /// base table (the name before the first `$`), by path or classification.
    fn metadata_refusal(&self, user: &str, o: &Obj) -> Option<Decision> {
        let (Some(schema), Some(table)) = (o.schema, o.table) else {
            return None;
        };
        let (base, _) = table.split_once('$')?;
        let mut classes = BTreeSet::new();
        if let Some(tc) = self.index.classes.get(&(
            o.catalog.to_lowercase(),
            schema.to_lowercase(),
            base.to_lowercase(),
        )) {
            classes.extend(tc.table.iter().cloned());
            for c in tc.columns.values() {
                classes.extend(c.iter().cloned());
            }
        }
        for i in self.candidates(o.catalog, &classes) {
            let s = &self.statements[i];
            if !applies(&s.audience, user) || !restricts(&s.effect) {
                continue;
            }
            let hit = match &s.target {
                Target::Path(p) => path_on_table(p, o.catalog, schema, base),
                Target::Classification { name } => classes.contains(&name.to_lowercase()),
                Target::EngineObject { .. } | Target::Infra(_) => false,
            };
            if hit {
                return Some(refused(format!(
                    "{table} is a metadata table of {base}, on which {} restricts {user}",
                    label(s)
                )));
            }
        }
        None
    }

    /// D2: a column carries its own classifications and its table's; a table
    /// only its own; a schema or catalog none.
    fn classes_of(&self, o: &Obj) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let (Some(schema), Some(table)) = (o.schema, o.table) else {
            return out;
        };
        let key = (
            o.catalog.to_lowercase(),
            schema.to_lowercase(),
            table.to_lowercase(),
        );
        if let Some(tc) = self.index.classes.get(&key) {
            out.extend(tc.table.iter().cloned());
            if let Some(c) = o.column {
                if let Some(cs) = tc.columns.get(&c.to_lowercase()) {
                    out.extend(cs.iter().cloned());
                }
            }
        }
        out
    }

    /// Every statement that can match an object in `catalog` carrying
    /// `classes`, ascending (source order decides `by`).
    pub(crate) fn candidates(&self, catalog: &str, classes: &BTreeSet<String>) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .index
            .by_catalog
            .get(&catalog.to_lowercase())
            .into_iter()
            .flatten()
            .chain(&self.index.wild_catalog)
            .chain(
                classes
                    .iter()
                    .filter_map(|c| self.index.by_classification.get(c))
                    .flatten(),
            )
            .copied()
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The first applying infrastructure statement whose object passes `f`.
    fn infra_where(&self, user: &str, f: impl Fn(&InfraObject) -> bool) -> Option<String> {
        self.index.infra.iter().find_map(|&i| {
            let s = &self.statements[i];
            match &s.target {
                Target::Infra(o) if applies(&s.audience, user) && f(o) => Some(label(s)),
                _ => None,
            }
        })
    }

    /// The all-access statement (`InfraObject::WholeEp`), if it applies.
    fn whole_ep(&self, user: &str) -> Option<String> {
        self.infra_where(user, |o| matches!(o, InfraObject::WholeEp))
    }

    /// An applying engine object of `capability` whose values equal `values`,
    /// ignoring case.
    fn engine_object(&self, user: &str, capability: &str, values: &[&str]) -> Option<String> {
        self.index.engine.iter().find_map(|&i| {
            let s = &self.statements[i];
            match &s.target {
                Target::EngineObject {
                    capability: c,
                    values: v,
                } if c == capability
                    && applies(&s.audience, user)
                    && v.len() == values.len()
                    && v.iter().zip(values).all(|(a, b)| eq_ci(a, b)) =>
                {
                    Some(label(s))
                }
                _ => None,
            }
        })
    }

    /// An applying catalog-keyed engine object below the object: its catalog
    /// is the object's and, for a schema, its schema is too. Nothing of these
    /// capabilities sits below a table.
    fn engine_reach(&self, user: &str, o: &Obj) -> Option<String> {
        if o.table.is_some() {
            return None;
        }
        self.index.engine.iter().find_map(|&i| {
            let s = &self.statements[i];
            let Target::EngineObject { capability, values } = &s.target else {
                return None;
            };
            let catalog_ok = CATALOG_KEYED.contains(&capability.as_str())
                && values.first().is_some_and(|c| eq_ci(c, o.catalog));
            let schema_ok = o.schema.is_none_or(|sch| {
                SCHEMA_KEYED.contains(&capability.as_str())
                    && values.get(1).is_some_and(|v| eq_ci(v, sch))
            });
            (catalog_ok && schema_ok && applies(&s.audience, user)).then(|| label(s))
        })
    }
}
