//! The answer to "what does this person get?", one sentence per person and
//! engine, built once on the server (spec §5.1, C1).
//!
//! **Pure.** [`effect_lines`] reads a finished [`ExplainProfile`] plus the few
//! names the wire does not carry (a tied policy's model object, a classification's
//! display name) and writes words; it reads no database. So every wording rule
//! is a row of the table-driven test at the bottom, and every surface (Check
//! access, Who can read?, the member table) says it the same way.
//!
//! A line is structured parts plus their joined text. `text` is always the
//! concatenation of every part's `text`: a client renders the parts (bold the
//! outcome, link the rule) and never parses `text`.

use std::collections::HashMap;

use egret_perm::{Action, PrincipalKind, Scope};
use serde::Serialize;
use uuid::Uuid;

use crate::effect::NotEnforced;
use crate::explain_wire::{
    WireDecision, WireDenyReason, WireEngineType, WireMaskDecision, WireMaskResult,
    WireRowFilterDecision, WireTiedOutcome, WireUndecidableCause,
};
use crate::routes::{
    ExplainAccess, ExplainAttribution, ExplainClearanceGap, ExplainExemption, ExplainProfile,
    ExplainRowFilterGap, ExplainTagDecline, ExplainTargetLevel,
};

/// One sentence about one person on one engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ExplainEffectLine {
    pub about: EffectAbout,
    pub outcome: EffectOutcome,
    /// The parts' texts, joined: for aria labels, tests and copying.
    pub text: String,
    pub parts: Vec<EffectPart>,
}

/// What a line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectAbout {
    /// The value of the target column.
    Column,
    /// The rows of the target's table.
    Rows,
    /// Whether the target can be read or written at all.
    Access,
}

/// The outcome a line states, as a word a client can style without reading
/// the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutcome {
    RealValue,
    Masked,
    Nullified,
    Refused,
    Undecided,
    AllRows,
    Filtered,
    Allowed,
    NotEnforced,
}

/// One run of a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPart {
    Text {
        text: String,
    },
    /// The outcome word.
    Strong {
        text: String,
    },
    /// A column, a table, a filter expression, a custom mask expression.
    Code {
        text: String,
    },
    /// The username the line is about.
    Subject {
        text: String,
    },
    /// A rule, by name (its id when the row is gone); links to the rule.
    Rule {
        text: String,
        rule_id: Uuid,
    },
    /// A classification's clearance; links to its Clearance tab.
    Clearance {
        text: String,
        tag_id: Uuid,
    },
    /// A user or group that exempts the subject from a rule.
    Principal {
        text: String,
        principal_kind: PrincipalKind,
        r#ref: String,
    },
    /// A manual policy (one egret's compiler does not own), by Ranger name.
    Policy {
        text: String,
        policy_id: Option<i64>,
    },
    Catalog {
        text: String,
        catalog_id: Uuid,
    },
}

impl EffectPart {
    fn text(&self) -> &str {
        match self {
            EffectPart::Text { text }
            | EffectPart::Strong { text }
            | EffectPart::Code { text }
            | EffectPart::Subject { text }
            | EffectPart::Rule { text, .. }
            | EffectPart::Clearance { text, .. }
            | EffectPart::Principal { text, .. }
            | EffectPart::Policy { text, .. }
            | EffectPart::Catalog { text, .. } => text,
        }
    }
}

/// What [`effect_lines`] reads besides the profile itself.
pub(crate) struct EffectInput<'a> {
    pub subject: &'a str,
    pub action: Action,
    pub target: &'a Scope,
    pub profile: &'a ExplainProfile,
    pub tag_resolution_declined: Option<&'a ExplainTagDecline>,
    /// The model object of every policy a tie names, by Ranger policy name
    /// (`None`: a manual policy).
    pub tied: &'a HashMap<String, Option<ExplainAttribution>>,
    /// `tags.name` by tag id, for the clearances the profile names.
    pub tag_names: &'a HashMap<Uuid, String>,
    /// Every gated classification on the target column, by name: what a
    /// StarRocks clearance mask (one per column, the intersection of their
    /// cleared sets) clears a reader for. Empty for a table target.
    pub cleared_for: &'a [ExplainClearanceGap],
}

/// The lines for one profile: for a `read`, the column (or table) line and
/// then the rows line, which a refused read does not get; for a `write`, one
/// access line.
pub(crate) fn effect_lines(input: &EffectInput<'_>) -> Vec<ExplainEffectLine> {
    let b = Builder { input };
    match input.action {
        Action::Write => vec![b.write_line()],
        Action::Read => {
            let first = match &input.target.column {
                Some(column) => b.column_line(column),
                None => b.table_line(),
            };
            let refused = first.outcome == EffectOutcome::Refused;
            let mut lines = vec![first];
            if !refused {
                lines.extend(b.rows_line());
            }
            lines
        }
    }
}

/// C5: the line for `no_profile_serves_the_catalog`.
pub(crate) fn not_enforced_line(
    catalog_id: Uuid,
    catalog_name: &str,
    why: NotEnforced,
) -> ExplainEffectLine {
    let mut l = Line::default();
    l.text("Not enforced: ");
    l.push(EffectPart::Catalog {
        text: catalog_name.to_string(),
        catalog_id,
    });
    l.text(match why {
        NotEnforced::NoEngine => " has no engine.",
        _ => "'s engines have no policy profile.",
    });
    l.finish(EffectAbout::Access, EffectOutcome::NotEnforced)
}

#[derive(Default)]
struct Line {
    parts: Vec<EffectPart>,
}

impl Line {
    fn push(&mut self, part: EffectPart) {
        self.parts.push(part);
    }
    fn text(&mut self, s: impl Into<String>) {
        self.push(EffectPart::Text { text: s.into() });
    }
    fn strong(&mut self, s: impl Into<String>) {
        self.push(EffectPart::Strong { text: s.into() });
    }
    fn code(&mut self, s: impl Into<String>) {
        self.push(EffectPart::Code { text: s.into() });
    }
    fn finish(self, about: EffectAbout, outcome: EffectOutcome) -> ExplainEffectLine {
        ExplainEffectLine {
            about,
            outcome,
            text: self.parts.iter().map(EffectPart::text).collect(),
            parts: self.parts,
        }
    }
}

/// A tie's positional lists: policy ids, policy names, tied outcomes.
type Tie<'a> = (&'a [Option<i64>], &'a [String], &'a [WireTiedOutcome]);

struct Builder<'a> {
    input: &'a EffectInput<'a>,
}

