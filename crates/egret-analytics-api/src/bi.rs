//! Pure-serde BI wire DTOs shared across the Analytics seam.
//!
//! Moved verbatim (same fields + serde attrs) from `egret-bi::model` so the wire
//! shapes and persisted jsonb are unchanged; `egret-bi` re-exports these to keep
//! its handlers compiling. Only the pure-data TYPES live here — the domain
//! conversions (`chart_dto`, `detail_dto`, …), the `From<Row>` impls, and any DTO
//! that references an `egret-bi`-local type (e.g. `ChartDataBody`'s `QuerySpec`)
//! stay behind in `egret-bi`.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SaveQuery {
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SavedQueryDto {
    pub id: String,
    pub database_id: Option<String>,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub created_by: Option<String>,
    /// Folder this saved query lives in (`None` = root). Serialized as a string.
    pub folder_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A single column in a DESCRIBE result.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchemaColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A schema-object search hit (a table/view matched by the SQL Lab global
/// object search). `catalog` is empty for engines without a catalog level.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchHit {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

/// A reusable SQL snippet (SQL Lab).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SnippetDto {
    pub id: String,
    pub name: String,
    pub body: String,
    pub description: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Create/update body for a SQL snippet.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct SnippetBody {
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FolderDto {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateFolder {
    pub name: String,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RenameFolder {
    pub name: String,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetFolder {
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateSource {
    Physical {
        catalog: Option<String>,
        schema: String,
        table: String,
    },
    Virtual {
        sql: String,
    },
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDataset {
    pub name: String,
    pub database_id: Uuid,
    #[serde(default)]
    pub description: String,
    pub source: CreateSource,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateDataset {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sql: Option<String>,
    // NO certification fields. They lived here until 0109, and that is how an editor could vouch
    // for their own dataset — and, because `certified_by` came from this body, name a colleague
    // as the voucher. Certification now has its own route and its own platform permission.
}

/// Body of the dataset / metric certification endpoints. Deliberately carries NO `certified_by`:
/// the certifier is the session, exactly as it is for dashboards. A client that could name the
/// certifier could forge provenance, which is the defect 0109 closes.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetDatasetCertification {
    pub certified: bool,
    /// Ignored when `certified` is false — withdrawing clears the note rather than leaving a
    /// stale justification attached to an uncertified dataset.
    #[serde(default)]
    pub details: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatasetColumnDto {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
    pub expression: Option<String>,
    pub verbose_name: String,
    pub description: String,
    pub is_calculated: bool,
    /// Usable as a groupby dimension in charts.
    pub groupby: bool,
    /// Usable as a filter in charts.
    pub filterable: bool,
    /// Display format (ColumnFormat JSON); empty = default.
    pub format: String,
    /// Semantic role; '' = none. See [`SEMANTIC_TYPES`] for the full vocabulary.
    ///
    /// TYPED `String` AND NOT AN ENUM, ON PURPOSE — this is the READ side, and it must stay total.
    /// A value stored before migration 0104's constraint (or by any future writer that bypasses
    /// `validate_semantic_type`) has to be serialisable, because this DTO is built one-per-column
    /// by `load_detail` and a decode failure on a single column would take the whole dataset detail
    /// response down with it. The `…` this doc comment used to end in is also gone: it predated
    /// `latitude`/`longitude` (#438) and silently under-described the field, which is the same
    /// class of drift the enum-free vocabulary caused in the first place.
    #[schema(schema_with = semantic_type_schema)]
    pub semantic_type: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatasetSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    #[schema(value_type = String)]
    pub kind: &'static str,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub column_count: i64,
    pub folder_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
    /// Certified as a trusted/official dataset (for a list badge).
    pub certified: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatasetDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    #[schema(value_type = String)]
    pub kind: &'static str,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub sql: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub columns: Vec<DatasetColumnDto>,
    pub metrics: Vec<MetricDto>,
    /// Certified as a trusted/official dataset.
    pub certified: bool,
    pub certified_by: String,
    pub certification_details: String,
    /// When the dataset was moved to the trash; null while it is live. Its charts keep running
    /// until it is deleted, and say so.
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The answer to a Refresh: the dataset as it is now, and which physical columns the source
/// added or dropped since the last sync.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatasetRefreshDto {
    #[serde(flatten)]
    pub dataset: DatasetDetailDto,
    pub added_columns: Vec<String>,
    pub removed_columns: Vec<String>,
}

/// Body of `POST /datasets/{id}/duplicate`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DuplicateDataset {
    /// Name of the copy; unique in the workspace.
    pub name: String,
}

/// The typed confirmation a delete of something charts use requires: the object's own name.
#[derive(Debug, Default, Deserialize)]
pub struct ConfirmQuery {
    /// The dataset's (or metric's) exact name. Required only while charts use it; with it, the
    /// delete goes ahead and takes those charts (or their metric) with it.
    pub confirm: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetricDto {
    pub id: String,
    pub metric_name: String,
    pub expression: String,
    pub data_type: String,
    pub type_generic: String,
    pub verbose_name: String,
    pub description: String,
    pub ordinal: i32,
    /// Display format (ColumnFormat JSON); empty = default.
    pub format: String,
    /// Marked as a trusted/certified metric.
    pub certified: bool,
    pub certified_by: String,
    pub certification_details: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MetricBody {
    pub metric_name: String,
    pub expression: String,
    #[serde(default)]
    pub verbose_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: String,
    // NO certification fields, for the reason on `UpdateDataset`. A metric is what the semantic
    // layer actually shows a reader, so a forgeable metric badge would defeat the dataset gate.
}

/// A chart that uses a dataset (for the dataset Usage view).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DatasetUsageDto {
    pub id: String,
    pub name: String,
    /// The chart's viz type, for its icon.
    pub viz_type: String,
    /// The chart is in the trash (listed only with `include_trashed`).
    pub trashed: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddColumnBody {
    pub column_name: String,
    pub expression: String,
    #[serde(default)]
    pub verbose_name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateColumnBody {
    #[serde(default)]
    pub verbose_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub groupby: Option<bool>,
    #[serde(default)]
    pub filterable: Option<bool>,
    #[serde(default)]
    pub is_temporal: Option<bool>,
    #[serde(default)]
    pub format: Option<String>,
    /// Semantic role to set; omit to leave unchanged. Validated against [`SEMANTIC_TYPES`];
    /// anything else is a 400. Still `Option<String>` rather than an `Option<enum>` so the
    /// rejection is that 400 and not axum's generic 422 — see [`validate_semantic_type`].
    #[serde(default)]
    #[schema(schema_with = semantic_type_update_schema)]
    pub semantic_type: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ChartDto {
    pub id: String,
    pub dataset_id: String,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<String>,
    pub folder_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ChartDataResponse {
    pub sql: String,
    pub database_id: String,
}

/// A saved chart's query, executed: columns + row-major rows. Mirrors the first
/// two fields of `TabularResult` so the frontend consumes it like a query
/// result. Returned by the object-scoped `POST /charts/{id}/data` endpoint,
/// which runs ONLY the chart's persisted query (no client-supplied spec).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChartRunResponse {
    pub columns: Vec<egret_core::ColumnMeta>,
    /// Row-major cell values; each inner vec is one row aligned to `columns`.
    #[schema(value_type = Vec<Vec<Object>>)]
    pub rows: Vec<Vec<serde_json::Value>>,
}

// ── Dashboard DTOs ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_by: Option<String>,
    pub folder_id: Option<String>,
    pub card_count: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub bookmarked: bool,
    /// A steward has vouched for this dashboard. Display only — the list carries no
    /// `certified_by`, since the badge here answers "is it trusted", and "by whom" is a
    /// question the detail page already answers.
    pub certified: bool,
    /// Whether this caller may run the list's mutating actions — duplicate, move, trash,
    /// delete.
    ///
    /// WORKSPACE-SCOPED, AND THE SAME ON EVERY ROW. Unlike `DashboardDetailDto.can_manage`,
    /// which is evaluated against the individual object, this is one workspace-level decision
    /// copied across the response. That is deliberate: a per-object answer costs two SQL
    /// queries PER ROW (`grants_for` plus the classification-tag lookup inside `CedarPdp`),
    /// so a 100-dashboard list would issue 200 extra queries to draw a menu.
    ///
    /// The cost of the shortcut is a FALSE NEGATIVE, never a false positive: someone who holds
    /// manage only through a per-object or per-folder share sees the row menu withheld here.
    /// They lose nothing they cannot recover — opening the dashboard runs the real per-object
    /// check and gives them the editor. The opposite error, offering Trash to a reader, is the
    /// one that costs.
    pub can_manage: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardCardDto {
    pub id: String,
    pub chart_id: Option<String>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    pub tab_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardTabDto {
    pub id: String,
    pub name: String,
    pub position: i32,
}

/// Body for `PUT /dashboards/{id}/certification`.
///
/// A SEPARATE endpoint from the dashboard PUT on purpose: certification is guarded by the
/// platform-scoped `dashboard:certify`, while editing is guarded by the workspace-scoped
/// `dashboard:manage`. Folding these fields into `UpdateDashboard` would let any editor certify
/// their own dashboard by saving it, which is the failure this feature exists to avoid.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct SetDashboardCertification {
    pub certified: bool,
    /// Free text explaining the certification. Ignored when `certified` is false — withdrawing
    /// clears the note rather than leaving a stale justification attached.
    #[serde(default)]
    pub details: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<String>,
    /// Display name for `created_by` — the profile name, else the email. Resolved server-side:
    /// the endpoints that can turn a user id into a name are admin-gated, so the browser cannot
    /// do it for an ordinary workspace editor. Same shape and same reason as
    /// `MemberDto.principal_name`.
    pub created_by_name: Option<String>,
    /// NULL for dashboards predating migration 0106, which does not invent a creation date.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_by: Option<String>,
    /// Display name for `updated_by`, resolved the same way as `created_by_name`.
    pub updated_by_name: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// The folder this dashboard is filed under; NULL = Uncategorized. Present on
    /// `DashboardSummaryDto` since folders shipped, and absent here until now, which is why the
    /// info sidebar had no folder to name.
    pub folder_id: Option<String>,
    pub width: String,
    /// Marked as trusted by a steward. Same three fields datasets and metrics have carried since
    /// 0098, so "is this certified" is one question across the product rather than three.
    pub certified: bool,
    /// Who certified it, as their email — the string is read by whoever sees the badge, and a
    /// uuid would tell them nothing. Empty while `certified` is false.
    pub certified_by: String,
    /// Why, in the certifier's words. Empty is allowed: a certification with no note is still a
    /// certification.
    pub certification_details: String,
    /// May the CALLER edit this dashboard? Workspace-scoped `dashboard:manage`, answered by the
    /// PDP — the session's permission list carries platform grants only and can never contain it.
    pub can_manage: bool,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
    pub tabs: Vec<DashboardTabDto>,
    pub cards: Vec<DashboardCardDto>,
}

/// One entry in a dashboard's History tab.
///
/// The `snapshot` is deliberately NOT on this DTO. A 22-card dashboard's PUT body is hundreds of
/// kilobytes and the cap is twenty of them, so shipping the content would make opening the info
/// sidebar a multi-megabyte download for a list the user reads as six lines of text. Revert takes
/// a revision id and does the work server-side, where the snapshot already is.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DashboardRevisionDto {
    pub id: String,
    /// `baseline` | `edit` | `revert` — see migration 0107.
    pub kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    /// Display name for `created_by`, resolved server-side for the same reason
    /// `DashboardDetailDto.created_by_name` is.
    pub created_by_name: Option<String>,
    /// True for the revision that matches the dashboard's live state — the newest one. The client
    /// suppresses the revert action on it rather than deriving "newest" from the array order,
    /// which would silently become wrong the day the endpoint gains a sort parameter.
    pub is_current: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Card as supplied by the client when saving a layout.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CardInput {
    /// The id this card already has, so a save can UPDATE it instead of replacing it.
    ///
    /// Absent means "new card" — which is what every client sent before this field existed, and
    /// what a card the user has just dropped onto the canvas still sends. It is a HINT, not an
    /// instruction: the server keeps the id only when it already belongs to THIS dashboard (see
    /// `replace_layout`), so a stale or borrowed id can never make one dashboard adopt another's
    /// card.
    ///
    /// `skip_serializing_if` keeps a body without ids byte-identical to what it was before this
    /// field existed — `update_dashboard_round_trips_through_json_unchanged` caught the `"id":
    /// null` this would otherwise have added to every card of every re-serialized body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub chart_id: Option<Uuid>,
    #[serde(default = "default_card_type")]
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    #[serde(default = "empty_obj")]
    pub options: serde_json::Value,
    #[serde(default)]
    pub tab_index: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NewTabInput {
    /// The id this tab already has. Same contract as [`CardInput::id`]: a hint, honoured only when
    /// the tab already belongs to this dashboard.
    ///
    /// Tabs need it for a reason cards do not: `dashboard_cards.tab_id` is `ON DELETE CASCADE`, so
    /// recreating a tab took every card on it with it — the cards were being deleted twice over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub name: String,
    pub position: i32,
}

fn default_card_type() -> String {
    "chart".into()
}
fn empty_obj() -> serde_json::Value {
    serde_json::json!({})
}
fn empty_arr() -> serde_json::Value {
    serde_json::json!([])
}
/// The width a PUT gets when the client omits the field. Must agree with the column default set
/// by migration 0105 — if these two disagree, a dashboard's width depends on whether the row was
/// created by the API or by an INSERT, which is exactly the kind of split nobody debugs twice.
fn default_width() -> String {
    "fixed".into()
}

/// The dashboard PUT body — and, byte for byte, one stored revision (migration 0107).
/// `Serialize` is derived alongside `Deserialize` so the snapshot is produced from this exact
/// type rather than hand-built JSON that could drift from it; a compile error is how a new field
/// announces that revisions must carry it too.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UpdateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_arr")]
    pub parameters: serde_json::Value,
    #[serde(default = "default_width")]
    pub width: String,
    #[serde(default)]
    pub tabs: Vec<NewTabInput>,
    #[serde(default)]
    pub cards: Vec<CardInput>,
}

/// The complete vocabulary of `dataset_columns.semantic_type`. `""` means "no role".
///
/// THIS IS THE WHOLE LIST, AND IT IS THE `<select>`'s LIST. The vocabulary was never written down
/// anywhere until now: migration 0098 created the column as `text NOT NULL DEFAULT ''` with no
/// CHECK, Rust typed it `String`, utoipa emitted a bare `type: string`, and the only enumeration in
/// the entire codebase was nine `<option value="…">` elements in `ColumnsTab.tsx`. `PUT
/// semantic_type: "banana"` therefore persisted, which is what #438 measured.
///
/// WHY ALL NINE AND NOT THE SIX THAT ARE READ. Six of these drive behaviour today —
/// `currency`/`email`/`url`/`image` are read by `PreviewTab` and `ChartExplore` to default a
/// column's display format, and `latitude`/`longitude` are read by the pin map's coordinate pickers
/// (#438). The other two, `category` and `country`, are WRITE-ONLY: the editor offers them, the API
/// stores them, and nothing anywhere reads them. #438 flagged that as a warning, and the warning
/// cuts the opposite way from how it first reads. A list built from "what does something read?"
/// would have dropped both — and the live deployment's `analytics.dataset_columns` was queried
/// before this list was written: 72 rows, 71 at `''` and ONE AT `category`. A reader-derived
/// allow-list would have rejected a row that exists in production and failed its own migration on
/// deploy. Write-only means "no consumer yet", not "illegitimate"; a semantic role that a future
/// chart type will read is exactly the kind of thing an author annotates in advance.
///
/// CASE- AND FORM-SENSITIVE, deliberately. Every reader compares with `===` against a lowercase
/// bare word, so `"Latitude"` and Metabase's own `"type/Latitude"` spelling are not near-misses to
/// be helpfully coerced — they are values that would store cleanly and then produce a picker that
/// never fills, with no error at any layer. Rejecting them is the point. Note this is a divergence
/// from Metabase, which namespaces its semantic types (`type/Latitude`, `type/Email`); egret chose
/// bare lowercase words in 0098 and the readers were written against them, so the vocabulary is
/// what egret's own code reads, not what Metabase's does. Changing to Metabase's spelling would be
/// a data migration plus three reader changes, and is not this.
pub const SEMANTIC_TYPES: [&str; 9] = [
    "",
    "category",
    "country",
    "currency",
    "email",
    "url",
    "image",
    "latitude",
    "longitude",
];

/// Reject a `semantic_type` outside [`SEMANTIC_TYPES`]. `Err` carries a message for a 400.
///
/// WHY VALIDATION SITS ON THE WRITE PATH AND NOT IN THE TYPE. The obvious shape is
/// `enum SemanticType` with serde, and it was tried and rejected for a specific failure mode: the
/// row struct `DatasetColumn` is `sqlx::FromRow`, so an enum there makes DECODING a stored value
/// fallible. One legacy row holding anything unexpected — and rows predating any constraint are
/// precisely what we cannot rule out on every deployment — would then fail to decode, and it would
/// not fail narrowly: `load_detail` selects all of a dataset's columns at once, so a single bad row
/// takes out the whole dataset detail page with a 500. Strictness on read converts a cosmetic data
/// problem into an outage. Reads therefore stay `String` and stay total; only writes are narrowed.
///
/// A SERDE ENUM ON `UpdateColumnBody` ALONE WAS ALSO REJECTED, more narrowly. It would reject
/// unknown values at deserialisation, which is the right moment, but the failure surfaces as axum's
/// generic 422 for a malformed body — "unknown variant `banana`", with no statement of what the
/// permitted values are and no distinction from a genuinely malformed request. The handler already
/// answers a comparable mistake with a plain 400 that says what is wrong ("cannot set an expression
/// on a physical column"), so this matches that.
///
/// WHAT HAPPENS TO A VALUE ALREADY STORED THAT IS NOT IN THE LIST. Nothing, at read time — it is
/// served verbatim, the editor's `<select>` matches no option and renders blank, and every reader's
/// `===` misses, which is the same do-nothing behaviour an unknown value has always had. It cannot
/// be re-saved as itself: the next edit through the API must choose a permitted value. Migration
/// 0104 additionally normalises any such value to `''` before adding its CHECK, so after deploy
/// there are none; see that migration for why it normalises rather than failing.
/// The OpenAPI schema for a `semantic_type` field, BUILT FROM [`SEMANTIC_TYPES`] rather than
/// restating it.
///
/// The root cause #438 identified was that the vocabulary existed in exactly one place and that
/// place was a `<select>` in a React component — so the API's own contract could not tell you what
/// the field accepted, and the generated client typed it `string`. Emitting an `enum` fixes that,
/// but the obvious way to emit one (a `#[derive(ToSchema)]` enum beside the const) would create a
/// THIRD copy of the vocabulary and a third thing to forget to update. `schema_with` takes the
/// values straight from the constant the validator uses, so the documented contract and the
/// enforced contract cannot disagree — there is nothing to keep in step.
pub fn semantic_type_schema() -> utoipa::openapi::schema::Object {
    utoipa::openapi::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::Type::String)
        .enum_values(Some(SEMANTIC_TYPES))
        .description(Some(SEMANTIC_TYPE_DESC))
        .build()
}

const SEMANTIC_TYPE_DESC: &str = "Semantic role driving smart rendering and chart defaults; \
     \"\" = none. Rejected with 400 if not one of the listed values.";

/// [`semantic_type_schema`] for the UPDATE body, where the field is `Option<String>`.
///
/// NULL IS PERMITTED HERE AND ONLY HERE, and it is not the same thing as `""`. On
/// `UpdateColumnBody` every field is an `Option` meaning "leave this one alone", so `null` is the
/// documented no-op and `""` is the documented "clear the role" — two different requests. Emitting
/// the plain non-nullable schema on both sides would have narrowed this field's contract relative to
/// its own siblings (`format`, `verbose_name` and the rest all publish `["string","null"]`), which
/// is a change nobody asked for and would have shipped as a side effect of adding the enum. `null`
/// has to appear in the `enum` as well as in the type: JSON Schema applies `enum` to the value
/// regardless of type, so a null that is not listed is rejected by the very keyword added to make
/// the field clearer.
pub fn semantic_type_update_schema() -> utoipa::openapi::schema::Object {
    let mut values: Vec<serde_json::Value> = SEMANTIC_TYPES.iter().map(|s| (*s).into()).collect();
    values.push(serde_json::Value::Null);
    utoipa::openapi::ObjectBuilder::new()
        .schema_type(utoipa::openapi::schema::SchemaType::from_iter([
            utoipa::openapi::schema::Type::String,
            utoipa::openapi::schema::Type::Null,
        ]))
        .enum_values(Some(values))
        .description(Some(SEMANTIC_TYPE_DESC))
        .build()
}

pub fn validate_semantic_type(value: &str) -> Result<(), String> {
    if SEMANTIC_TYPES.contains(&value) {
        return Ok(());
    }
    // The permitted values are listed in the error because an allow-list whose rejection does not
    // say what IS allowed just relocates the guesswork to the caller. `''` is rendered as `""` so
    // that "no role" is visibly one of the choices rather than looking like a formatting slip.
    let allowed = SEMANTIC_TYPES
        .iter()
        .map(|s| format!("{s:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "unknown semantic_type {value:?}; expected one of {allowed}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value the dataset column editor's Role `<select>` can produce. Kept here as a literal
    /// rather than read from the constant so that this list and the constant have to be changed
    /// together and neither can silently drift from the other.
    const UI_VOCABULARY: [&str; 9] = [
        "",
        "category",
        "country",
        "currency",
        "email",
        "url",
        "image",
        "latitude",
        "longitude",
    ];

    #[test]
    fn update_dashboard_width_defaults_to_fixed_when_the_client_omits_it() {
        // Agreeing with migration 0105's column default. A PUT body with no `width` used to reset
        // the dashboard to full width, which for a fixed dashboard is a silent data loss through
        // an endpoint the client thought it was using to rename something.
        let b: UpdateDashboard = serde_json::from_str(r#"{"name":"D"}"#).unwrap();
        assert_eq!(b.width, "fixed");
    }

    #[test]
    fn update_dashboard_width_is_taken_from_the_body_when_present() {
        // The default must not shadow an explicit value — the frontend always sends one.
        let b: UpdateDashboard = serde_json::from_str(r#"{"name":"D","width":"full"}"#).unwrap();
        assert_eq!(b.width, "full");
    }

    #[test]
    fn accepts_every_value_the_column_editor_can_produce() {
        for v in UI_VOCABULARY {
            assert!(
                validate_semantic_type(v).is_ok(),
                "the Role select offers {v:?} and the API must accept it"
            );
        }
    }

    #[test]
    fn accepts_category_which_the_live_deployment_already_stores() {
        // MEASURED, not assumed. The kind deployment's `analytics.dataset_columns` holds 72 rows:
        // 71 at '' and one at 'category'. `category` is one of the two values #438 correctly
        // identified as WRITE-ONLY — nothing reads it — and it is exactly the value a
        // "only allow what something reads" list would have dropped, which would have rejected a
        // row that exists in production and failed the migration on deploy. Write-only is not the
        // same as illegitimate.
        assert!(validate_semantic_type("category").is_ok());
        assert!(validate_semantic_type("country").is_ok());
    }

    #[test]
    fn rejects_a_value_that_is_not_in_the_vocabulary() {
        // The defect #438 measured: `PUT semantic_type: "banana"` persisted.
        assert!(validate_semantic_type("banana").is_err());
    }

    #[test]
    fn rejects_a_near_miss_that_would_silently_not_fill_the_pin_map() {
        // The whole reason this stopped being harmless. `map.tsx` compares `c.semantic_type ===
        // "latitude"` exactly, so any of these stores cleanly and then produces a lat/lon picker
        // that does not fill — a typo with no error anywhere, which is the worst shape of bug.
        for v in [
            "Latitude",
            "LATITUDE",
            "lat",
            " latitude",
            "latitude ",
            "type/Latitude",
        ] {
            assert!(
                validate_semantic_type(v).is_err(),
                "{v:?} is not what the pin map reads and must not be storable"
            );
        }
    }

    #[test]
    fn the_error_names_the_offending_value_and_the_permitted_ones() {
        // An allow-list whose rejection does not say what IS allowed just moves the guesswork.
        let e = validate_semantic_type("banana").unwrap_err();
        assert!(
            e.contains("banana"),
            "error should quote the rejected value: {e}"
        );
        assert!(
            e.contains("latitude"),
            "error should list the permitted values: {e}"
        );
    }

    #[test]
    fn the_allow_list_is_exactly_the_editor_vocabulary() {
        // Guards the drift in both directions: an option added to the `<select>` without being
        // added here would 400 in the UI, and a value added here without an option would be dead.
        let mut allowed = SEMANTIC_TYPES.to_vec();
        let mut ui = UI_VOCABULARY.to_vec();
        allowed.sort_unstable();
        ui.sort_unstable();
        assert_eq!(allowed, ui);
    }

    #[test]
    fn the_openapi_schema_publishes_the_same_vocabulary_the_validator_enforces() {
        // The point of `schema_with`: what the API documents and what it enforces are one list.
        // Asserted on the emitted JSON rather than on the builder call so that a utoipa upgrade
        // that changed how `enum_values` renders would be caught rather than assumed away.
        let v = serde_json::to_value(semantic_type_schema()).unwrap();
        let published = v["enum"]
            .as_array()
            .expect("the schema must publish an enum, not a bare string")
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(published, SEMANTIC_TYPES.to_vec());
        assert!(
            published.iter().any(|s| s.is_empty()),
            "'' is a real value (no role) and must be documented as one"
        );
    }

    #[test]
    fn the_update_schema_keeps_null_alongside_the_vocabulary() {
        // `null` = leave unchanged, `""` = clear the role. Both must survive adding the enum, and
        // `null` has to be in the enum too or the keyword rejects the value its own type allows.
        let v = serde_json::to_value(semantic_type_update_schema()).unwrap();
        let ty = v["type"]
            .as_array()
            .expect("nullable field keeps a type array");
        assert!(ty.iter().any(|t| t == "null"), "lost nullability: {ty:?}");
        let e = v["enum"].as_array().unwrap();
        assert!(
            e.contains(&serde_json::Value::Null),
            "null must be permitted: {e:?}"
        );
        assert!(e.contains(&serde_json::Value::from("latitude")));
        assert_eq!(e.len(), SEMANTIC_TYPES.len() + 1);
    }

    #[test]
    fn schema_node_serializes() {
        let c = SchemaColumn {
            name: "id".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"name":"id","type":"bigint"})
        );
    }

    #[test]
    fn update_dashboard_round_trips_through_json_unchanged() {
        // A revision is stored by serialising this DTO and restored by deserialising it. If the
        // two directions disagree about ANY field, a revert silently restores a different
        // dashboard than the one the user picked — with no error at any layer.
        let body = serde_json::json!({
            "name": "Ops",
            "description": "d",
            "parameters": [{ "id": "p1", "name": "Cat", "slug": "cat", "type": "category" }],
            "width": "fixed",
            "tabs": [{ "name": "A", "position": 0 }],
            "cards": [{
                "chart_id": "6f9619ff-8b86-d011-b42d-00c04fc964ff",
                "card_type": "chart",
                "grid_col": 3, "grid_row": 1, "size_x": 6, "size_y": 4,
                "options": { "vizOverride": { "cartesian.stack": true } },
                "tab_index": 0
            }]
        });
        let dto: super::UpdateDashboard =
            serde_json::from_value(body.clone()).expect("deserialises");
        assert_eq!(serde_json::to_value(&dto).unwrap(), body);
    }
}
