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
    pub r#type: &'static str,
    pub references: &'static [RefField],
}

/// Every task type egret knows, and which of its fields name egret objects.
///
/// This table is the declaration half of a seam: W3's handlers are the
/// implementation half and must stay in step with it. Publish refuses a type
/// absent from here, so the table is also what makes an unknown type
/// unpublishable rather than silently unresolvable (spec section 4.2, 8).
pub const TASK_TYPES: &[TaskTypeSpec] = &[
    TaskTypeSpec {
        r#type: "egret.sql",
        references: &[RefField {
            field: "database",
            kind: RefKind::Database,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.python",
        references: &[RefField {
            field: "notebook",
            kind: RefKind::Notebook,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.container",
        references: &[],
    },
    TaskTypeSpec {
        r#type: "egret.dataapi.call",
        references: &[RefField {
            field: "endpoint",
            kind: RefKind::DataApiEndpoint,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.dataset.refresh",
        references: &[RefField {
            field: "dataset",
            kind: RefKind::Dataset,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.report.send",
        references: &[RefField {
            field: "chart",
            kind: RefKind::Chart,
        }],
    },
    TaskTypeSpec {
        r#type: "core.http",
        references: &[],
    },
    TaskTypeSpec {
        r#type: "core.slack",
        references: &[],
    },
    TaskTypeSpec {
        r#type: "core.log",
        references: &[],
    },
];

pub fn known_type(t: &str) -> Option<&'static TaskTypeSpec> {
    TASK_TYPES.iter().find(|s| s.r#type == t)
}