/// The glossary's word for a mask type (E24), and the outcome it is.
fn mask_word(mask_type: Option<&str>) -> (&str, EffectOutcome) {
    match mask_type {
        None | Some("MASK_NONE") => ("the real value", EffectOutcome::RealValue),
        Some("MASK_NULL") => ("NULL", EffectOutcome::Nullified),
        Some("MASK") => ("redacted", EffectOutcome::Masked),
        Some("MASK_SHOW_LAST_4") => ("partly shown (last 4)", EffectOutcome::Masked),
        Some("MASK_SHOW_FIRST_4") => ("partly shown (first 4)", EffectOutcome::Masked),
        Some("MASK_HASH") => ("hashed", EffectOutcome::Masked),
        Some("MASK_DATE_SHOW_YEAR") => ("year only", EffectOutcome::Masked),
        Some("CUSTOM") => ("a custom expression", EffectOutcome::Masked),
        Some(other) => (other, EffectOutcome::Masked),
    }
}

/// The glossary's LABEL for a mask type, as the rule dialog names it.
fn mask_label(mask_type: Option<&str>) -> &str {
    match mask_type {
        Some("MASK") => "Redact",
        Some("MASK_SHOW_LAST_4") => "Partial (last 4)",
        Some("MASK_SHOW_FIRST_4") => "Partial (first 4)",
        Some("MASK_HASH") => "Hash",
        Some("MASK_NULL") => "Nullify",
        None | Some("MASK_NONE") => "Unmasked",
        Some("MASK_DATE_SHOW_YEAR") => "Year only",
        Some("CUSTOM") => "Custom",
        Some(other) => other,
    }
}

