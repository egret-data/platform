import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
import {
  useDataApiGetOpenapi,
  useDataApiListEndpointVersions,
  useDataApiListServiceAccounts,
} from "@/api/generated/data-api/data-api";
import { useEndpointDetail } from "../EndpointDetail";
import { DownloadOpenApiDialog } from "../DownloadOpenApiDialog";

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
 * `columns` of the LIVE version (never the draft — a draft can differ from
 * what is currently served, and this section exists to show the real
 * contract), the `output.shape`/`naming` controls that decide how those
 * columns are keyed in the response body, and the served base URL plus the
 * per-service-account OpenAPI download.
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
 * The `output` controls are unchanged: a `Setting | Details | Value` table
 * whose "Output shape" heading is a `ConfigTableGroup` row.
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
 * spend width in the one table §5.1 measured down to `max-w-xl` *because* the
 * type had drifted away from the name it describes. Marking only the exceptions
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

export default function ResponseSection() {
  const { ws, id, endpoint, form, setForm } = useEndpointDetail();

  const versionsQuery = useDataApiListEndpointVersions(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const versionsEnvelope = versionsQuery.data;
  const versions = versionsEnvelope?.status === 200 ? versionsEnvelope.data : [];
  const liveVersion = versions.find((v) => v.is_live);
  // Only the LIVE version's columns, same as the table below: the caveat
  // explains a marker, so it must be computed from the rows that carry it.
  const hasOptionalColumn = !!liveVersion?.snapshot.columns.some(mayBeAbsent);
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

  return (
    // Capped like `Access`, `Try it` and `Versions`, and for the same reason:
    // removing the route cap moved width into the sections, and this section
    // was measured in neither the pass that built it (there was no cap to
    // remove yet) nor the pass that measured Config and the tabs it rebuilt.
    // Uncapped at a ~1470px content box, `table-fixed` gives Setting 288 /
    // Value 208 / Details ~975 while `max-w-prose` stops the sentence near
    // 585px — ~390px of empty column, with the two Selects sitting ~1260px
    // right of the labels they answer to. `max-w-5xl` puts them back at
    // ~816px, and it is the same cap the three sibling sections already use.
    <div className="max-w-5xl space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Response</h2>
        <p className="text-sm text-muted-foreground">
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
            down: the table is capped at `max-w-xl` (§5.1) for a reason about
            two short cells, and prose held to that measure would run
            unnecessarily tall. */}
        {hasOptionalColumn && (
          <p className="max-w-prose text-sm text-muted-foreground">
            Columns marked <span className="font-medium">May be absent</span> come from a branch that
            only some parameter values reach — on the calls that skip it the key is missing from the
            row entirely, never null. Every other column is in every response.
          </p>
        )}
        {/* Two short columns — a column name and a type. Left at the shared
            `Table`'s `w-full`, `bigint` sat a screen-width away from the column
            it describes even inside the section cap, so the table keeps the
            `max-w-xl` it was given in the width pass (§5.1). Auto layout, not
            `table-fixed`: nothing here truncates, and the state rows below are
            sentences that want to wrap rather than scroll. */}
        <Table className="max-w-xl">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Type</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {/* The four states are rows spanning the table rather than a branch
                that replaces it (§4.3), so `Name`/`Type` are on screen in every
                one of them — with the card gone there is otherwise nothing left
                to say what this list even records. `whitespace-normal` because
                the shared `TableCell` is `whitespace-nowrap` and these are
                sentences. */}
            {columnsLoading ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={2} className="whitespace-normal text-sm text-muted-foreground">
                  Loading…
                </TableCell>
              </TableRow>
            ) : columnsErrored ? (
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
            ) : !endpoint.live_version_id || !liveVersion ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={2} className="whitespace-normal text-sm text-muted-foreground">
                  This endpoint has no live version yet. Publish it to see the columns a consumer will
                  receive — these come from the published contract, not the draft.
                </TableCell>
              </TableRow>
            ) : (
              liveVersion.snapshot.columns.map((c) => (
                <TableRow key={c.name}>
                  {/* The badge marks the EXCEPTION, and rides in the Name cell
                      rather than in a `Required` column of its own. §4.3: a
                      column whose every cell is identical is section-level copy
                      in disguise — and every endpoint published before the
                      branching-columns work, plus every endpoint whose
                      definition does not branch, has every column guaranteed,
                      so a third column would hold one repeated word in every
                      cell of the common case while costing width in the one
                      table §5.1 measured down to `max-w-xl` precisely because
                      the type had drifted away from the name it describes.

                      Text, not a colour or an icon: it has to be legible
                      without a hover and to a reader who is not reading
                      visually. The Name cell keeps its own `font-mono text-xs`
                      on the name alone — the badge's label is prose. */}
                  <TableCell className="py-2">
                    <span className="flex items-center gap-2">
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
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <ConfigTable detailsClassName="max-w-prose">
        <ConfigTableGroup label="Output shape" />
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
      </ConfigTable>

      <div className="space-y-2">
        <h3 className="text-sm font-medium">OpenAPI document</h3>
        {/* `max-w-prose` is the one class this conversion ADDED, and it
            replaces something the card was doing rather than changing the
            block: the `CardContent`'s `p-6` inside a card inside the section
            cap was what stopped this sentence short of ~816px. Free of the
            frame it would run the full `max-w-5xl`, wider than the Details
            column two blocks up — which caps at exactly this measure
            (`detailsClassName="max-w-prose"`) for the same reason. */}
        <p className="max-w-prose text-sm text-muted-foreground">
          Generated per service account — the document lists exactly the endpoints that account may
          invoke. Choose one to download this endpoint&apos;s contract.
        </p>
        <div className="flex flex-wrap items-center gap-3">
          {/* The only control in this block, and a plain `<button>`, so the
              shell's `<fieldset disabled={trashed}>` reaches it — unlike the
              `window` keydown listener §11.1 records. It opens a dialog and
              writes nothing, so it needs no `trashed` check of its own. */}
          <Button type="button" variant="outline" onClick={() => setDownloadOpen(true)}>
            Download OpenAPI
          </Button>
          {baseUrl && <span className="font-mono text-xs text-muted-foreground">{baseUrl}</span>}
        </div>
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
