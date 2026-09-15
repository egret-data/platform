/// What kind of egret object a task field names.
///
/// Moved to `egret-workflow-api` in Task 6 (publish), so the port trait that
/// names it (`WorkflowRefPort::workspace_of`) can live in that leaf crate
/// without pulling this crate's engine dependencies along with it.
/// Re-exported here under its old path so nothing importing
/// `spec::registry::RefKind` had to change.
pub use egret_workflow_api::RefKind;

#[derive(Debug, Clone, Copy)]
pub struct RefField {
    pub field: &'static str,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskTypeSpec {
    /// **The canonical spelling, and for a ported Kestra task that is the
    /// FQCN** — programme spec section 8 rule 4: *"`type:` accepts Kestra's
    /// full FQCN as the canonical form (`io.kestra.plugin.core.log.Log`);
    /// short aliases are secondary. A Kestra flow pasted into egret runs, and
    /// looking a task up in Kestra's docs lands on the right page."*
    pub r#type: &'static str,
    /// Other spellings [`known_type`] accepts for the same task. Secondary by
    /// the rule above, and **the single source of truth for the alias**: the
    /// worker registry reads this list rather than repeating it, so a spelling
    /// publish accepts for a ported task is a spelling the worker dispatches by
    /// construction (`worker::Handlers::add_publishable`).
    pub aliases: &'static [&'static str],
    pub references: &'static [RefField],
}

/// Every task type egret knows, and which of its fields name egret objects.
///
/// This table is the declaration half of a seam: W3's handlers are the
/// implementation half and must stay in step with it. Publish refuses a type
/// absent from here, so the table is also what makes an unknown type
/// unpublishable rather than silently unresolvable (spec section 4.2, 8).
///
/// # "Must stay in step with it" is now held by construction, not by memory
///
/// The sentence above stood for four packages while the two halves were
/// **disjoint** (see the comment on the four core rows below). It is no longer
/// a request: [`worker::Handlers::add_publishable`](crate::worker::Handlers::add_publishable)
/// registers a handler under this table's canonical spelling **and** its
/// aliases, and **panics if the handler's `type_id()` has no row here at all**
/// — so a handler that is not publishable cannot reach
/// [`with_core_tasks`](crate::worker::Handlers::with_core_tasks), and
/// `egret-bin` will not start. An alias added here is dispatched without
/// anyone remembering to add it twice.
///
/// **The converse does not hold and is not meant to**: a row here with no
/// handler is a type egret publishes and does not run (`core.http`,
/// `core.slack`, and every `egret.*` task the deployment does not wire in).
/// That asymmetry is the shipping default `main.rs` documents, and
/// `WorkerError::NoHandler` is what a flow using one gets.
pub const TASK_TYPES: &[TaskTypeSpec] = &[
    TaskTypeSpec {
        r#type: "egret.sql",
        aliases: &[],
        references: &[RefField {
            field: "database",
            kind: RefKind::Database,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.python",
        aliases: &[],
        references: &[RefField {
            field: "notebook",
            kind: RefKind::Notebook,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.container",
        aliases: &[],
        references: &[],
    },
    // W4 Task 5, fix round 1. `egret.python` one row above is the
    // NOTEBOOK-kernel task (programme spec §6.4: "`KernelLauncher`; cần port
    // `egret-kernel-api` mới"), and its `notebook` reference is required. The
    // container-backed inline-`script:` handler that task built is a different
    // task, so it took a different name rather than bending this one; see
    // `worker::handlers::python`'s module doc. No `RefField`: a script and an
    // image name are not egret objects, exactly as for `egret.container`.
    TaskTypeSpec {
        r#type: "egret.script.python",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "egret.dataapi.call",
        aliases: &[],
        references: &[RefField {
            field: "endpoint",
            kind: RefKind::DataApiEndpoint,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.dataset.refresh",
        aliases: &[],
        references: &[RefField {
            field: "dataset",
            kind: RefKind::Dataset,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.report.send",
        aliases: &[],
        references: &[RefField {
            field: "chart",
            kind: RefKind::Chart,
        }],
    },
    TaskTypeSpec {
        r#type: "core.http",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "core.slack",
        aliases: &[],
        references: &[],
    },
    // ── The four tasks `worker::Handlers::with_core_tasks()` implements ────
    //
    // **FQCN canonical, short form as an alias, per programme spec section 8
    // rule 4.** Until W10 Task 6 fix round 1 this table held `core.log` and
    // nothing else of the four, while the handlers registered Kestra's FQCNs:
    // the intersection was EMPTY, so a flow using `core.log` published and then
    // found no handler, a flow using the FQCN ran and could not be published,
    // and **no Kestra flow pasted into egret could be published at all** —
    // measured as `each of the four handler TYPE_IDs appears in this table
    // exactly zero times`. That is a violation of a decided rule, not an open
    // question, and this table was the side that was wrong.
    //
    // `core.log` is kept as an alias because it is a spelling this table has
    // published since W1. **The other three get no short alias**, because they
    // have never had one and inventing three spellings is a decision nobody
    // has made: `core.log` maps to the PACKAGE segment (`…core.log.Log`), which
    // does not generalise to `…core.debug.Return` without picking between
    // `core.debug` and `core.return`.
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.log.Log",
        aliases: &["core.log"],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.debug.Return",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.execution.Fail",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.flow.Sleep",
        aliases: &[],
        references: &[],
    },
];

/// The row for `t`, matching the canonical spelling **or any alias**.
///
/// The alias arm is why `core.log` and `io.kestra.plugin.core.log.Log` are one
/// task to publish rather than two: see [`TaskTypeSpec::aliases`].
pub fn known_type(t: &str) -> Option<&'static TaskTypeSpec> {
    TASK_TYPES
        .iter()
        .find(|s| s.r#type == t || s.aliases.contains(&t))
}