impl Builder<'_> {
    fn subject(&self, l: &mut Line) {
        l.push(EffectPart::Subject {
            text: self.input.subject.to_string(),
        });
    }

    fn table_label(&self) -> String {
        let t = self.input.target;
        [t.schema.as_deref(), t.table.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(".")
    }

    fn clearance_part(&self, tag_id: Uuid, key: Option<&str>) -> EffectPart {
        let text = self
            .input
            .tag_names
            .get(&tag_id)
            .cloned()
            .or_else(|| key.map(str::to_string))
            .unwrap_or_else(|| tag_id.to_string());
        EffectPart::Clearance { text, tag_id }
    }

    fn rule_part(rule_id: Uuid, name: Option<&str>) -> EffectPart {
        EffectPart::Rule {
            text: name.map_or_else(|| rule_id.to_string(), str::to_string),
            rule_id,
        }
    }

    /// "rule *X*", "clearance *PII*", "manual policy *P*", … — the decider of
    /// a verdict, from its attribution, else its Ranger name.
    fn decider(
        &self,
        l: &mut Line,
        attribution: Option<&ExplainAttribution>,
        policy_id: Option<i64>,
        policy_name: &str,
    ) {
        match attribution {
            Some(ExplainAttribution::Rule { rule_id, name, .. }) => {
                l.text("rule ");
                l.push(Self::rule_part(*rule_id, name.as_deref()));
            }
            Some(ExplainAttribution::Clearance { tag_id, tag, .. }) => {
                l.text("clearance ");
                l.push(self.clearance_part(*tag_id, tag.as_deref()));
            }
            Some(ExplainAttribution::Grant { .. }) => l.text("a grant"),
            Some(ExplainAttribution::EngineGrant { .. }) => l.text("an engine grant"),
            Some(ExplainAttribution::MaterializedViewDeny) => l.text("the materialized view deny"),
            None => {
                l.text("manual policy ");
                l.push(EffectPart::Policy {
                    text: policy_name.to_string(),
                    policy_id,
                });
            }
        }
    }

    /// " (a grant denies it)" and the like, for a denied access.
    fn deny_reason(
        &self,
        l: &mut Line,
        decision: &WireDecision,
        attribution: Option<&ExplainAttribution>,
    ) {
        let WireDecision::Denied {
            policy_id,
            policy_name,
            reason,
        } = decision
        else {
            return;
        };
        l.text(" (");
        match (attribution, reason) {
            (None, WireDenyReason::NoPolicyMatched) => l.text("no policy allows it"),
            (Some(ExplainAttribution::Grant { .. }), _) => l.text("a grant denies it"),
            (Some(ExplainAttribution::EngineGrant { .. }), _) => {
                l.text("an engine grant denies it")
            }
            _ => self.decider(l, attribution, *policy_id, policy_name),
        }
        l.text(")");
    }

    /// " (exempt from rule *X* via group data-team and 2 more)".
    fn exemption(&self, l: &mut Line, rule_id: Uuid, name: Option<&str>, e: &ExplainExemption) {
        l.text("exempt from rule ");
        l.push(Self::rule_part(rule_id, name));
        if e.via.is_empty() {
            l.text(
                "; the current rule does not name them, so its compiled policy is older than \
                 the rule",
            );
            return;
        }
        let direct = e
            .via
            .iter()
            .any(|v| v.principal_kind == PrincipalKind::User);
        let groups: Vec<_> = e
            .via
            .iter()
            .filter(|v| v.principal_kind != PrincipalKind::User)
            .collect();
        if direct {
            l.text(" directly");
        }
        if groups.is_empty() && e.more == 0 {
            return;
        }
        l.text(if direct { " and via " } else { " via " });
        for (i, v) in groups.iter().enumerate() {
            if i > 0 {
                let last = i + 1 == groups.len() && e.more == 0;
                l.text(if last { " and " } else { ", " });
            }
            l.text(format!("{} ", v.principal_kind.as_str()));
            l.push(EffectPart::Principal {
                text: v.r#ref.clone(),
                principal_kind: v.principal_kind,
                r#ref: v.r#ref.clone(),
            });
        }
        if e.more > 0 {
            l.text(format!(" and {} more", e.more));
        }
    }

    /// Words for an undecidable answer that is not a tie (spec §5.6).
    fn cause(&self, l: &mut Line, why: &[WireUndecidableCause]) {
        match why.first() {
            Some(WireUndecidableCause::TagsUnresolved) => {
                l.text("egret could not resolve this column's classifications");
                match self.input.tag_resolution_declined {
                    Some(ExplainTagDecline::StoredFoldDivergent {
                        schema,
                        table,
                        column,
                    }) => {
                        l.text(": the classified column ");
                        l.code(format!("{schema}.{table}.{column}"));
                        l.text(" has a name egret cannot compare safely");
                    }
                    Some(ExplainTagDecline::TargetFoldDivergent { level, value }) => {
                        let level = match level {
                            ExplainTargetLevel::Schema => "schema",
                            ExplainTargetLevel::Table => "table",
                            ExplainTargetLevel::Column => "column",
                        };
                        l.text(format!(": the {level} name "));
                        l.code(value.clone());
                        l.text(" cannot be compared safely");
                    }
                    None => {}
                }
            }
            Some(WireUndecidableCause::TagPriorityArbitrationNotModelled { .. }) => l.text(
                "classification rules at different priorities decide this, which egret does \
                 not model",
            ),
            Some(
                WireUndecidableCause::MaskExpression { .. }
                | WireUndecidableCause::RowFilterExpression { .. },
            ) => l.text("the policy uses an expression egret does not evaluate"),
            Some(other) => {
                let kind = serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_string))
                    .unwrap_or_default();
                l.text(format!("egret does not model this policy set ({kind})"));
            }
            None => l.text("egret cannot decide this"),
        }
    }

    fn undecided(&self, why: &[WireUndecidableCause], about: EffectAbout) -> ExplainEffectLine {
        let mut l = Line::default();
        self.subject(&mut l);
        l.text(": ");
        l.strong("undecided");
        l.text(" (");
        self.cause(&mut l, why);
        l.text(")");
        l.finish(about, EffectOutcome::Undecided)
    }

    /// The tied policies of a single-cause tie that reports its outcomes.
    fn tie(why: &[WireUndecidableCause]) -> Option<Tie<'_>> {
        match why {
            [WireUndecidableCause::EvalOrderAmbiguous {
                policy_ids,
                policy_names,
                outcomes,
            }] if !outcomes.is_empty() && outcomes.len() == policy_names.len() => {
                Some((policy_ids, policy_names, outcomes))
            }
            _ => None,
        }
    }

    fn tied_part(&self, policy_id: Option<i64>, name: &str) -> EffectPart {
        match self.input.tied.get(name).cloned().flatten() {
            Some(ExplainAttribution::Rule { rule_id, name, .. }) => {
                Self::rule_part(rule_id, name.as_deref())
            }
            Some(ExplainAttribution::Clearance { tag_id, tag, .. }) => {
                self.clearance_part(tag_id, tag.as_deref())
            }
            _ => EffectPart::Policy {
                text: name.to_string(),
                policy_id,
            },
        }
    }

    /// "rules" when every tied policy is a rule, else "policies".
    fn tied_noun(&self, names: &[String]) -> &'static str {
        let all_rules = names.iter().all(|n| {
            matches!(
                self.input.tied.get(n),
                Some(Some(ExplainAttribution::Rule { .. }))
            )
        });
        if all_rules {
            "rules"
        } else {
            "policies"
        }
    }

    /// " (2 rules agree: *A*, *B*)".
    fn agreeing(&self, l: &mut Line, ids: &[Option<i64>], names: &[String]) {
        l.text(format!(
            " ({} {} agree: ",
            names.len(),
            self.tied_noun(names)
        ));
        for (i, (id, name)) in ids.iter().zip(names).enumerate() {
            if i > 0 {
                l.text(", ");
            }
            l.push(self.tied_part(*id, name));
        }
        l.text(")");
    }

    /// "u: **undecided** — rules *A* (Hash) and *B* (Redact) apply at the same
    /// priority; …".
    fn conflicting(
        &self,
        ids: &[Option<i64>],
        names: &[String],
        outcomes: &[WireTiedOutcome],
        about: EffectAbout,
    ) -> ExplainEffectLine {
        let mut l = Line::default();
        self.subject(&mut l);
        l.text(": ");
        l.strong("undecided");
        l.text(format!(" — {} ", self.tied_noun(names)));
        let n = names.len();
        for (i, ((id, name), outcome)) in ids.iter().zip(names).zip(outcomes).enumerate() {
            if i > 0 {
                l.text(if i + 1 == n { " and " } else { ", " });
            }
            l.push(self.tied_part(*id, name));
            l.text(" (");
            match outcome {
                WireTiedOutcome::Mask { mask_type, .. } => l.text(mask_label(mask_type.as_deref())),
                WireTiedOutcome::RowFilter { filter_expr } => l.code(filter_expr.clone()),
            }
            l.text(")");
        }
        l.text(
            " apply at the same priority; the engine applies one of them, and which one cannot \
             be predicted here",
        );
        l.finish(about, EffectOutcome::Undecided)
    }

    /// "u reads c as **hashed**", up to the outcome word.
    fn reads_as(
        &self,
        l: &mut Line,
        column: &str,
        mask_type: Option<&str>,
        value: Option<&str>,
    ) -> EffectOutcome {
        self.subject(l);
        l.text(" reads ");
        l.code(column);
        l.text(" as ");
        let (word, outcome) = mask_word(mask_type);
        l.strong(word);
        if mask_type == Some("CUSTOM") {
            if let Some(v) = value {
                l.text(" ");
                l.code(v);
            }
        }
        outcome
    }

    fn masked_result(&self, l: &mut Line, column: &str, r: &WireMaskResult) -> EffectOutcome {
        self.reads_as(l, column, r.mask_type.as_deref(), r.masked_value.as_deref())
    }

    fn column_line(&self, column: &str) -> ExplainEffectLine {
        let p = self.input.profile;
        let select = p.accesses.iter().find(|a| a.access_type == "select");
        let mut line = match select.map(|a| &a.decision) {
            Some(WireDecision::Denied { .. }) => {
                let a = select.expect("matched above");
                let mut l = Line::default();
                self.subject(&mut l);
                l.text(" is ");
                l.strong("refused");
                l.text(" ");
                l.code(column);
                self.deny_reason(&mut l, &a.decision, a.attribution.as_ref());
                (l, EffectAbout::Column, EffectOutcome::Refused)
            }
            Some(WireDecision::Undecidable { why }) => {
                let line = self.undecided(why, EffectAbout::Column);
                return self.with_clearance_gaps(line);
            }
            // Not asked: StarRocks under a MASK_NULL. The mask decides only if the
            // query may read the table's other columns at all.
            None if p.masked_read_skips_access => match p.other_columns_access.as_ref() {
                Some(
                    a @ ExplainAccess {
                        decision: WireDecision::Denied { .. },
                        ..
                    },
                ) => {
                    let mut l = Line::default();
                    self.subject(&mut l);
                    l.text(" is ");
                    l.strong("refused");
                    l.text(" ");
                    l.code(column);
                    self.deny_reason(&mut l, &a.decision, a.attribution.as_ref());
                    l.text(
                        ": StarRocks masks it to NULL, and the query still needs SELECT on the \
                         other columns it reads",
                    );
                    (l, EffectAbout::Column, EffectOutcome::Refused)
                }
                Some(ExplainAccess {
                    decision: WireDecision::Undecidable { why },
                    ..
                }) => {
                    let line = self.undecided(why, EffectAbout::Column);
                    return self.with_clearance_gaps(line);
                }
                _ => match self.mask_line(column) {
                    Ok(parts) => parts,
                    Err(done) => return self.with_clearance_gaps(done),
                },
            },
            // Allowed: the mask decides.
            _ => match self.mask_line(column) {
                Ok(parts) => parts,
                Err(done) => return self.with_clearance_gaps(done),
            },
        };
        self.append_clearance_gaps(&mut line.0);
        line.0.finish(line.1, line.2)
    }

    /// The column line from the mask verdict; `Err` for a finished line that
    /// is not a "reads as" sentence (an undecidable answer).
    fn mask_line(
        &self,
        column: &str,
    ) -> Result<(Line, EffectAbout, EffectOutcome), ExplainEffectLine> {
        let mut l = Line::default();
        let Some(mask) = &self.input.profile.mask else {
            let outcome = self.reads_as(&mut l, column, None, None);
            return Ok((l, EffectAbout::Column, outcome));
        };
        let outcome = match &mask.decision {
            WireMaskDecision::NoMask => self.reads_as(&mut l, column, None, None),
            WireMaskDecision::Undecidable { why } => {
                if let Some((ids, names, outcomes)) = Self::tie(why) {
                    if outcomes.windows(2).all(|w| w[0] == w[1]) {
                        let outcome = match &outcomes[0] {
                            WireTiedOutcome::Mask {
                                mask_type,
                                masked_value,
                                ..
                            } => self.reads_as(
                                &mut l,
                                column,
                                mask_type.as_deref(),
                                masked_value.as_deref(),
                            ),
                            WireTiedOutcome::RowFilter { .. } => {
                                return Err(self.undecided(why, EffectAbout::Column))
                            }
                        };
                        self.agreeing(&mut l, ids, names);
                        return Ok((l, EffectAbout::Column, outcome));
                    }
                    return Err(self.conflicting(ids, names, outcomes, EffectAbout::Column));
                }
                return Err(self.undecided(why, EffectAbout::Column));
            }
            WireMaskDecision::Masked { result } => match &mask.attribution {
                Some(ExplainAttribution::Rule { rule_id, name, .. }) => match &mask.exemption {
                    Some(e) => {
                        let outcome = self.masked_result(&mut l, column, result);
                        l.text(" (");
                        self.exemption(&mut l, *rule_id, name.as_deref(), e);
                        l.text(")");
                        outcome
                    }
                    None => {
                        let outcome = self.masked_result(&mut l, column, result);
                        l.text(" (");
                        self.decider(
                            &mut l,
                            mask.attribution.as_ref(),
                            result.policy_id,
                            &result.policy_name,
                        );
                        l.text(")");
                        outcome
                    }
                },
                Some(ExplainAttribution::Clearance { tag_id, tag, .. }) => {
                    let clearance = self.clearance_part(*tag_id, tag.as_deref());
                    match &mask.replayed {
                        // Not cleared: the clearance's own NULL.
                        None => {
                            let outcome = self.masked_result(&mut l, column, result);
                            l.text(" (clearance ");
                            l.push(clearance);
                            l.text(")");
                            outcome
                        }
                        Some(replay) => {
                            // The word is always the SERVED mask's (what the engine
                            // applies); the replay only says who decided it, and only
                            // while the two agree. When they differ the compiled
                            // clearance mask predates the rule masks (review finding 5).
                            let as_outcome = |r: &WireMaskResult| {
                                (
                                    r.mask_type.clone().unwrap_or_else(|| "MASK_NONE".into()),
                                    r.masked_value.clone(),
                                )
                            };
                            let stale = match &replay.decision {
                                WireMaskDecision::NoMask => {
                                    as_outcome(result) != ("MASK_NONE".to_string(), None)
                                }
                                WireMaskDecision::Masked { result: r } => {
                                    as_outcome(r) != as_outcome(result)
                                }
                                WireMaskDecision::Undecidable { .. } => false,
                            };
                            let outcome = match (&replay.decision, &replay.attribution) {
                                (
                                    WireMaskDecision::Masked { .. },
                                    Some(ExplainAttribution::Rule { rule_id, name, .. }),
                                ) if !stale => {
                                    let outcome = self.masked_result(&mut l, column, result);
                                    match &replay.exemption {
                                        Some(e) => {
                                            l.text(" (");
                                            self.exemption(&mut l, *rule_id, name.as_deref(), e);
                                            l.text("; ");
                                        }
                                        None => {
                                            l.text(" by rule ");
                                            l.push(Self::rule_part(*rule_id, name.as_deref()));
                                            l.text(" (");
                                        }
                                    }
                                    outcome
                                }
                                (WireMaskDecision::Masked { result: r }, attribution) if !stale => {
                                    let outcome = self.masked_result(&mut l, column, result);
                                    l.text(" by ");
                                    self.decider(
                                        &mut l,
                                        attribution.as_ref(),
                                        r.policy_id,
                                        &r.policy_name,
                                    );
                                    l.text(" (");
                                    outcome
                                }
                                // No rule mask, or a replay egret cannot decide:
                                // the clearance's folded outcome stands.
                                _ => {
                                    let outcome = self.masked_result(&mut l, column, result);
                                    l.text(" (");
                                    outcome
                                }
                            };
                            l.text("cleared for ");
                            self.cleared_for(&mut l, clearance);
                            if stale {
                                l.text("; the served clearance mask is older than the rule masks");
                            }
                            l.text(")");
                            outcome
                        }
                    }
                }
                other => {
                    let outcome = self.masked_result(&mut l, column, result);
                    l.text(" (");
                    self.decider(
                        &mut l,
                        other.as_ref(),
                        result.policy_id,
                        &result.policy_name,
                    );
                    l.text(")");
                    outcome
                }
            },
        };
        Ok((l, EffectAbout::Column, outcome))
    }

    /// "*PII* and *Finance*": every gated classification on the column, or
    /// the deciding clearance's own when none is known.
    fn cleared_for(&self, l: &mut Line, deciding: EffectPart) {
        let all = self.input.cleared_for;
        if all.is_empty() {
            l.push(deciding);
            return;
        }
        for (i, c) in all.iter().enumerate() {
            if i > 0 {
                l.text(if i + 1 == all.len() { " and " } else { ", " });
            }
            l.push(EffectPart::Clearance {
                text: c.tag_name.clone(),
                tag_id: c.tag_id,
            });
        }
    }

    /// D2: ". Clearance *PII* is **not enforced** here: …", once per gap.
    fn append_clearance_gaps(&self, l: &mut Line) {
        for gap in &self.input.profile.clearance_not_enforced {
            l.text(". Clearance ");
            l.push(EffectPart::Clearance {
                text: gap.tag_name.clone(),
                tag_id: gap.tag_id,
            });
            l.text(" is ");
            l.strong("not enforced");
            l.text(
                " here: it is on the whole table, and StarRocks applies a clearance only where \
                 it is on a column.",
            );
        }
    }

    fn with_clearance_gaps(&self, line: ExplainEffectLine) -> ExplainEffectLine {
        let mut l = Line { parts: line.parts };
        self.append_clearance_gaps(&mut l);
        l.finish(line.about, line.outcome)
    }

    fn table_line(&self) -> ExplainEffectLine {
        let p = self.input.profile;
        let Some(select) = p.accesses.iter().find(|a| a.access_type == "select") else {
            return self.undecided(&[], EffectAbout::Access);
        };
        let mut l = Line::default();
        self.subject(&mut l);
        let outcome = match &select.decision {
            WireDecision::Allowed { .. } => {
                l.text(" ");
                l.strong("can read");
                l.text(" ");
                l.code(self.table_label());
                EffectOutcome::Allowed
            }
            WireDecision::Denied { .. } => {
                l.text(" ");
                l.strong("is refused");
                l.text(" ");
                l.code(self.table_label());
                self.deny_reason(&mut l, &select.decision, select.attribution.as_ref());
                EffectOutcome::Refused
            }
            WireDecision::Undecidable { why } => return self.undecided(why, EffectAbout::Access),
        };
        l.finish(EffectAbout::Access, outcome)
    }

    fn rows_line(&self) -> Option<ExplainEffectLine> {
        let rf = self.input.profile.row_filter.as_ref()?;
        let mut l = Line::default();
        let outcome = match &rf.decision {
            WireRowFilterDecision::NoFilter => {
                self.every_row(&mut l);
                self.append_row_filter_gaps(&mut l, &self.input.profile.row_filter_not_applied);
                EffectOutcome::AllRows
            }
            WireRowFilterDecision::Filtered { result } => match (&rf.attribution, &rf.exemption) {
                (Some(ExplainAttribution::Rule { rule_id, name, .. }), Some(e)) => {
                    self.every_row(&mut l);
                    l.text(" (");
                    self.exemption(&mut l, *rule_id, name.as_deref(), e);
                    l.text(")");
                    EffectOutcome::AllRows
                }
                (attribution, _) => {
                    let outcome = if result.filter_expr.is_empty() {
                        self.every_row(&mut l);
                        EffectOutcome::AllRows
                    } else {
                        self.rows_where(&mut l, &result.filter_expr);
                        EffectOutcome::Filtered
                    };
                    l.text(" (");
                    self.decider(
                        &mut l,
                        attribution.as_ref(),
                        result.policy_id,
                        &result.policy_name,
                    );
                    l.text(")");
                    outcome
                }
            },
            WireRowFilterDecision::Undecidable { why } => {
                let Some((ids, names, outcomes)) = Self::tie(why) else {
                    return Some(self.undecided(why, EffectAbout::Rows));
                };
                if !outcomes.windows(2).all(|w| w[0] == w[1]) {
                    return Some(self.conflicting(ids, names, outcomes, EffectAbout::Rows));
                }
                let outcome = match &outcomes[0] {
                    WireTiedOutcome::RowFilter { filter_expr } if !filter_expr.is_empty() => {
                        self.rows_where(&mut l, filter_expr);
                        EffectOutcome::Filtered
                    }
                    _ => {
                        self.every_row(&mut l);
                        EffectOutcome::AllRows
                    }
                };
                self.agreeing(&mut l, ids, names);
                outcome
            }
        };
        Some(l.finish(EffectAbout::Rows, outcome))
    }

    fn every_row(&self, l: &mut Line) {
        self.subject(l);
        l.text(" sees ");
        l.strong("every row");
    }

    fn rows_where(&self, l: &mut Line, filter_expr: &str) {
        self.subject(l);
        l.text(" sees ");
        l.strong("rows where");
        l.text(" ");
        l.code(filter_expr);
    }

    /// D3 and its StarRocks mirror: ": rule *X* does not filter on Trino here,
    /// because *PII* is on a column, not on the table" (StarRocks: "on the
    /// table, not on a column").
    fn append_row_filter_gaps(&self, l: &mut Line, gaps: &[ExplainRowFilterGap]) {
        let (engine, here, there) = match self.input.profile.engine {
            WireEngineType::Trino => ("Trino", "a column", "the table"),
            WireEngineType::StarRocks => ("StarRocks", "the table", "a column"),
        };
        match gaps {
            [] => {}
            [gap] => {
                l.text(": rule ");
                l.push(Self::rule_part(gap.rule_id, Some(&gap.rule_name)));
                l.text(format!(
                    " does not filter on {engine} here, because {} is on {here}, not on {there}",
                    gap.tag_name
                ));
            }
            several => {
                l.text(": rules ");
                for (i, gap) in several.iter().enumerate() {
                    if i > 0 {
                        l.text(if i + 1 == several.len() {
                            " and "
                        } else {
                            ", "
                        });
                    }
                    l.push(Self::rule_part(gap.rule_id, Some(&gap.rule_name)));
                }
                l.text(format!(
                    " do not filter on {engine} here, because their classifications are on \
                     {here}, not on {there}"
                ));
            }
        }
    }

    /// A `write` is a summary of its access verdicts (spec §12).
    fn write_line(&self) -> ExplainEffectLine {
        let p = self.input.profile;
        let target = match &self.input.target.column {
            Some(c) => format!("{}.{c}", self.table_label()),
            None => self.table_label(),
        };
        let mut l = Line::default();
        if let Some(a) = p
            .accesses
            .iter()
            .find(|a| matches!(a.decision, WireDecision::Denied { .. }))
        {
            self.subject(&mut l);
            l.text(" is ");
            l.strong("refused");
            l.text(format!(" {} on ", a.access_type));
            l.code(target);
            self.deny_reason(&mut l, &a.decision, a.attribution.as_ref());
            return l.finish(EffectAbout::Access, EffectOutcome::Refused);
        }
        if let Some(WireDecision::Undecidable { why }) = p
            .accesses
            .iter()
            .map(|a| &a.decision)
            .find(|d| matches!(d, WireDecision::Undecidable { .. }))
        {
            return self.undecided(why, EffectAbout::Access);
        }
        self.subject(&mut l);
        l.text(" ");
        l.strong("can write");
        l.text(" ");
        l.code(target);
        l.finish(EffectAbout::Access, EffectOutcome::Allowed)
    }
}

