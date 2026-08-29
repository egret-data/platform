import { useRef, useState, type ReactNode } from "react";
import { Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import MonacoEditor from "@/components/monaco/MonacoEditor";
import {
  useDataApiGetOpenapi,
  useDataApiListEndpointVersions,
  useDataApiListServiceAccounts,
} from "@/api/generated/data-api/data-api";
import type { ColumnSpec, TransformSpec } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";
import { DownloadOpenApiDialog } from "../DownloadOpenApiDialog";

/**
 * `crates/egret-dataapi/src/lifecycle.rs`'s `MAX_TRANSFORM_SCRIPT_BYTES` — no
 * mechanism in this repo shares a constant between Rust and TypeScript (the
 * same gap `MAX_STATEMENTS` in `Query.tsx` documents), so this is a copy, not
 * a reference. Shown so an author sees the cap while writing, rather than
 * meeting it only in a refusal toast after Save.
 */
const MAX_TRANSFORM_SCRIPT_BYTES = 8192;

/**
 * The script's size the way the server measures it: `bytes`, not
 * `.length` — a `String`'s `.len()` on the Rust side counts UTF-8 bytes, and
 * `.length` on a JS string counts UTF-16 code units. The two agree for ASCII
 * script text and diverge the moment an author's comment uses a non-ASCII
 * character, which is exactly the case a byte-accurate counter exists to get
 * right.
 */
function scriptByteLength(script: string): number {
  return new TextEncoder().encode(script).length;
}

/** A fresh, syntactically valid transform an author can edit from — the same
 *  shape `openapi_gen.rs`'s own doc-test fixture uses, so a screenshot of
 *  either would show the same script. */
const DEFAULT_TRANSFORM_SCRIPT = 'def transform(params, statements):\n    return {}\n';

/** A new declared return column. `required: true` explicitly, matching
 *  `ColumnSpec`'s own default (`mayBeAbsent` below reads it the same way a
 *  frozen SQL column would) — the author LOOSENS a freshly-added row by
 *  unchecking Required, rather than the row starting loose by omission. */
function newReturnColumn(): ColumnSpec {
  return { name: "", ty: "", required: true };
}

/**
 * `crates/egret-dataapi/src/lifecycle.rs`'s `MAX_TRANSFORM_RETURNS`. A copy,
 * for the same reason `MAX_TRANSFORM_SCRIPT_BYTES` above is one.
 */
const MAX_TRANSFORM_RETURNS = 64;

/** What is wrong with one row's `name`, in the order `check_transform_shape`
 *  decides it. `null` means nothing is. */
type ReturnNameProblem = "blank" | "padded" | "duplicate";

/**
 * Why each row's `name` would be refused at publish — one problem per row,
 * `null` where there is none.
 *
 * # This is a PREVIEW of a server rule, and it is now the same rule
 *
 * It used to be a different one, in both directions, and both were reachable
 * from the editor:
 *
 * - the editor trimmed and skipped blanks, so **two unnamed rows looked fine**
 *   here and publish refused them with ``duplicate name ""`` — a refusal
 *   naming a column that does not exist;
 * - the editor compared trimmed names, so `"total"` beside `"total "` was
 *   **flagged here and published happily** — two OpenAPI properties nobody can
 *   tell apart.
 *
 * `check_transform_shape` now refuses a blank name as blank and a padded name
 * as padded, BEFORE its duplicate rule. So by the time either side compares
 * names for duplication, every name is non-blank and already trimmed, and
 * comparing verbatim (Rust) and comparing trimmed (here) are the same
 * comparison. That is the agreement, and it is why the duplicate pass below
 * uses the raw `name`.
 *
 * # Why it is checked twice at all
 *
 * The server is the gate — no write route calls `check_transform`, so this
 * editor is the only thing between an author and a refusal that arrives at
 * Preview or Publish. This exists to move that refusal to the keystroke, never
 * to replace it: a row this function passes can still be refused by the server,
 * and a row it flags is always refused by the server.
 */
function returnNameProblems(returns: ColumnSpec[]): (ReturnNameProblem | null)[] {
  // Counted first, so BOTH rows of a colliding pair are marked rather than
  // only the second. The server names the collision once (it has one error to
  // return); the author has two rows on screen and needs to see which two.
  // Only well-formed names are counted — a blank or padded row has its own,
  // more specific problem and must not also read as somebody's duplicate.
  const counts = new Map<string, number>();
  for (const c of returns) {
    if (c.name.trim() !== "" && c.name.trim() === c.name) {
      counts.set(c.name, (counts.get(c.name) ?? 0) + 1);
    }
  }
  return returns.map((c) => {
    if (c.name.trim() === "") return "blank";
    if (c.name.trim() !== c.name) return "padded";
    if ((counts.get(c.name) ?? 0) > 1) return "duplicate";
    return null;
  });
}

/** The sentence shown under a row whose name would be refused. */
const RETURN_NAME_PROBLEM_TEXT: Record<ReturnNameProblem, string> = {
  blank: "Name this column — a declared column is matched against the response by name, and a blank name matches nothing.",
  padded:
    "Remove the leading or trailing space — the name becomes a JSON key exactly as written, so this and its trimmed spelling would be two columns nobody can tell apart.",
  duplicate: "Declared more than once — a JSON object has one key per name.",
};

/**
 * Just enough of the generated OpenAPI document to show the base URL. The
 * generated hook types the 200 body as `unknown` (it is assembled as
 * `serde_json::Value` server-side — see `openapi_gen.rs`'s module comment —
 * not through utoipa, so orval has no schema to generate a model from), so
 * this is a local, minimal shape rather than a generated one.
 */
interface OpenApiDocPreview {
  servers?: { url?: string }[];
}

/**
 * What a consumer actually receives when they call this endpoint: the frozen
 * columns of the LIVE version (never the draft — a draft can differ from what
 * is currently served, and this section exists to show the real contract), the
 * `output` controls that decide how those columns are keyed and wrapped in the
 * response body, and the served base URL plus the per-service-account OpenAPI
 * download.
 *
 * # One contract per statement
 *
 * A snapshot freezes a LIST of statements (M2), each with its own columns, so
 * this section renders one `Name | Type` table PER statement. At one statement
 * — the common case, and the shape migration 0129 gave every endpoint that
 * pre-dated the list — it renders exactly the one unlabelled table it always
 * did: that endpoint answers with an ARRAY, so its statement name is not a key
 * any caller reads, and printing it here would put a word on screen that looks
 * like part of the contract and is not one. With several, `data` and
 * `meta.statements` are both objects keyed by that name, so the name IS the
 * contract and each table is labelled with it.
 *
 * The four FETCH states stay one table between them. A state is about the
 * request, not about a statement; repeating "Loading…" once per statement would
 * say it N times about one fetch.
 *
 * # Why there are no `Card`s here any more, having deliberately kept two
 *
 * #521 kept `Frozen columns` and `OpenAPI document` as cards on a recorded
 * argument (`docs/en/ui-conventions.md` §4.2): neither is
 * `Setting | Details | Value` shaped, so folding either into the shared
 * `ConfigTable` would fabricate a Value column for read-only output and make
 * it look editable. **That half of the argument is still correct and neither
 * block has become a `ConfigTable` row.** What changed is that it was a
 * choice between two options and there are now three.
 *
 * §4.3 — written after #521, converting `Versions` and `Access` — established
 * the third: a heading plus a plain `Table` in the section's `space-y-6`
 * rhythm, with loading / error / empty as rows *spanning* the table. Measured
 * against that:
 *
 * - **Frozen columns is the textbook §4.3 shape.** `Name | Type`, one row per
 *   column, read down, no Value anywhere in it — structurally identical to the
 *   publish history `Versions` converted, whose own doc comment cites §4.2 as
 *   its precedent while doing the opposite thing. It was also the section's
 *   third table idiom: a hand-written `<table>` beside a `ConfigTable` and,
 *   elsewhere in the tab set, the shared `Table`.
 *
 *   Dropping the card is what *forces* the states into the table, and that is
 *   the reader-visible part. Before, the branch replaced the table and the
 *   `Name`/`Type` headers went with it — so an author who had never published
 *   got a sentence with nothing on screen saying what a frozen column even is,
 *   and the card's title was carrying that alone. As spanning rows the columns
 *   are named in all four states.
 *
 * - **The OpenAPI block is not a list, so §4.3 does not reach it** — but "an
 *   action is not a setting" is an argument against `ConfigTable`, not an
 *   argument for a frame. `Versions` already renders exactly this shape
 *   (prose, then a button that opens a dialog) with an `h3` and no card, and
 *   the one thing here that wants a visual boundary — the 409 — is an `Alert`,
 *   which brings its own. Keeping it would have left the endpoint editor with
 *   a single card in one section out of six, which reads as an oversight
 *   rather than as a distinction, and the next reader would re-open the
 *   question this comment exists to close.
 *
 * The complaint that removed the `ConfigSection`s (§4.1) was about cards
 * framing near-empty tables, not about cards — that remains true, and it is
 * not the reason either of these went. The reason is that both blocks read as
 * the same *kind* of thing as their four sibling sections, and now look it.
 *
 * The `output` controls are a `Setting | Details | Value` table whose "Output
 * shape" heading is a `ConfigTableGroup` row. `Envelope` joined `Shape` and
 * `Naming` there because it had shipped with no control at all: `bare` was
 * reachable through the API and invisible to whoever publishes the endpoint —
 * the same gap `required` had, and had to be come back for.
 *
 * # One table for the settings, and a measured premise for what stays out
 *
 * The reported complaint about this tab was that `Config` carries the same
 * amount of material as ONE table with group rows while this section stacked
 * four blocks, each with a flat `text-sm` heading and a shape of its own.
 * Three of the four were re-read against that:
 *
 * - **`Response transform` moved IN**, as a `ConfigTableGroup` plus one
 *   `ConfigTableRow`, above `Output shape`. Its old argument — the script and
 *   the returns grid are `ParamEditor`-shaped, not one
 *   `Setting | Details | Value` row — is true of the script and the grid and
 *   was never true of the TOGGLE, which is a checkbox with a sentence of help
 *   beside it. On an endpoint with no transform the toggle is the only part of
 *   the block on screen, so the tab was carrying a titled block, permanently,
 *   to house one checkbox. The script and the grid take a full-width
 *   `colSpan={3}` row of the same table — the device this table already used
 *   for the `SettingsNote` that stands in for `Shape` and `Naming` — because
 *   `valueClassName` is 384px at `lg` and a code editor is not a 384px thing.
 *
 * - **`Frozen columns` stays out, and the premise is now a number.** There is
 *   a third option the §4.3 argument above never weighed: `name | why | type`
 *   maps onto `Setting | Details | Value` without fabricating anything,
 *   because the type genuinely IS this listing's value. It loses on geometry.
 *   `ColumnTable` is `table-fixed` with `w-72` on Name, so the type starts
 *   288px after the name at any table width — and that number is the whole
 *   reason this frame was rewritten (see its own comment: under auto layout
 *   inside `max-w-xl` it was 444px, and "`bigint` sat far from the name"
 *   was the complaint). In a `ConfigTable` the Value column sits after
 *   `Setting` (288) AND `Details` (624 at a 1296px content box), so the type
 *   would start 912px after the name. A `Details` cell cannot collapse per
 *   row; the column is sized once for the table.
 *
 * - **`OpenAPI document` stays out on kind and changed SHAPE.** An action is
 *   not a setting — unchanged. What it stopped being is a third block layout:
 *   it is now `Config.tsx`'s `Validate` shape, a `border-t` block with the
 *   heading and a `size="sm"` button on one line. See its own comment below.
 *
 * # What a frozen column's `required` adds, and why it is a badge
 *
 * A conditional definition describes EVERY branch at publish and freezes the
 * UNION of their columns, flagging `required` for the ones every branch returns
 * (`serving/router.rs#shape_rows`). A consumer sees that distinction twice — in
 * the served `meta.columns` and in the generated document's per-schema
 * `required` list — and an author saw it nowhere: `Name | Type` renders a
 * column a caller can rely on identically to one that vanishes on some
 * parameter values.
 *
 * **The mark rides in the Name cell; there is no third column.** §4.3's "a
 * column whose every cell is identical is section-level copy in disguise" is
 * the reason. Every endpoint published before the branching-columns work
 * carries `required: true` on every column — the serde default exists so those
 * snapshots keep exact-match semantics — and so does every endpoint whose
 * definition does not branch, which is most of them. A `Required` column would
 * therefore hold one repeated word in every cell of the common case, and would
 * put a third column between a name and the type it describes in the one table
 * on this page whose whole job is to keep those two side by side. Marking only
 * the exceptions
 * leaves the common case rendering byte-identically to what it did before, and
 * the two tests that already pinned this table's grid did not have to change.
 *
 * §4.3's other half is why the caveat sentence exists: with the exception
 * carried by a badge, the ABSENCE of one is what says "a consumer can rely on
 * this", and that has to be said once above the table rather than inferred. It
 * renders only when a marked row is actually on screen — an endpoint with no
 * branching column has no marker to explain, and a sentence about one would be
 * the noise a badge was chosen over a column to avoid.
 *
 * **Not a hover, not a colour.** `title` and an icon both hide the fact from a
 * reader who is not using a pointer, and §4.4 already ruled that the
 * load-bearing half of a per-row caveat stays on screen with nothing clicked.
 * The badge is a text label; `variant="outline"` is the neutral tone
 * `AccessSection` already uses for a per-row fact that is not a status.
 */
/**
 * Whether a consumer may find this column MISSING from a response object.
 *
 * `=== false`, never `!c.required`, and the difference is the whole feature.
 * `required` carries `#[serde(default)]` on the Rust side and so is absent from
 * the schema's own `required` array — orval therefore types it
 * `required?: boolean`, and `undefined` is exactly what a snapshot frozen
 * before the branching-columns work persisted. The server reads those as
 * `true` (`ColumnSpec`'s serde default) so an already-published endpoint keeps
 * exact-match semantics; `!c.required` would read them as `false` and tell an
 * author that every column of every pre-existing endpoint may be absent —
 * inverting the fact for the majority of endpoints on the instance.
 */
function mayBeAbsent(column: { required?: boolean }): boolean {
  return column.required === false;
}

/**
 * The `Name | Type` frame, with whatever rows it is given.
 *
 * # Why it is `table-fixed` with a sized Name, and no longer `max-w-xl`
 *
 * Two short columns — a column name and a type — and the complaint the cap was
 * added for was that `bigint` sat far from the name it describes. Measured in
 * Chromium at 1600×1000, the cap did not actually fix that: `w-full` AUTO
 * layout inside `max-w-xl` gave Name 443.9px of the table's 576, so the type
 * started 444px in. Auto layout sizes columns from their content, so the gap
 * grew with the longest column name and the cap only bounded it.
 *
 * `table-fixed` with `w-72` on Name puts the type 288px in at ANY table width,
 * which is both closer and stable — and `w-72` is the width the
 * `Setting | Details | Value` table below uses for its own first column, so the
 * two read as one grid down the section instead of as two tables that happen to
 * be stacked. That is what lets the frame follow the section's measure (the
 * page) rather than needing a cap of its own: the audit measured this table at
 * 575px against the settings table's 1020px, one of the four widths the section
 * stacked.
 *
 * The name cell therefore wraps rather than scrolls (`whitespace-normal
 * break-all` there): the shared `TableCell` is `whitespace-nowrap`, and under
 * fixed layout a long identifier would otherwise overflow a column that can no
 * longer grow to hold it.
 *
 * A component rather than a copy per branch because a several-statement
 * contract renders one of these PER STATEMENT, and the headers are what say
 * what the list records in every state.
 */
function ColumnTable({ children }: { children: ReactNode }) {
  return (
    <Table className="table-fixed">
      <TableHeader>
        <TableRow>
          <TableHead className="w-72">Name</TableHead>
          <TableHead>Type</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>{children}</TableBody>
    </Table>
  );
}

/** One row per frozen column, with the exception marked. */
function ColumnRows({ columns }: { columns: ColumnSpec[] }) {
  return (
    <>
      {columns.map((c) => (
        <TableRow key={c.name}>
          {/* The badge marks the EXCEPTION, and rides in the Name cell rather
              than in a `Required` column of its own. §4.3: a column whose every
              cell is identical is section-level copy in disguise — and every
              endpoint published before the branching-columns work, plus every
              endpoint whose definition does not branch, has every column
              guaranteed, so a third column would hold one repeated word in
              every cell of the common case, in the one table on this page
              whose sized `Name` column exists precisely to keep the type beside
              the name it describes.

              Text, not a colour or an icon: it has to be legible without a
              hover and to a reader who is not reading visually. The Name cell
              keeps its own `font-mono text-xs` on the name alone — the badge's
              label is prose. */}
          <TableCell className="py-2 whitespace-normal break-all">
            <span className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-xs">{c.name}</span>
              {mayBeAbsent(c) && (
                <Badge variant="outline" className="text-muted-foreground">
                  May be absent
                </Badge>
              )}
            </span>
          </TableCell>
          <TableCell className="py-2 text-muted-foreground">{c.ty}</TableCell>
        </TableRow>
      ))}
    </>
  );
}

/**
 * The `Name | Type | Required` table for `output.transform.returns` — the
 * contract a transform script promises, checked against what it actually
 * returns at publish (`check_returns`) and enforced on every request after
 * (`serving/router.rs`).
 *
 * **One table row per declaration, following `ParamEditor`'s precedent** —
 * the same reviewer-verified shape: a plain `components/ui/table`, one
 * control per field, every control's `aria-label` naming its row (`Return 2
 * name`) because a visible `<th>` cannot say which row a control belongs to.
 * `ty` is free text rather than a `Select` — unlike a parameter's `ParamType`,
 * a column's type is whatever string the SQL engine or the script names
 * (`"integer"`, `"varchar"`, a script's own `"object"`), and there is no
 * closed set here to offer.
 *
 * Rows are keyed by a stable, locally-generated id — never the array index —
 * for the same reason `ParamEditor` is: React would otherwise re-associate a
 * surviving row's DOM node with the NEXT row's data when a row in the middle
 * is removed.
 */
function ReturnsEditor({
  returns,
  onChange,
}: {
  returns: ColumnSpec[];
  onChange: (returns: ColumnSpec[]) => void;
}) {
  const nextRowId = useRef(returns.length);
  const [rowIds, setRowIds] = useState<number[]>(() => returns.map((_, i) => i));
  const problems = returnNameProblems(returns);
  const overCap = returns.length > MAX_TRANSFORM_RETURNS;

  function update(i: number, patch: Partial<ColumnSpec>) {
    onChange(returns.map((c, j) => (j === i ? { ...c, ...patch } : c)));
  }
  function remove(i: number) {
    onChange(returns.filter((_, j) => j !== i));
    setRowIds((ids) => ids.filter((_, j) => j !== i));
  }
  function add() {
    const rowId = nextRowId.current++;
    onChange([...returns, newReturnColumn()]);
    setRowIds((ids) => [...ids, rowId]);
  }

  return (
    <div className="space-y-2">
      {/* Serving would have nothing to check a transform's response against —
          it would fail OPEN, serving whatever the script returned unchecked.
          `TransformReturnsEmpty` is what `check_transform` gives for this —
          but `check_transform` runs from `validate_endpoint`, which only
          `/validate` (publish) and `/preview` call. Create and update
          (`routes.rs`'s `create_endpoint`/`update_endpoint`) call neither, so
          an empty `returns` list SAVES successfully; the refusal only
          arrives at Preview or Publish. This alert previews that refusal
          before the author reaches it — it does not claim Save itself is
          gated, because it is not. */}
      {returns.length === 0 && (
        <Alert variant="destructive">
          <AlertDescription>
            No return columns declared. Serving checks a transform&rsquo;s response against this
            list — with none, there is nothing to check, so a published endpoint would serve
            whatever the script returns, unchecked. Preview and Publish both refuse this; add at
            least one before either.
          </AlertDescription>
        </Alert>
      )}
      {/* The `returns` cap, previewed the same way the script's byte cap is:
          `check_transform_shape` refuses more than `MAX_TRANSFORM_RETURNS`,
          and no write route checks a transform, so without this an author
          builds the list and meets the refusal at Preview or Publish. */}
      {overCap && (
        <Alert variant="destructive">
          <AlertDescription>
            {returns.length} return columns declared, over the {MAX_TRANSFORM_RETURNS} column cap.
            Serving checks every response against this list on every request. Preview and Publish
            both refuse this.
          </AlertDescription>
        </Alert>
      )}
      {/* Keeps a cap of its own where the frozen list above gave one up, and
          the difference is what each table IS. That one is a read-down LISTING
          whose frame belongs on the section's measure; this is a row of form
          controls — a name input, a type input, a checkbox and a delete — and
          four controls stretched across a wide page put the delete button an
          arm's length from the row it acts on. §5.5 records that a section may
          legitimately cap its own children, and that is the rule this leans on.

          It used to cite `Try it`'s parameter grid as the precedent. That was
          never true and is now checkably false: that grid is a bare
          `<Table className="table-fixed">` with no cap of its own — it was
          sitting inside `TryIt`'s section cap, which §5.1's reversal removed.
          Grepped after that removal, this `max-w-xl` is the ONLY non-prose
          `max-w-*` left anywhere under `detail/`, so it has no precedent and
          stands on the argument above alone. §9: a citation that reads as
          sourced and is not is worse than no citation. */}
      <Table className="max-w-xl table-fixed">
        <TableHeader>
          <TableRow>
            <TableHead className="w-1/2">Name</TableHead>
            <TableHead>Type</TableHead>
            <TableHead className="w-24">Required</TableHead>
            <TableHead className="w-12">
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {returns.map((c, i) => {
            const rowId = rowIds[i] ?? i;
            const row = `Return ${i + 1}`;
            const problem = problems[i] ?? null;
            return (
              <TableRow key={rowId}>
                <TableCell className="align-top">
                  <Input
                    id={`return-name-${rowId}`}
                    aria-label={`${row} name`}
                    // A blank name is the state EVERY freshly added row starts
                    // in, so it is reported as a sentence under the field and
                    // not as `aria-invalid` — announcing "invalid" the instant
                    // an author clicks Add would be the editor shouting at
                    // someone who has not typed yet. `padded` and `duplicate`
                    // are things an author did.
                    aria-invalid={problem !== null && problem !== "blank"}
                    value={c.name}
                    onChange={(e) => update(i, { name: e.target.value })}
                  />
                  {problem && (
                    <p className="mt-1 text-xs text-destructive">
                      {RETURN_NAME_PROBLEM_TEXT[problem]}
                    </p>
                  )}
                </TableCell>
                <TableCell className="align-top">
                  <Input
                    id={`return-type-${rowId}`}
                    aria-label={`${row} type`}
                    value={c.ty}
                    onChange={(e) => update(i, { ty: e.target.value })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  <Checkbox
                    id={`return-required-${rowId}`}
                    className="mt-2.5"
                    aria-label={`${row} required`}
                    checked={!mayBeAbsent(c)}
                    onCheckedChange={(v) => update(i, { required: v === true })}
                  />
                </TableCell>
                <TableCell className="align-top">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove ${row.toLowerCase()}`}
                    onClick={() => remove(i)}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
      <Button type="button" variant="outline" size="sm" onClick={add}>
        <Plus className="h-4 w-4" /> Add return column
      </Button>
    </div>
  );
}

export default function ResponseSection() {
  const { ws, id, endpoint, form, setForm } = useEndpointDetail();

  const versionsQuery = useDataApiListEndpointVersions(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const versionsEnvelope = versionsQuery.data;
  const versions = versionsEnvelope?.status === 200 ? versionsEnvelope.data : [];
  const liveVersion = versions.find((v) => v.is_live);
  // A snapshot freezes one column set PER STATEMENT (M2) — an endpoint has one
  // result set per statement, and a union of them would report a contract no
  // version holds.
  const frozen = liveVersion?.snapshot.statements ?? [];
  // Only the LIVE version's columns, same as the tables below: the caveat
  // explains a marker, so it must be computed from the rows that carry it —
  // across every statement, because any of them can contribute one.
  const hasOptionalColumn = frozen.some((s) => s.columns.some(mayBeAbsent));
  // The endpoint record says a live version exists, but this section's own
  // fetch of it hasn't landed yet — distinct from "never published", which is
  // `!endpoint.live_version_id` below. Conflating the two would flash "no
  // live version" on every load of a perfectly-live endpoint.
  const columnsLoading = !!endpoint.live_version_id && versionsQuery.isLoading;
  // react-query clears `isLoading` on ANY settle, error included — so a fetch
  // failure for a live endpoint falls straight into the `!liveVersion`
  // branch below unless checked first, and reads as "never published" to an
  // author who has already published. Same class of bug this codebase fixed
  // once across four list pages (`Endpoints.tsx`'s own `isError` branch is
  // the pattern this mirrors) — the one section whose whole purpose is
  // representing server state accurately cannot re-introduce it.
  const columnsErrored = !!endpoint.live_version_id && versionsQuery.isError;

  const { data: accountsEnvelope } = useDataApiListServiceAccounts(ws);
  const accounts = accountsEnvelope?.status === 200 ? accountsEnvelope.data : [];
  // Any service account's document carries the same `servers[0].url` — it is
  // derived from `issuer` and the workspace slug, not from which account
  // asked — so the first one is enough to preview it. The download itself
  // (`DownloadOpenApiDialog`) still makes the author choose explicitly.
  const previewAccountId = accounts[0]?.id;

  const oaQuery = useDataApiGetOpenapi(
    ws,
    { service_account_id: previewAccountId ?? "", endpoint_id: id },
    { query: { enabled: !!previewAccountId } },
  );
  const oaEnvelope = oaQuery.data;
  const doc = oaEnvelope?.status === 200 ? (oaEnvelope.data as OpenApiDocPreview) : undefined;
  const baseUrl = doc?.servers?.[0]?.url;
  // The mutator throws the real error on any non-2xx, so react-query's
  // `error` carries it (never `data`) — see `apiFetch`/`mutator.ts`. Typed as
  // `ErrorBody` by the generated hook but actually an `Error` with `.status`,
  // same cast `EndpointDetail.tsx` and `DownloadOpenApiDialog.tsx` already use.
  const oaError = oaQuery.error as unknown as (Error & { status?: number }) | null | undefined;

  const [downloadOpen, setDownloadOpen] = useState(false);

  const transform = form.output.transform ?? null;
  const hasTransform = !!transform;
  const scriptBytes = scriptByteLength(transform?.script ?? "");
  const scriptOverCap = scriptBytes > MAX_TRANSFORM_SCRIPT_BYTES;

  /**
   * Turn the transform ON, from the checkbox below.
   *
   * `shape` and `naming` are cleared to their default in the SAME patch that
   * sets `transform` — not merely hidden — following `ParamEditor`'s
   * `clearInapplicableConstraints`: hiding a control while its old value
   * survives underneath produces exactly the defect that function exists to
   * prevent, a Save whose body still carries the non-default value the editor
   * no longer shows the author, refused by `TransformWithShapeOrNaming` with
   * no control on screen to point at. `undefined` rather than the literal
   * `"objects"`/`"as_is"` strings, matching how every Select above already
   * reads an absent field as its own default.
   */
  /**
   * What `enableTransform` cleared, so that `disableTransform` can put it
   * back.
   *
   * A `ref`, not state: nothing renders from it, and it must not re-render
   * anything when it changes. It is written on the way in and read on the way
   * out, so ticking the box and unticking it leaves `output` exactly as it was
   * found — before this, `shape: "arrays"` was cleared by the tick and simply
   * gone, and an author who ticked the box to look at the script and unticked
   * it again silently published an `objects` endpoint.
   *
   * **What it deliberately does not survive: unmounting.** Leaving the section
   * and coming back restores nothing, because there is nothing left to restore
   * from — the cleared values are not in `form` any more, which is the whole
   * point of clearing them (see `enableTransform`). Within one visit, which is
   * where the accident happens, this is exact.
   */
  const clearedByTransform = useRef<{ shape?: string; naming?: string } | null>(null);

  function enableTransform() {
    setForm((f) => {
      clearedByTransform.current = { shape: f.output.shape, naming: f.output.naming };
      return {
        ...f,
        output: {
          ...f.output,
          shape: undefined,
          naming: undefined,
          transform: { script: DEFAULT_TRANSFORM_SCRIPT, returns: [] },
        },
      };
    });
  }

  function disableTransform() {
    // Read AND clear the ref here, outside the updater — not inside it.
    // `setForm`'s updater can run twice for one call in development: React 19
    // double-invokes a `useState` updater function under `StrictMode`,
    // keeping only the second call's return value and discarding the first's.
    // Clearing `clearedByTransform.current` inside the updater is a side
    // effect that does not rewind between those two calls, so the first
    // invocation would see the remembered value and restore it, and the
    // second — reading the same, now-`null` ref — would fall through to
    // `f.output.shape`/`f.output.naming`, which are `undefined` on the very
    // state being patched. React keeps that second, empty result, silently
    // undoing the restore this function exists to do. Reading the ref once,
    // before `setForm` is even called, makes both invocations pure functions
    // of the same captured `restored` value, so they agree.
    const restored = clearedByTransform.current;
    clearedByTransform.current = null;
    setForm((f) => ({
      ...f,
      output: {
        ...f.output,
        // `?? f.output.shape` rather than an unconditional write: with
        // nothing remembered (a transform that was already on when this
        // section mounted) the field is left exactly as it is, which is the
        // behaviour before this ref existed. Restoring `undefined` there
        // would be this fix inventing a change of its own.
        shape: restored ? restored.shape : f.output.shape,
        naming: restored ? restored.naming : f.output.naming,
        transform: null,
      },
    }));
  }

  /** Patches the transform IN PLACE — spreading the existing object rather
   *  than rebuilding one from `patch` alone is what keeps a script edit from
   *  wiping `returns`, and a `returns` edit from wiping the script. `returns`
   *  is the declared CONTRACT, not a view derived from the script text, so a
   *  keystroke in one must never touch the other. */
  function updateTransform(patch: Partial<TransformSpec>) {
    setForm((f) => {
      const current = f.output.transform;
      if (!current) return f;
      return { ...f, output: { ...f.output, transform: { ...current, ...patch } } };
    });
  }

  return (
    // Uncapped, like all five sibling tabs (§5.1). This section reached that
    // state first and for its own reason, and the reason has since been
    // superseded: §5.1 used to sort sections by what they HOLD — settings
    // tables and code editors use the page, prose and short lists cap
    // themselves — and this section moved groups when it gained a
    // `Setting | Details | Value` table (§4.2) and a Monaco editor for the
    // response transform. That criterion is retired: Access, Try it and
    // Versions are uncapped now too, so there are no longer two groups to sort
    // into. Kept because the measurement below is still why THIS section's cap
    // came off, and the `valueClassName` half of the change depends on it.
    //
    // Measured in Chromium at 1600×1000 with the sidebar expanded, before this
    // change: `max-w-5xl` stopped the section at 1024px inside a 1296px content
    // box — a 288px gutter no block could ever use — while the four blocks
    // ended at 840 / 796 / 1288 / 796. One measure means the frames now share
    // the page's right edge, and PROSE is the single deliberate exception:
    // `max-w-prose` caps a sentence because a line of text has a readable
    // measure of its own (§5.2, "cap the prose, not the column").
    //
    // Removing the cap is not enough on its own — see the `valueClassName` on
    // the `ConfigTable` below, which is the other half of this change.
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Response</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          What a consumer actually receives when they call this endpoint.
        </p>
      </div>

      <div className="space-y-2">
        <h3 className="text-sm font-medium">Frozen columns</h3>
        {/* Said once, above the table, and only when a marked row is on screen
            (§4.3). With the exception carried by a badge rather than by a
            column, an UNMARKED row is otherwise ambiguous between "guaranteed"
            and "nobody recorded it" — and it is the unmarked rows that are the
            answer to "what can a consumer rely on", so leaving that to
            inference is leaving the majority of the table unexplained.

            Conditional because an endpoint whose definition does not branch has
            no marker to explain, and a sentence about one would be exactly the
            per-row noise a badge was chosen over a third column to avoid.

            `max-w-prose`, matching the OpenAPI block's sentence two blocks
            down. It is the one thing in this section that does NOT take the
            page's measure, and deliberately: a table frame can span whatever it
            is given, but a line of running text has a readable measure of its
            own (§5.2, "cap the prose, not the column"). */}
        {hasOptionalColumn && (
          <p className="max-w-prose text-sm text-muted-foreground">
            Columns marked <span className="font-medium">May be absent</span> come from a branch that
            only some parameter values reach — on the calls that skip it the key is missing from the
            row entirely, never null. Every other column is in every response.
          </p>
        )}
        {/* The four states are rows spanning the table rather than a branch
            that replaces it (§4.3), so `Name`/`Type` are on screen in every one
            of them — with the card gone there is otherwise nothing left to say
            what this list even records. `whitespace-normal` because the shared
            `TableCell` is `whitespace-nowrap` and these are sentences.

            One table for the states even when the contract has several
            statements: a state is about the FETCH, not about a statement, and
            repeating "Loading…" once per statement would say it N times about
            one request. */}
        {columnsLoading ? (
          <ColumnTable>
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={2} className="whitespace-normal text-sm text-muted-foreground">
                Loading…
              </TableCell>
            </TableRow>
          </ColumnTable>
        ) : columnsErrored ? (
          <ColumnTable>
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={2} className="whitespace-normal">
                <Alert variant="destructive">
                  <AlertDescription>
                    Could not load the published contract.{" "}
                    {(versionsQuery.error as unknown as Error)?.message ?? "Try again in a moment."}
                  </AlertDescription>
                </Alert>
              </TableCell>
            </TableRow>
          </ColumnTable>
        ) : !endpoint.live_version_id || !liveVersion ? (
          <ColumnTable>
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={2} className="whitespace-normal text-sm text-muted-foreground">
                This endpoint has no live version yet. Publish it to see the columns a consumer will
                receive — these come from the published contract, not the draft.
              </TableCell>
            </TableRow>
          </ColumnTable>
        ) : frozen.length <= 1 ? (
          // The common case, and byte-identical to what it rendered before an
          // endpoint could have several statements: one table, no name. A
          // one-statement endpoint answers with an ARRAY, so its statement name
          // is not a key any caller reads — printing it here would put a word
          // on screen that looks like part of the contract and is not one.
          <ColumnTable>
            <ColumnRows columns={frozen[0]?.columns ?? []} />
          </ColumnTable>
        ) : (
          // With several, the name IS part of the contract: `data` and
          // `meta.statements` are both objects keyed by it. One table each, and
          // each under its own name, because these are separate result sets —
          // a single table with a `Statement` column would read as one contract
          // with a repeated qualifier.
          frozen.map((s) => (
            <div key={s.name} className="space-y-1 pt-2">
              <h4 className="font-mono text-xs font-medium">{s.name}</h4>
              <ColumnTable>
                <ColumnRows columns={s.columns} />
              </ColumnTable>
            </div>
          ))
        )}
      </div>

      {/* `valueClassName`, not just the removed cap above: `ConfigTable` is
          `table-fixed`, so every pixel the section gains goes to Details — the
          only unsized column — and the three Selects stay at the shared 208px
          however wide the page is (§5.2). Measured in Chromium at 1600×1000:
          the trigger was 192px inside a 208px column and is 368px inside a
          384px one, and Details went 528 → 624 rather than taking all 720.

          `lg:` and not a flat `w-96`, which is a measurement rather than a
          hunch (§5.3): below `lg` the Details column is hidden but the two
          sized columns do not shrink to fit — fixed layout honours their pixel
          widths and the shadcn `Table` wrapper scrolls instead — so the
          widening is taken only where there is room to take it. The same pair
          `Config` opts into, on the tab beside this one. */}
      <ConfigTable valueClassName="w-52 lg:w-96" detailsClassName="max-w-prose">
        {/* An author's response transform — a Starlark script plus the `returns`
            it promises (`spec §3`, Tasks 1–8).

            # It used to be a titled block above this table, and that is the
            # defect this task removes

            The recorded argument was that "the script and its returns table
            are the `ParamEditor` shape (a titled block holding a multi-row
            table and, here, a code editor), not a single
            `Setting | Details | Value` row". That is true of the SCRIPT and
            the RETURNS. It was never true of the TOGGLE, which is a checkbox
            with a sentence of help beside it — the exact shape this table
            exists for — and the toggle is the only part of the block a reader
            sees on the endpoints that have no transform, which is most of
            them. So the tab was carrying a titled block, permanently, to
            house one checkbox.

            §4.2's ruling holds for the other two blocks on this tab and is
            unchanged: a read-only listing is not a setting and an action is
            not a setting. This block was neither of those things — it is a
            setting — and it is here now.

            Above `Output shape`, and that is load-bearing rather than
            alphabetical: turning this on REMOVES `Shape` and `Naming` from
            the rows below (see the note in that group), and a control whose
            effect is to delete two rows reads above them rather than under
            them. */}
        <ConfigTableGroup label="Response transform" />
        {/* The second sentence is conditional because it points AT something.
            With a transform on, Shape and Naming are not "below" — they are
            not on the page at all, so telling an author to prefer two controls
            that are not there sat directly above a note explaining that they
            had gone. Seen in a browser, 2026-08-27.

            A plain string, no markup: `details` is typed `string | null` and
            backticks render as literal backticks (§6). The word `data` was
            `<span className="font-mono text-xs">data</span>` while this was a
            paragraph; in a Details cell it is spelled out instead. */}
        <ConfigTableRow
          label="Build the response with a script"
          htmlFor="resp-transform-enabled"
          details={
            hasTransform
              ? "An optional Starlark script that builds the response's data itself once every statement has run, for a shape no SQL projection can express. Most endpoints do not need one — renaming and objects-vs-arrays are what Shape and Naming are for, and unticking this brings them back."
              : "An optional Starlark script that builds the response's data itself once every statement has run, for a shape no SQL projection can express. Most endpoints do not need one — Shape and Naming below already cover renaming and objects-vs-arrays."
          }
        >
          <Checkbox
            id="resp-transform-enabled"
            checked={hasTransform}
            onCheckedChange={(v) => (v === true ? enableTransform() : disableTransform())}
          />
        </ConfigTableRow>
        {/* The script and the returns grid take a FULL-WIDTH row of this same
            table rather than a Value cell, and that is what let the toggle
            move at all: `valueClassName` is 384px at `lg` and a code editor
            and a four-column editable grid are not 384px things. `colSpan={3}`
            is the device this table already uses for the `SettingsNote` below,
            so it is not a new one.

            Rendered only when there IS a transform — not rendered-and-hidden.
            An author with no transform sees one checkbox row, which is the
            whole point of moving it. */}
        {hasTransform && transform && (
          <TableRow className="hover:bg-transparent">
            <TableCell colSpan={3} className="whitespace-normal pt-0 pb-4">
              <div className="space-y-4">
                <div className="space-y-1.5">
                  <p className="text-sm font-medium">Script</p>
                  {/* Fixed height, not `CellEditor`'s grow-with-content: this
                      section is an ordinary scrolling document (`fieldset` in
                      `EndpointDetail`), not a bounded split pane like `Query`'s,
                      so a script that grows the editor also grows the page under
                      it rather than fighting a sibling pane for height. */}
                  <div className="h-64 overflow-hidden rounded-md border border-border">
                    <MonacoEditor
                      height="100%"
                      defaultLanguage="python"
                      value={transform.script}
                      onChange={(v) => updateTransform({ script: v ?? "" })}
                      options={{
                        minimap: { enabled: false },
                        fontSize: 13,
                        scrollBeyondLastLine: false,
                        padding: { top: 10, bottom: 10 },
                        renderLineHighlight: "line",
                        ariaLabel: "Response transform script",
                      }}
                    />
                  </div>
                  <p className={`text-xs ${scriptOverCap ? "text-destructive" : "text-muted-foreground"}`}>
                    {scriptBytes.toLocaleString()} / {MAX_TRANSFORM_SCRIPT_BYTES.toLocaleString()} bytes
                    {scriptOverCap &&
                      " — over the cap. A response transform reshapes what a statement already returned; it is not where an endpoint's business logic belongs, and publish will refuse this script."}
                  </p>
                </div>

                <div className="space-y-1.5">
                  <p className="text-sm font-medium">Declared returns</p>
                  <p className="max-w-prose text-sm text-muted-foreground">
                    What <span className="font-mono text-xs">data</span> looks like once the script
                    runs — checked against the script&rsquo;s own output at publish, and enforced on
                    every request after.
                  </p>
                  <ReturnsEditor
                    returns={transform.returns}
                    onChange={(returns) => updateTransform({ returns })}
                  />
                </div>
              </div>
            </TableCell>
          </TableRow>
        )}
        <ConfigTableGroup label="Output shape" />
        {/* Neither row below is merely disabled while a transform is present
            — it is not rendered at all. `openapi_gen.rs`'s own generated
            `data` description says why, verbatim: once a script decides `data`,
            `shape` and `naming` "describe how a row straight from SQL becomes
            JSON, and a transform's output never goes through that step" — so
            offering them here would be a control whose only effect is being
            silently ignored, which is worse than not offering it. Said once,
            in the author's own words rather than left for them to discover in
            the generated OpenAPI document (Task 7) after the fact. */}
        {hasTransform && (
          <TableRow className="hover:bg-transparent">
            <TableCell colSpan={3} className="whitespace-normal pb-3">
              <SettingsNote>
                {/* "here", not "below": this note is rendered IN the two rows'
                    own place, and the only row under it is Envelope. Seen in a
                    browser, 2026-08-27 — the word sent a reader looking down
                    the table for controls that had been removed. */}
                This endpoint has a response transform, so{" "}
                <span className="font-medium text-foreground">Shape</span> and{" "}
                <span className="font-medium text-foreground">Naming</span> are not shown here —
                they describe how a row straight from SQL becomes JSON, and a transform&rsquo;s
                output never goes through that step. Remove the transform to bring them back.
              </SettingsNote>
            </TableCell>
          </TableRow>
        )}
        {!hasTransform && (
          <>
            <ConfigTableRow
              label="Shape"
              htmlFor="resp-shape"
              details="Objects returns each row as a named object. Arrays returns positional rows, which are smaller but depend on column order."
            >
              <Select
                value={form.output.shape ?? "objects"}
                onValueChange={(v) => setForm((f) => ({ ...f, output: { ...f.output, shape: v } }))}
              >
                <SelectTrigger id="resp-shape" className="w-full" aria-label="Shape">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="objects">Objects</SelectItem>
                  <SelectItem value="arrays">Arrays</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>
            <ConfigTableRow
              label="Naming"
              htmlFor="resp-naming"
              details="How column names are written in the response: as the query returns them, camelCase, or snake_case. Two columns that collapse onto one name make the endpoint refuse to serve until it is re-published."
            >
              <Select
                value={form.output.naming ?? "as_is"}
                onValueChange={(v) => setForm((f) => ({ ...f, output: { ...f.output, naming: v } }))}
              >
                <SelectTrigger id="resp-naming" className="w-full" aria-label="Key naming">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="as_is">As-is</SelectItem>
                  <SelectItem value="camel">camelCase</SelectItem>
                  <SelectItem value="snake">snake_case</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>
          </>
        )}
        {/* `output.envelope` shipped with no control at all — `bare` was
            reachable through the API and invisible to the person publishing the
            endpoint, which is the same gap `required` had and had to be come
            back for.

            The default is spelled here as `"wrapped"` rather than left blank,
            and it matches `serving::router::envelope_of`: an absent value —
            what every endpoint saved before this field existed carries — serves
            the WRAPPER, so an unpublished endpoint cannot change shape by being
            re-read. Anything unrecognised serves the wrapper too, and logs. */}
        <ConfigTableRow
          label="Envelope"
          htmlFor="resp-envelope"
          details="Wrapped answers with status, message, data and meta around the rows. Bare answers with the rows alone — no version, no cache flag, no request id. A refused call still answers with the full error body either way."
        >
          <Select
            value={form.output.envelope ?? "wrapped"}
            onValueChange={(v) => setForm((f) => ({ ...f, output: { ...f.output, envelope: v } }))}
          >
            <SelectTrigger id="resp-envelope" className="w-full" aria-label="Envelope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="wrapped">Wrapped</SelectItem>
              <SelectItem value="bare">Bare — data only</SelectItem>
            </SelectContent>
          </Select>
        </ConfigTableRow>
      </ConfigTable>

      {/* The tab's one ACTION, and §4.2's ruling on it is unchanged: an action
          is not a setting, so it is not a row of the table above. What changed
          is the shape it takes OUTSIDE that table.

          It used to be a heading, then a paragraph, then a button on a line of
          its own — a third block layout on a tab that already has a read-only
          listing and a settings table, which is the "four shapes" complaint
          this task answers. `Config.tsx` ships the same KIND of thing at the
          end of the tab beside this one (`Validate`: an action over the block
          it acts on) and gives it `border-t border-border pt-4` with the
          heading and the button on ONE line. That is the shape on the tab the
          user called fine, so this takes it rather than inventing a third.

          The sentence moves BELOW that line for the same reason `Config`'s
          validation result sits below its own: the line is the action, and the
          prose is what the action produced or explains. */}
      <div className="space-y-2 border-t border-border pt-4">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-medium">OpenAPI document</h3>
          {/* The only control in this block, and a plain `<button>`, so the
              shell's `<fieldset disabled={trashed}>` reaches it — unlike the
              `window` keydown listener §11.1 records. It opens a dialog and
              writes nothing, so it needs no `trashed` check of its own.

              `size="sm"` beside a `text-sm` heading, exactly as `Config`'s
              `Validate` button is: a default-height button on a heading line
              sets the line's height from the control rather than from the
              heading. */}
          <Button type="button" variant="outline" size="sm" onClick={() => setDownloadOpen(true)}>
            Download OpenAPI
          </Button>
          {baseUrl && <span className="font-mono text-xs text-muted-foreground">{baseUrl}</span>}
        </div>
        {/* `max-w-prose` is the one class the §4.2 conversion ADDED, and it
            replaced something the card had been doing rather than changing the
            block: the `CardContent`'s `p-6` was what stopped this sentence
            short. Free of the frame it would run the section's whole width —
            now the page's — where the Details column above caps at exactly
            this measure (`detailsClassName="max-w-prose"`) for the same
            reason. Prose is the section's one exception to a single measure;
            see the note on the root element. */}
        <p className="max-w-prose text-sm text-muted-foreground">
          Generated per service account — the document lists exactly the endpoints that account may
          invoke. Choose one to download this endpoint&apos;s contract.
        </p>
        {oaQuery.isError && (
          <Alert variant="destructive">
            <AlertDescription>
              {oaError?.status === 409
                ? oaError.message
                : "Could not generate a preview of the served request."}
            </AlertDescription>
          </Alert>
        )}
      </div>

      <DownloadOpenApiDialog ws={ws} open={downloadOpen} onOpenChange={setDownloadOpen} endpointId={id} />
    </div>
  );
}
