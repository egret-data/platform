//! The evaluator's input, frozen and indexed: one service's statements and
//! the classified columns of its catalogs, read at one `(policy_version,
//! tag_version)`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use egret_perm_ir::audience::{Audience, MemberName};
use egret_perm_ir::statement::{Level, Path, Source, Statement, Target};

use crate::pattern;

/// One classification on a column, or on a whole table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedColumn {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    /// None: the classification is on the whole table.
    pub column: Option<String>,
    /// `tags.name`, the name `Target::Classification` carries.
    pub classification: String,
}

pub struct SnapshotInput {
    pub service_id: i64,
    /// (policy_version, tag_version) the inputs were read at.
    pub key: (i64, i64),
    pub statements: Vec<Statement>,
    pub classified: Vec<ClassifiedColumn>,
    /// The reserved principal view-lineage EXPLAIN runs as
    /// (`egret_core::engine_principals::LINEAGE_PRINCIPAL`, filled by
    /// `egret-perm-compile`). It is refused every access
    /// `egret_perm_ir::lineage` does not keep, whatever the grants say,
    /// exactly as the Ranger download's lineage transform denies it.
    /// Required: an empty name fails the build (`NoLineagePrincipal`), so a
    /// builder that forgets it serves nothing rather than a snapshot that
    /// lets the principal do what the grants allow (fail closed).
    pub lineage_principal: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("statement {index} names an unexpanded principal {principal}")]
    UnexpandedMember { index: usize, principal: String },
    #[error("statement {index} has a path Ranger would read wider than written: {reason}")]
    MalformedPath { index: usize, reason: &'static str },
    #[error("the snapshot names no reserved lineage principal")]
    NoLineagePrincipal,
}

/// The classifications of one table: on the table itself, and per column.
/// Every name and key is lowercased.
#[derive(Debug, Clone, Default)]
pub(crate) struct TableClasses {
    pub(crate) table: BTreeSet<String>,
    pub(crate) columns: BTreeMap<String, BTreeSet<String>>,
}

/// `(catalog, schema, table)`, lowercased.
pub(crate) type TableKey = (String, String, String);

/// One level of the path index (Task 22F).
///
/// - `named`: the statements whose value at this level is a literal (no `*`
///   or `?`), keyed by that value lowercased. A literal matches a request
///   value only when the two are equal ignoring case
///   (`pattern::matches`), and never a request value that is `*` or empty
///   (`named_matches` asks a wildcard run there, which a literal is not), so
///   a lookup by the request value lowercased finds every literal that can
///   match it.
/// - `wild`: every other statement: an unnamed level, or a pattern holding
///   `*` or `?` (wildcard runs such as `**` included). Every lookup at this
///   level visits it.
#[derive(Debug, Clone, Default)]
pub(crate) struct LevelIndex<T> {
    pub(crate) named: HashMap<String, T>,
    pub(crate) wild: T,
}

impl<T: Default> LevelIndex<T> {
    /// The bucket a statement whose value at this level is `key` belongs in.
    fn slot(&mut self, key: Option<String>) -> &mut T {
        match key {
            Some(k) => self.named.entry(k).or_default(),
            None => &mut self.wild,
        }
    }
}

impl<T> LevelIndex<T> {
    /// Every bucket that may hold a statement matching a request whose value
    /// at this level is `value`. An absent value visits every bucket: the
    /// reach rule leaves the levels below the object free.
    pub(crate) fn visit<'a>(&'a self, value: Option<&str>, f: &mut impl FnMut(&'a T)) {
        match value {
            Some(v) => {
                if let Some(t) = self.named.get(&v.to_lowercase()) {
                    f(t);
                }
            }
            None => self.named.values().for_each(&mut *f),
        }
        f(&self.wild);
    }
}

/// The literal key of a level value: lowercased, or None when the value
/// holds a wildcard and so belongs with the patterns.
fn literal_key(value: &str) -> Option<String> {
    (!pattern::is_wild(value)).then(|| value.to_lowercase())
}

fn level_key(level: &Level) -> Option<String> {
    match level {
        Level::Named(v) => literal_key(v),
        Level::Unnamed => None,
    }
}

/// `Target::Path` statements by catalog, then schema, then table.
pub(crate) type PathIndex = LevelIndex<LevelIndex<LevelIndex<Vec<usize>>>>;

#[derive(Debug, Clone, Default)]
pub(crate) struct Index {
    /// `Target::Path` statements by (catalog, schema, table) level.
    pub(crate) paths: PathIndex,
    /// `Target::Classification` statements by classification name, lowercased.
    pub(crate) by_classification: BTreeMap<String, Vec<usize>>,
    /// `Target::Infra` statements.
    pub(crate) infra: Vec<usize>,
    /// `Target::EngineObject` statements.
    pub(crate) engine: Vec<usize>,
    pub(crate) classes: BTreeMap<TableKey, TableClasses>,
}