#[cfg(test)]
mod tests {
    //! One case per row of spec §5.1's two tables, plus the glossary words.
    //! Each case asserts the text and the part kinds; every case also asserts
    //! that `text` is the parts' texts joined.

    use super::*;
    use crate::explain_wire::{WireMaskResult, WireRowFilterResult};
    use crate::routes::{ExemptVia, ExplainAccess, ExplainMask, ExplainReplay, ExplainRowFilter};

    const RULE: Uuid = Uuid::from_u128(1);
    const RULE_B: Uuid = Uuid::from_u128(2);
    const TAG: Uuid = Uuid::from_u128(3);
    const CAT: Uuid = Uuid::from_u128(4);

    fn target(column: Option<&str>) -> Scope {
        Scope {
            catalog_id: CAT,
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: column.map(str::to_string),
        }
    }

    fn profile(engine: WireEngineType) -> ExplainProfile {
        ExplainProfile {
            service_id: 1,
            service_name: "p".into(),
            engine,
            accesses: vec![allowed()],
            tags: Vec::new(),
            tag_policies_in_download: 0,
            mask: None,
            row_filter: None,
            clearance_not_enforced: Vec::new(),
            row_filter_not_applied: Vec::new(),
            masked_read_skips_access: false,
            other_columns_access: None,
            effect_lines: Vec::new(),
        }
    }

    fn allowed() -> ExplainAccess {
        ExplainAccess {
            access_type: "select".into(),
            decision: WireDecision::Allowed {
                policy_id: Some(1),
                policy_name: "perm-grant-x".into(),
            },
            attribution: None,
        }
    }

    fn denied(
        reason: WireDenyReason,
        name: &str,
        attribution: Option<ExplainAttribution>,
    ) -> ExplainAccess {
        ExplainAccess {
            access_type: "select".into(),
            decision: WireDecision::Denied {
                policy_id: Some(9),
                policy_name: name.into(),
                reason,
            },
            attribution,
        }
    }

    fn rule(id: Uuid, name: &str) -> Option<ExplainAttribution> {
        Some(ExplainAttribution::Rule {
            rule_id: id,
            locator: None,
            name: Some(name.into()),
        })
    }

    fn clearance() -> Option<ExplainAttribution> {
        Some(ExplainAttribution::Clearance {
            tag_id: TAG,
            tag: Some("pii_key".into()),
            required_roles: Some(vec!["r".into()]),
            locator: Some("mask-c.sales.orders.email".into()),
        })
    }

    fn masked(mask_type: &str) -> WireMaskDecision {
        WireMaskDecision::Masked {
            result: WireMaskResult {
                policy_id: Some(5),
                policy_name: "pii-mask".into(),
                mask_type: Some(mask_type.into()),
                masked_value: (mask_type == "CUSTOM").then(|| "left({col}, 2)".into()),
                mask_condition: None,
            },
        }
    }