pub struct Snapshot {
    service_id: i64,
    key: (i64, i64),
    pub(crate) statements: Vec<Statement>,
    classified: Vec<ClassifiedColumn>,
    pub(crate) lineage_principal: String,
    pub(crate) index: Index,
}

impl Snapshot {
    pub fn build(input: SnapshotInput) -> Result<Snapshot, SnapshotError> {
        if input.lineage_principal.trim().is_empty() {
            return Err(SnapshotError::NoLineagePrincipal);
        }
        let mut index = Index::default();
        for (i, s) in input.statements.iter().enumerate() {
            check_expanded(i, &s.audience)?;
            if let Target::Path(p) = &s.target {
                check_path(i, p)?;
            }
            match &s.target {
                Target::Path(p) => index
                    .paths
                    .slot(literal_key(&p.catalog))
                    .slot(level_key(&p.schema))
                    .slot(level_key(&p.table))
                    .push(i),
                Target::Classification { name } => index
                    .by_classification
                    .entry(name.to_lowercase())
                    .or_default()
                    .push(i),
                Target::Infra(_) => index.infra.push(i),
                Target::EngineObject { .. } => index.engine.push(i),
            }
        }
        for c in &input.classified {
            let key = (
                c.catalog.to_lowercase(),
                c.schema.to_lowercase(),
                c.table.to_lowercase(),
            );
            let entry = index.classes.entry(key).or_default();
            let name = c.classification.to_lowercase();
            match &c.column {
                None => {
                    entry.table.insert(name);
                }
                Some(col) => {
                    entry
                        .columns
                        .entry(col.to_lowercase())
                        .or_default()
                        .insert(name);
                }
            }
        }
        Ok(Snapshot {
            service_id: input.service_id,
            key: input.key,
            statements: input.statements,
            classified: input.classified,
            lineage_principal: input.lineage_principal,
            index,
        })
    }

    /// A copy whose infrastructure statements (`Source::Infra`) are replaced by `infra`,
    /// every other statement kept. The refresher uses it when only the data lowering failed (C4).
    pub fn with_infra(&self, infra: Vec<Statement>) -> Result<Snapshot, SnapshotError> {
        let statements = self
            .statements
            .iter()
            .filter(|s| !matches!(s.source, Source::Infra { .. }))
            .cloned()
            .chain(infra)
            .collect();
        Snapshot::build(SnapshotInput {
            service_id: self.service_id,
            key: self.key,
            statements,
            classified: self.classified.clone(),
            lineage_principal: self.lineage_principal.clone(),
        })
    }

    /// The reserved lineage principal (see [`SnapshotInput`]).
    pub fn lineage_principal(&self) -> &str {
        &self.lineage_principal
    }

    pub fn service_id(&self) -> i64 {
        self.service_id
    }

    pub fn key(&self) -> (i64, i64) {
        self.key
    }

    pub fn len(&self) -> usize {
        self.statements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }
}

/// D2: audiences arrive expanded; a member the lowering could not expand
/// would make the statement's reach unknowable, so the build refuses it.
fn check_expanded(index: usize, a: &Audience) -> Result<(), SnapshotError> {
    for m in a.include.iter().chain(&a.except) {
        if let MemberName::Unexpanded(p) = &m.name {
            return Err(SnapshotError::UnexpandedMember {
                index,
                principal: format!("{}:{}", p.kind.as_str(), p.r#ref),
            });
        }
    }
    Ok(())
}

/// Refuse the path shapes Ranger renders wider than the evaluator would read
/// them, so the evaluator fails closed on its own instead of trusting the
/// upstream checks (`grants_check`, `ensure_scope_levels_are_named`):
/// - an empty value: Ranger drops it, which leaves match-any;
/// - a named level below an unnamed one: `scope_to_resources` latches `*`
///   below the first unnamed level.
fn check_path(index: usize, p: &Path) -> Result<(), SnapshotError> {
    let bad = |reason| Err(SnapshotError::MalformedPath { index, reason });
    if p.catalog.is_empty() {
        return bad("an empty catalog");
    }
    let mut unnamed_above = false;
    for level in [&p.schema, &p.table, &p.column] {
        match level {
            Level::Named(v) if v.is_empty() => return bad("an empty named level"),
            Level::Named(_) if unnamed_above => return bad("a named level below an unnamed one"),
            Level::Named(_) => {}
            Level::Unnamed => unnamed_above = true,
        }
    }
    Ok(())
}