    fn mask(
        decision: WireMaskDecision,
        attribution: Option<ExplainAttribution>,
    ) -> Option<ExplainMask> {
        Some(ExplainMask {
            access_type: "select".into(),
            decision,
            attribution,
            exemption: None,
            replayed: None,
        })
    }

    fn via(kind: PrincipalKind, r: &str) -> ExemptVia {
        ExemptVia {
            principal_kind: kind,
            r#ref: r.into(),
        }
    }

    fn group_exemption() -> ExplainExemption {
        ExplainExemption {
            via: vec![via(PrincipalKind::Group, "data-team")],
            more: 0,
        }
    }

    fn filter(
        expr: &str,
        attribution: Option<ExplainAttribution>,
        exemption: Option<ExplainExemption>,
    ) -> Option<ExplainRowFilter> {
        Some(ExplainRowFilter {
            access_type: "select".into(),
            decision: WireRowFilterDecision::Filtered {
                result: WireRowFilterResult {
                    policy_id: Some(7),
                    policy_name: "rf".into(),
                    filter_expr: expr.into(),
                },
            },
            attribution,
            exemption,
        })
    }

    fn tie(outcomes: Vec<WireTiedOutcome>) -> Vec<WireUndecidableCause> {
        vec![WireUndecidableCause::EvalOrderAmbiguous {
            policy_ids: vec![Some(1), Some(2)],
            policy_names: vec!["perm-rule-a".into(), "perm-rule-b".into()],
            outcomes,
        }]
    }

    fn tied_mask(t: &str) -> WireTiedOutcome {
        WireTiedOutcome::Mask {
            mask_type: Some(t.into()),
            masked_value: None,
            mask_condition: None,
        }
    }

    struct Case {
        name: &'static str,
        action: Action,
        column: Option<&'static str>,
        profile: ExplainProfile,
        decline: Option<ExplainTagDecline>,
        want: Vec<&'static str>,
    }

    fn lines(c: &Case) -> Vec<ExplainEffectLine> {
        let tied = HashMap::from([
            ("perm-rule-a".to_string(), rule(RULE, "A")),
            ("perm-rule-b".to_string(), rule(RULE_B, "B")),
        ]);
        let tag_names = HashMap::from([(TAG, "PII".to_string())]);
        let target = target(c.column);
        effect_lines(&EffectInput {
            subject: "u",
            action: c.action,
            target: &target,
            profile: &c.profile,
            tag_resolution_declined: c.decline.as_ref(),
            tied: &tied,
            tag_names: &tag_names,
            cleared_for: &[],
        })
    }

    fn col(name: &'static str, p: ExplainProfile, want: &'static str) -> Case {
        Case {
            name,
            action: Action::Read,
            column: Some("email"),
            profile: p,
            decline: None,
            want: vec![want],
        }
    }

    fn with(mut p: ExplainProfile, f: impl FnOnce(&mut ExplainProfile)) -> ExplainProfile {
        f(&mut p);
        p
    }

    fn cases() -> Vec<Case> {
        let t = || profile(WireEngineType::Trino);
        let sr = || profile(WireEngineType::StarRocks);
        let mut out = vec![
            col("no mask", t(), "u reads email as the real value"),
            col("rule mask", with(t(), |p| p.mask = mask(masked("MASK_HASH"), rule(RULE, "Hash personal data"))),
                "u reads email as hashed (rule Hash personal data)"),
            col("exempt", with(t(), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(group_exemption());
            }), "u reads email as the real value (exempt from rule X via group data-team)"),
            col("exempt, several", with(t(), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(ExplainExemption {
                    via: vec![via(PrincipalKind::Group, "data-team"), via(PrincipalKind::Group, "ops")],
                    more: 2,
                });
            }), "u reads email as the real value (exempt from rule X via group data-team, group ops and 2 more)"),
            col("exempt, named directly", with(t(), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(ExplainExemption { via: vec![via(PrincipalKind::User, "u")], more: 0 });
            }), "u reads email as the real value (exempt from rule X directly)"),
            col("exempt, directly and via a group", with(t(), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(ExplainExemption {
                    via: vec![via(PrincipalKind::User, "u"), via(PrincipalKind::Group, "data-team")],
                    more: 0,
                });
            }), "u reads email as the real value (exempt from rule X directly and via group data-team)"),
            col("exempt, reason not in the model", with(t(), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(ExplainExemption { via: Vec::new(), more: 0 });
            }), "u reads email as the real value (exempt from rule X; the current rule does not name them, so its compiled policy is older than the rule)"),
            col("manual policy mask", with(t(), |p| p.mask = mask(masked("MASK"), None)),
                "u reads email as redacted (manual policy pii-mask)"),
            col("Trino clearance deny", with(t(), |p| p.accesses = vec![denied(WireDenyReason::DenyPolicyItem, "perm-clearance-x", clearance())]),
                "u is refused email (clearance PII)"),
            col("grant deny", with(t(), |p| p.accesses = vec![denied(WireDenyReason::DenyPolicyItem, "perm-grant-x", Some(ExplainAttribution::Grant { grant_id: RULE }))]),
                "u is refused email (a grant denies it)"),
            col("manual deny", with(t(), |p| p.accesses = vec![denied(WireDenyReason::DenyPolicyItem, "P", None)]),
                "u is refused email (manual policy P)"),
            col("nothing allows", with(t(), |p| p.accesses = vec![denied(WireDenyReason::NoPolicyMatched, "", None)]),
                "u is refused email (no policy allows it)"),
            col("StarRocks uncleared", with(sr(), |p| {
                p.accesses = Vec::new();
                p.masked_read_skips_access = true;
                p.mask = mask(masked("MASK_NULL"), clearance());
            }), "u reads email as NULL (clearance PII)"),
            col("StarRocks cleared, replay", with(sr(), |p| {
                p.mask = mask(masked("MASK_HASH"), clearance());
                p.mask.as_mut().unwrap().replayed = Some(ExplainReplay { decision: masked("MASK_HASH"), attribution: rule(RULE, "X"), exemption: None });
            }), "u reads email as hashed by rule X (cleared for PII)"),
            col("StarRocks cleared, exempt", with(sr(), |p| {
                p.mask = mask(masked("MASK_NONE"), clearance());
                p.mask.as_mut().unwrap().replayed = Some(ExplainReplay { decision: masked("MASK_NONE"), attribution: rule(RULE, "X"), exemption: Some(group_exemption()) });
            }), "u reads email as the real value (exempt from rule X via group data-team; cleared for PII)"),
            col("StarRocks cleared, no rule", with(sr(), |p| {
                p.mask = mask(masked("MASK_NONE"), clearance());
                p.mask.as_mut().unwrap().replayed = Some(ExplainReplay { decision: WireMaskDecision::NoMask, attribution: None, exemption: None });
            }), "u reads email as the real value (cleared for PII)"),
            col("StarRocks cleared, replay disagrees with the served mask", with(sr(), |p| {
                p.mask = mask(masked("MASK_NONE"), clearance());
                p.mask.as_mut().unwrap().replayed = Some(ExplainReplay { decision: masked("MASK_HASH"), attribution: rule(RULE, "X"), exemption: None });
            }), "u reads email as the real value (cleared for PII; the served clearance mask is older than the rule masks)"),
            col("agreeing tie", with(t(), |p| p.mask = mask(WireMaskDecision::Undecidable { why: tie(vec![tied_mask("MASK_HASH"), tied_mask("MASK_HASH")]) }, None)),
                "u reads email as hashed (2 rules agree: A, B)"),
            col("conflicting tie", with(t(), |p| p.mask = mask(WireMaskDecision::Undecidable { why: tie(vec![tied_mask("MASK_HASH"), tied_mask("MASK")]) }, None)),
                "u: undecided — rules A (Hash) and B (Redact) apply at the same priority; the engine applies one of them, and which one cannot be predicted here"),
            col("tag priority", with(t(), |p| p.mask = mask(WireMaskDecision::Undecidable { why: vec![WireUndecidableCause::TagPriorityArbitrationNotModelled { policy_ids: vec![], policy_names: vec![], priorities: vec![] }] }, None)),
                "u: undecided (classification rules at different priorities decide this, which egret does not model)"),
            col("other undecidable", with(t(), |p| p.mask = mask(WireMaskDecision::Undecidable { why: vec![WireUndecidableCause::SuperUsersUnknown] }, None)),
                "u: undecided (egret does not model this policy set (super_users_unknown))"),
            col("D2 gap", with(sr(), |p| p.clearance_not_enforced = vec![ExplainClearanceGap { tag_id: TAG, tag_name: "PII".into() }]),
                "u reads email as the real value. Clearance PII is not enforced here: it is on the whole table, and StarRocks applies a clearance only where it is on a column."),
        ];
        out.push(Case {
            name: "tags unresolved",
            decline: Some(ExplainTagDecline::TargetFoldDivergent { level: ExplainTargetLevel::Column, value: "emaıl".into() }),
            ..col("", with(t(), |p| p.mask = mask(WireMaskDecision::Undecidable { why: vec![WireUndecidableCause::TagsUnresolved] }, None)),
                "u: undecided (egret could not resolve this column's classifications: the column name emaıl cannot be compared safely)")
        });
        // Every glossary word, and a type the glossary does not list.
        for (t_, word) in [
            ("MASK", "redacted"),
            ("MASK_SHOW_LAST_4", "partly shown (last 4)"),
            ("MASK_SHOW_FIRST_4", "partly shown (first 4)"),
            ("MASK_HASH", "hashed"),
            ("MASK_NULL", "NULL"),
            ("MASK_NONE", "the real value"),
            ("MASK_DATE_SHOW_YEAR", "year only"),
            ("CUSTOM", "a custom expression left({col}, 2)"),
            ("MASK_ODD", "MASK_ODD"),
        ] {
            let want: &'static str = Box::leak(
                format!("u reads email as {word} (manual policy pii-mask)").into_boxed_str(),
            );
            out.push(col(
                "glossary word",
                with(t(), |p| p.mask = mask(masked(t_), None)),
                want,
            ));
        }
        // The rows line.
        let rows = |name: &'static str, p: ExplainProfile, want: &'static str| Case {
            name,
            action: Action::Read,
            column: None,
            profile: p,
            decline: None,
            want: vec!["u can read sales.orders", want],
        };
        out.extend([
            rows("no filter", with(t(), |p| p.row_filter = Some(ExplainRowFilter { access_type: "select".into(), decision: WireRowFilterDecision::NoFilter, attribution: None, exemption: None })),
                "u sees every row"),
            rows("filter", with(t(), |p| p.row_filter = filter("region = 'us'", rule(RULE, "X"), None)),
                "u sees rows where region = 'us' (rule X)"),
            rows("filter exempt", with(t(), |p| p.row_filter = filter("", rule(RULE, "X"), Some(group_exemption()))),
                "u sees every row (exempt from rule X via group data-team)"),
            rows("D3 gap", with(t(), |p| {
                p.row_filter = Some(ExplainRowFilter { access_type: "select".into(), decision: WireRowFilterDecision::NoFilter, attribution: None, exemption: None });
                p.row_filter_not_applied = vec![ExplainRowFilterGap { rule_id: RULE, rule_name: "X".into(), tag_name: "PII".into() }];
            }), "u sees every row: rule X does not filter on Trino here, because PII is on a column, not on the table"),
            rows("filter tie, agreeing", with(t(), |p| p.row_filter = Some(ExplainRowFilter { access_type: "select".into(), decision: WireRowFilterDecision::Undecidable { why: tie(vec![
                    WireTiedOutcome::RowFilter { filter_expr: "a = 1".into() }, WireTiedOutcome::RowFilter { filter_expr: "a = 1".into() }]) }, attribution: None, exemption: None })),
                "u sees rows where a = 1 (2 rules agree: A, B)"),
            rows("filter tie, conflicting", with(t(), |p| p.row_filter = Some(ExplainRowFilter { access_type: "select".into(), decision: WireRowFilterDecision::Undecidable { why: tie(vec![
                    WireTiedOutcome::RowFilter { filter_expr: "a = 1".into() }, WireTiedOutcome::RowFilter { filter_expr: "a = 2".into() }]) }, attribution: None, exemption: None })),
                "u: undecided — rules A (a = 1) and B (a = 2) apply at the same priority; the engine applies one of them, and which one cannot be predicted here"),
        ]);
        out.push(Case {
            name: "refused table read has no rows line",
            action: Action::Read,
            column: None,
            profile: with(t(), |p| {
                p.accesses = vec![denied(WireDenyReason::NoPolicyMatched, "", None)];
                p.row_filter = filter("a = 1", rule(RULE, "X"), None);
            }),
            decline: None,
            want: vec!["u is refused sales.orders (no policy allows it)"],
        });
        out.push(Case {
            name: "write",
            action: Action::Write,
            column: None,
            profile: with(t(), |p| {
                p.accesses = vec![
                    ExplainAccess {
                        access_type: "insert".into(),
                        ..allowed()
                    },
                    ExplainAccess {
                        access_type: "delete".into(),
                        ..denied(WireDenyReason::NoPolicyMatched, "", None)
                    },
                ]
            }),
            decline: None,
            want: vec!["u is refused delete on sales.orders (no policy allows it)"],
        });
        out
    }

    #[test]
    fn every_case_says_its_sentence_and_its_text_is_its_parts() {
        for c in cases() {
            let got = lines(&c);
            let texts: Vec<&str> = got.iter().map(|l| l.text.as_str()).collect();
            assert_eq!(texts, c.want, "case {:?}", c.name);
            for l in &got {
                let joined: String = l.parts.iter().map(EffectPart::text).collect();
                assert_eq!(l.text, joined, "case {:?}", c.name);
                assert!(
                    matches!(l.parts.first(), Some(EffectPart::Subject { .. })),
                    "a line starts with the person: case {:?}",
                    c.name
                );
            }
        }
    }

    #[test]
    fn the_parts_carry_what_a_client_links_and_styles() {
        let c = col(
            "exempt",
            with(profile(WireEngineType::Trino), |p| {
                p.mask = mask(masked("MASK_NONE"), rule(RULE, "X"));
                p.mask.as_mut().unwrap().exemption = Some(group_exemption());
            }),
            "",
        );
        let line = &lines(&c)[0];
        assert_eq!(line.about, EffectAbout::Column);
        assert_eq!(line.outcome, EffectOutcome::RealValue);
        assert!(line.parts.contains(&EffectPart::Strong {
            text: "the real value".into()
        }));
        assert!(line.parts.contains(&EffectPart::Code {
            text: "email".into()
        }));
        assert!(line.parts.contains(&EffectPart::Rule {
            text: "X".into(),
            rule_id: RULE
        }));
        assert!(line.parts.contains(&EffectPart::Principal {
            text: "data-team".into(),
            principal_kind: PrincipalKind::Group,
            r#ref: "data-team".into(),
        }));

        let c = col(
            "clearance",
            with(profile(WireEngineType::StarRocks), |p| {
                p.mask = mask(masked("MASK_NULL"), clearance());
            }),
            "",
        );
        let line = &lines(&c)[0];
        assert_eq!(line.outcome, EffectOutcome::Nullified);
        assert!(line.parts.contains(&EffectPart::Clearance {
            text: "PII".into(),
            tag_id: TAG
        }));
    }

    #[test]
    fn the_not_enforced_line_names_the_catalog_and_the_remedy() {
        let l = not_enforced_line(CAT, "demo_warehouse", NotEnforced::NoEngine);
        assert_eq!(l.text, "Not enforced: demo_warehouse has no engine.");
        assert_eq!(l.outcome, EffectOutcome::NotEnforced);
        assert!(l.parts.contains(&EffectPart::Catalog {
            text: "demo_warehouse".into(),
            catalog_id: CAT
        }));
        let l = not_enforced_line(CAT, "demo_warehouse", NotEnforced::NoProfile);
        assert_eq!(
            l.text,
            "Not enforced: demo_warehouse's engines have no policy profile."
        );
    }
}
