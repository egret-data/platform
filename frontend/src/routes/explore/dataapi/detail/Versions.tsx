import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { CHIP_TONE } from "../chipTone";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import {
  useDataApiListEndpointVersions,
  useDataApiValidateEndpoint,
} from "@/api/generated/data-api/data-api";
// Publish/rollback move `state` and `live_version_id`, so their invalidation
// spans three queries (this history, the endpoint record, and the list badge).
// That lives in the wrapper — see `src/api/dataApi.ts`.
import { usePublishEndpoint, useRollbackEndpoint } from "@/api/dataApi";
import type { ContractChange, DataApiEndpointVersionDto } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/**
 * `Versions` — the publish lifecycle (spec §6.3). Serving reads ONLY the
 * frozen snapshot the live version points at, never the draft — this section
 * is the one place that freezing happens (Publish) and the one place it can
 * be undone (Rollback), so it is also the moment egret can put the §5.3
 * warning in front of an author who may never open Access: publishing makes
 * a contract callable, it does not make the underlying data readable.
 *
 * **A plain `Table`, not a `Card` and not a `ConfigTable`**
 * (`docs/en/ui-conventions.md` §4). The history is LIST-shaped — one row per
 * frozen snapshot, no Setting/Value pair anywhere in it — so `ConfigTable`
 * would have to fabricate a Value column for a read-only listing, which §4.2
 * records as the reason `ResponseSection`'s frozen columns stayed out of one.
 * The `Card` that used to frame it went for the reason §4.1 gives: a card
 * around a table of one or two rows is a mostly-empty box, and this table is
 * empty on every endpoint that has never been published.
 */
export default function VersionsSection() {
  // `dirty` is read here, not just in Query: the five sections share ONE
  // form that survives section navigation (`EndpointDetail` keys the Outlet by
  // id, not by pathname), so an author can edit SQL in Query, walk over
  // here with "Unsaved changes" still in the header, and publish — freezing
  // the SAVED draft, not the one on screen. Config already guards the far
  // cheaper Validate this way; Publish changes what every deployed consumer
  // receives and has more reason to.
  const { ws, id, dirty, trashed, refetch } = useEndpointDetail();

  const versionsQuery = useDataApiListEndpointVersions(ws, id, {
    query: { enabled: !!ws && !!id },
  });
  const versionsEnvelope = versionsQuery.data;
  const versions = versionsEnvelope?.status === 200 ? versionsEnvelope.data : [];
  // Newest first — `is_live` (server-computed against `live_version_id`, not
  // re-derived here) is what actually marks the served version, this is only
  // display order.
  const sorted = [...versions].sort((a, b) => b.version - a.version);

  const publishMutation = usePublishEndpoint();
  const rollbackMutation = useRollbackEndpoint();

  const [publishOpen, setPublishOpen] = useState(false);
  const [rollbackTarget, setRollbackTarget] = useState<DataApiEndpointVersionDto | null>(null);

  return (
    // Uncapped, like every other tab on this route (§5.1, as reversed). This
    // used to carry `max-w-5xl` on the argument that the history table is five
    // short columns — `v3`, a truncated note, an email, a relative time, one
    // button — and reads as a mostly-empty band at monitor width. The argument
    // still describes this table correctly. What outweighed it is the tab set:
    // at 1600×1000 in a 1296px content box, Versions stopped at 1024 while
    // Config, Query and Response took all 1296, so switching tabs moved the
    // content edge 272px for a reason no reader could see. One measure across
    // the six beats a per-section optimum.
    //
    // Where the 272px actually go, checked and not assumed (§5.2 — the table
    // below is `table-fixed`): `Note` is the only unsized column, so all of it
    // lands there.
    //
    // # The row-action half, which this task closes
    //
    // The old comment ended "`Actions` is `w-32` and right-aligned, so Roll
    // back is now 272px further from the version it acts on", accepted and not
    // fixed. Measured in real Chromium at 1600×1000 against a five-version
    // fixture, with the cap put BACK as a negative control, that number is
    // exactly right and it is measuring the wrong thing:
    //
    //   capped  (1024px)  columns 128/416/224/128/128   Roll back at x=1198
    //   uncapped(1296px)  columns 128/688/224/128/128   Roll back at x=1470
    //
    // 272px, reproduced. But the distance that a reader crosses is from the
    // row's last TEXT to its action, and that is **83px** here, in both
    // states — and it is STRUCTURALLY FORCED, not merely measured. `When` is a
    // fixed `w-32` column immediately left of a `text-right` action in a
    // `table-fixed` table, so the gap is `158 − (the width of "a month ago")`
    // at ANY viewport: 128 of `When` plus the action cell's padding and the
    // button's inset, none of which depends on the section's width. The cap
    // could not have improved it and removing the cap could not have worsened
    // it. (`Access`, whose equivalent regression §5.1 records as fixed,
    // measured ~643px on the same metric — from a one-account stub, so take
    // the shape and not the number. This table was never the one with the
    // hole.)
    //
    // So this half of §5.1's mitigation is WITHDRAWN, not fixed: `Roll back`
    // is at x=1470 before and after. The complaint was mis-specified.
    //
    // What the 272px actually did is put them all into `Note`, which on a row
    // with no note is 680px of blank in the MIDDLE of the row, while `Who` was
    // cutting realistic addresses at 224px on two of five rows. So the
    // instrument is the one `Access` used — size the row table's own columns so
    // the width lands where content wants it — and it is applied to `Who`
    // below, not to `Actions`.
    //
    // Two things deliberately NOT done, both measured rather than assumed:
    //
    // - Widening `Actions` moves nothing. The cell is `text-right` and the
    //   button is 82px, so its left edge is `columnRight − 8 − 82` at any
    //   column width; `w-32` → `w-52` leaves it at x=1470.
    // - Sizing all five columns does not stop `Note` hoarding. Under
    //   `table-layout: fixed` a shortfall is shared PROPORTIONALLY, measured in
    //   Chromium: 128/512/288/128/128 in a 1296px box lays out as
    //   140/553/312/140/140. `Note` stays the unsized remainder instead.
    //
    // The residual — a right-aligned action in a full-width table sits at the
    // page's right edge — is not a column-sizing problem and is shared with
    // every other action column on this route. Left-aligning this one would
    // fix the arithmetic and make `Versions` the only tab in the set that
    // aligns its action differently, which is the defect §5.1 was reversed to
    // remove, in miniature.
    <div className="space-y-6">
      {/* The same heading block as THREE of the five sibling sections — a bare
          `<div>` holding the `h2` and one capped paragraph, as `Config`,
          `Response` and `Access` do. This comment used to say "the five", and
          it was wrong: `Query.tsx:403` and `TryIt.tsx:305` put the intro
          BESIDE the `h2` in a `flex … items-baseline` row, because both hand
          the rest of their height to a sized child and cannot afford two
          stacked lines. Four of six is still the majority shape and this row
          belongs in it; the count is corrected rather than the layout.

          Worth knowing when changing either shape: **no width spec can tell
          them apart.** They all resolve the intro as `h2`'s sibling `<p>` and
          measure left and right edges, which are identical in both — so a
          section silently switching shapes is a review question, not a test
          failure. This one used to wrap that
          in a `flex … justify-between` row with a filled primary `Publish`
          pinned to its right — directly under the shell's own `Save`, so the
          page's top-right corner carried two primary-looking buttons doing
          unrelated things, on the one tab of six that did that.

          Publish moved to the `History` heading line instead, which is
          `Config.tsx`'s `Validate` shape and is where it belongs on its own
          merits: what Publish does is add a row to that table. */}
      <div>
        <h2 className="text-lg font-semibold">Versions</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          Every frozen snapshot of this endpoint&apos;s contract. A consumer only ever receives the one
          marked Live — never the draft sitting in Query.
        </p>
      </div>

      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-medium">History</h3>
          {/* `trashed` beside `dirty`, and stated on screen rather than only in
              a `title`. Publishing a trashed endpoint is a 409 naming restore
              (`lifecycle.rs`'s guard), so an enabled button here can only spend
              a round trip to produce a red toast — and for a trashed DISABLED
              endpoint the pre-guard behaviour was worse than that: the publish
              SUCCEEDED and froze a new version into the trash. The shell's
              `fieldset` would disable this button on its own; the explicit
              check is what puts the reason next to it.

              `size="sm"` on the heading line, like `Validate`; still the
              default (filled) variant, unlike `Validate`, because this one IS
              this tab's primary action — the shape is what unifies, not the
              weight. */}
          <Button
            type="button"
            size="sm"
            onClick={() => setPublishOpen(true)}
            disabled={dirty || trashed}
            title={dirty && !trashed ? "Save your changes before publishing — Publish freezes the saved draft." : undefined}
          >
            Publish saved draft
          </Button>
          {trashed && <span className="text-xs text-muted-foreground">Restore it first</span>}
        </div>
        {/* `table-fixed`, so the Note column's `truncate` is real: under the
            default auto layout a `max-w` on a cell is advisory and a long note
            simply widens the table instead of ellipsing. The sized columns do
            not shrink below their widths on a narrow screen — the shared
            `Table` wrapper scrolls instead (§5.3), which is the accepted
            trade the `ConfigTable` columns already make. */}
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              {/* Version and its Live badge share one column rather than the
                  badge getting a Status column of its own. `v{n}` is monospace
                  and at most three characters, so the badge sits at a constant
                  x anyway — and a Status column would be empty on every row but
                  one, which is §4.1's complaint about mostly-empty boxes
                  applied to a column instead of a card. */}
              {/* KI-024: at `w-32` the Version cell is 112px of content, and
                  `v2` + `Live` + `1 breaking change` lay out at about 200px, so
                  under `table-fixed` + the shared cell's `whitespace-nowrap` the
                  badge ran on top of the Note text. The cell now WRAPS its chips
                  (below), which is what holds at every width, and the column
                  takes a little more room — the whole run at `xl`, where `Note`
                  can spare it. */}
              <TableHead className="w-40 xl:w-56">Version</TableHead>
              <TableHead>Note</TableHead>
              {/* §5.1's last open mitigation, and the same instrument
                  `AccessSection` used: the freed width goes to the column that
                  is CUTTING its content, not to the one that merely has room.
                  Measured in Chromium at 1600×1000 with five versions, this
                  column laid out `priyanka.venkataraman@egret.local` (33
                  characters) at 231px against 224px of column and truncated it
                  on two of the five rows, while `Note` was being handed 688.
                  `w-80` is 320px, which holds the realistic band; a note is
                  unbounded prose that no finite width fixes, and it keeps the
                  `title` fallback it already had.

                  Gated at `xl` for §5.3's reason: `table-fixed` scrolls rather
                  than shrinks, so an ungated widening costs the narrow regime
                  64px of horizontal scroll to fix a wide-screen complaint. At
                  `xl` the content box is 976px and the four sized columns take
                  704, leaving `Note` 272. */}
              <TableHead className="w-56 xl:w-80">Who</TableHead>
              <TableHead className="w-32">When</TableHead>
              {/* Named for a screen reader, invisible on screen: a visible
                  "Actions" heading over a column of one small button reads as
                  louder than what it labels. */}
              <TableHead className="w-32 text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {/* The three states are rows spanning the table rather than a
                branch that replaces it, so the header stays put and the shape
                of the list is legible before the first publish. `ParamEditor`
                and `TryIt`'s parameter table use the same idiom.
                `whitespace-normal` because the shared `TableCell` is
                `whitespace-nowrap` and these are sentences, not values. */}
            {versionsQuery.isLoading ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="whitespace-normal text-sm text-muted-foreground">
                  Loading…
                </TableCell>
              </TableRow>
            ) : versionsQuery.isError ? (
              // Checked BEFORE the empty branch: react-query clears
              // `isLoading` on any settle, error included, so a failed fetch
              // reaching the empty branch would report a network failure as
              // "never published" (§2.1/§2.2).
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="whitespace-normal">
                  <Alert variant="destructive">
                    <AlertDescription>
                      Could not load versions.{" "}
                      {(versionsQuery.error as unknown as Error)?.message ?? "Try again in a moment."}
                    </AlertDescription>
                  </Alert>
                </TableCell>
              </TableRow>
            ) : sorted.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={5} className="whitespace-normal text-sm text-muted-foreground">
                  This endpoint has never been published. Publish it to freeze the current draft as
                  version 1 — that is what a consumer will start receiving.
                </TableCell>
              </TableRow>
            ) : (
              sorted.map((v) => (
                <TableRow key={v.id} className="align-top" data-testid={`version-row-${v.id}`}>
                  <TableCell className="py-2 whitespace-normal">
                    <span className="flex flex-wrap items-center gap-1.5">
                      <span className="font-mono text-xs">v{v.version}</span>
                      {/* The same shared tone the endpoint list and the editor
                          pill wear (`chipTone.ts`), so this `Live` and the one
                          a click away are the same chip. Not `variant="success"`:
                          that resolves to a hardcoded `bg-green-700`, not the
                          OKLCH `--success` token. The ink is `--foreground` for
                          the reason recorded in `chipTone.ts` — `text-success`
                          on this wash measures 3.72:1 in light mode, and the
                          word "Live" is what identifies the row anyway. */}
                      {v.is_live && <Badge className={CHIP_TONE.success}>Live</Badge>}
                      {/* What this version broke when it was published, and
                          therefore what the author in the next column accepted
                          (package 6). Rendered only when the list is non-empty:
                          `null` — a version published before the comparison
                          existed, or a first version, where there was nothing
                          to compare — and `[]` — compared, broke nothing — are
                          different facts, and neither of them is a chip.
                          Amber, not destructive: a version that broke its
                          predecessor is a deliberate act with a record, not a
                          fault. Same tone the diverged endpoint pill wears. */}
                      {(v.breaking_changes?.length ?? 0) > 0 && (
                        <Badge
                          className={CHIP_TONE.highlight}
                          title={v.breaking_changes
                            ?.map((c) => `${c.subject}: ${c.detail}`)
                            .join("\n")}
                          data-testid={`version-breaking-${v.id}`}
                        >
                          {v.breaking_changes?.length === 1
                            ? "1 breaking change"
                            : `${v.breaking_changes?.length} breaking changes`}
                        </Badge>
                      )}
                      {/* The baseline, shown ONLY when it is not the version
                          before this one — which is the case a rollback
                          creates and the only case where the chip above (or
                          its absence) says less than it appears to.

                          `rollback` moves the live pointer backwards and
                          writes no version row, so a publish afterwards
                          compares against the restored version, can truthfully
                          find nothing, and record `[]`. With no chip at all,
                          this row reads as "publishing this broke nobody" —
                          true of the consumers of the baseline, and silent
                          about the consumers of the version the rollback moved
                          them off. `neutral`: it is not a fault, it is the
                          missing half of a sentence. */}
                      {v.compared_against_version != null &&
                        v.compared_against_version !== v.version - 1 && (
                          <Badge
                            className={CHIP_TONE.neutral}
                            title={`The comparison that produced this row's verdict was made against version ${v.compared_against_version}, not version ${v.version - 1} — the live pointer had been rolled back. It says nothing about consumers of the versions in between.`}
                            data-testid={`version-baseline-${v.id}`}
                          >
                            {`compared against v${v.compared_against_version}`}
                          </Badge>
                        )}
                    </span>
                  </TableCell>
                  <TableCell className="truncate py-2 text-muted-foreground" title={v.note || undefined}>
                    {v.note || "—"}
                  </TableCell>
                  {/* `title`, like the `Note` cell above — and it is not
                      symmetry. This cell is `truncate`, and the `xl` gate on
                      its column means that BELOW `xl` it is exactly the 224px
                      column that was cutting realistic addresses. Without a
                      `title` there is no route at all to the clipped value in
                      the regime the widening deliberately does not reach: the
                      defect the fix is named after, left whole wherever the
                      fix does not land. `Access` makes the same call for the
                      one address its own widened column still cuts.

                      Computed once so the attribute and the text cannot
                      drift — the fallback chain is three-deep and a second
                      copy of it in a `title=` is a copy that will be edited
                      alone. `undefined` for the em-dash placeholder: a
                      tooltip reading "—" is noise. */}
                  {(() => {
                    const who =
                      v.created_by_name ?? v.created_by?.slice(0, 8) ?? null;
                    return (
                      <TableCell className="truncate py-2 text-muted-foreground" title={who ?? undefined}>
                        {who ?? "—"}
                      </TableCell>
                    );
                  })()}
                  <TableCell className="py-2 text-muted-foreground">
                    <RelativeTime iso={v.created_at} />
                  </TableCell>
                  <TableCell className="py-2 text-right">
                    {/* Same argument as Publish above: rollback writes
                        `live_version_id` and `state`, which a trashed row
                        refuses with a 409. Shown-but-disabled rather than
                        hidden, so the history stays readable and the reason
                        for the missing action is the banner at the top of the
                        page rather than a blank cell. */}
                    {!v.is_live && (
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        disabled={trashed}
                        onClick={() => setRollbackTarget(v)}
                      >
                        Roll back
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <PublishDialog
        ws={ws}
        id={id}
        liveVersion={versions.find((v) => v.is_live)?.version}
        open={publishOpen}
        onOpenChange={setPublishOpen}
        publish={publishMutation}
        onPublished={refetch}
      />

      {/* Rollback changes what every deployed client receives, so it uses the
          same type-to-confirm idiom the codebase already uses for other
          irreversible, wide-blast-radius actions (e.g. revoking a warehouse
          client) rather than a bare button. */}
      <ConfirmDeleteDialog
        open={rollbackTarget !== null}
        onOpenChange={(o) => {
          if (!o) setRollbackTarget(null);
        }}
        title={rollbackTarget ? `Roll back to v${rollbackTarget.version}?` : "Roll back?"}
        description="Every deployed client calling this endpoint's URL will immediately start receiving this version's frozen contract instead of the current live one. This does not delete anything — publish again to move forward once you're ready."
        confirmText="rollback"
        confirmLabel="Roll back"
        onConfirm={async () => {
          if (!rollbackTarget) return;
          try {
            const res = await rollbackMutation.mutateAsync({
              workspace: ws,
              id,
              data: { version_id: rollbackTarget.id },
            });
            if (res.status !== 200) {
              toast.error("Could not roll back.");
              return;
            }
            refetch();
            toast.success(`Rolled back to v${res.data.version}`);
          } catch (e) {
            toast.error(String(e));
          }
        }}
      />
    </div>
  );
}

/**
 * The publish confirmation. Carries the §5.3 warning (spec §6.3, second of
 * its three placements) — this is the moment BEFORE the endpoint goes live,
 * and the one place the warning reaches an author who never opens Access. It
 * has to sit here, not as a toast fired after `mutateAsync` resolves: a toast
 * arrives after the fact, once the endpoint may already be live.
 *
 * The note stays a `<Textarea rows={2}>`. It is PROSE — "what changed" for the
 * next reader of the history table — not code, so none of `Query`'s editor
 * treatment applies to it.
 *
 * # The pre-flight, and what it is NOT
 *
 * Opening this dialog runs `POST …/validate`, which since package 6 also
 * answers "what would publishing this do to anyone already calling it". The
 * breaking changes it reports are shown here, and Publish will not send until
 * the author has ticked a box saying they have read them.
 *
 * **That checkbox is not the gate.** The gate is the server's: `publish`
 * recomputes the diff against the live snapshot under the endpoint's row lock
 * and refuses without the acknowledgement, so a caller that never opens this
 * dialog — curl, a deploy script, a future console — is refused exactly the
 * same way. What this surface adds is the reading. If the pre-flight itself
 * fails, Publish therefore stays ENABLED and the failure is shown: a client
 * that can disable the publish button cannot be allowed to enable it either,
 * and a flaky check must not be able to block a publish the server would
 * accept.
 *
 * The acknowledgement is the server's own digest of those specific changes,
 * echoed back untouched. Nothing here decides what is breaking — that judgement
 * has one home (`egret_dataapi::contract`), and a second opinion computed in
 * TypeScript is the drift this programme keeps closing.
 */
function PublishDialog({
  ws,
  id,
  liveVersion,
  open,
  onOpenChange,
  publish,
  onPublished,
}: {
  ws: string;
  id: string;
  /** The version consumers are receiving, when there is one. */
  liveVersion?: number;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  publish: ReturnType<typeof usePublishEndpoint>;
  onPublished: () => void;
}) {
  const [note, setNote] = useState("");
  // The refusal, kept on screen until the author acts on it. See `submit`.
  const [refusal, setRefusal] = useState<string | null>(null);
  const [preflight, setPreflight] = useState<Preflight>({ phase: "checking" });
  const [acknowledged, setAcknowledged] = useState(false);
  const validateMutation = useDataApiValidateEndpoint();

  // Cleared on CLOSE, not on open: the dialog instance persists across opens
  // (it is not remounted), so a note left over from a previous publish must
  // not reappear pre-filled the next time this is opened. Clearing here
  // rather than in a `useEffect` keyed on `open` avoids a synchronous
  // setState-in-effect (and reaches every way the dialog can close — Cancel,
  // Escape, overlay click, and a successful submit — since they all funnel
  // through Radix's `onOpenChange`).
  //
  // `acknowledged` is cleared here too, and that is the load-bearing half of
  // it: a tick left over from a previous open would be an acknowledgement of
  // changes the author is no longer looking at. The token cannot be replayed
  // — the server binds it to the exact break set — so the failure would be a
  // refusal rather than a silent break, but a box that arrives already ticked
  // is not an acknowledgement of anything.
  const handleOpenChange = (o: boolean) => {
    if (!o) {
      setNote("");
      setRefusal(null);
      setAcknowledged(false);
      // And the pre-flight, back to its initial value rather than left holding
      // the previous open's answer. Resetting HERE rather than at the top of
      // the effect below is what keeps that effect free of a synchronous
      // setState — the same placement, for the same reason, as the note.
      setPreflight({ phase: "checking" });
    }
    onOpenChange(o);
  };

  // The pre-flight. An effect keyed on `open` because the dialog is opened by
  // the parent's state and not through Radix's `onOpenChange`, so there is no
  // callback to hang it on.
  //
  // Nothing in the body sets state synchronously — the reset lives in
  // `handleOpenChange` above — so what is left here is one asynchronous call
  // whose answer lands in a callback. That is what an effect is for, and it is
  // also what keeps `react-hooks/set-state-in-effect` satisfied without a
  // suppression comment.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const res = await validateMutation.mutateAsync({ workspace: ws, id });
        if (cancelled) return;
        if (res.status !== 200) {
          setPreflight({ phase: "failed", message: "Could not check this draft." });
          return;
        }
        const diff = res.data.against_live ?? null;
        setPreflight({
          phase: "ready",
          breaking: diff?.breaking ?? [],
          safeCount: diff?.safe?.length ?? 0,
          token: res.data.acknowledge_breaking ?? null,
          comparedAgainstLive: diff !== null,
        });
      } catch (e) {
        if (cancelled) return;
        setPreflight({
          phase: "failed",
          message: e instanceof Error ? e.message : String(e),
        });
      }
    })();
    return () => {
      cancelled = true;
    };
    // `validateMutation` is a fresh object every render; depending on it would
    // re-run the pre-flight forever.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, ws, id]);

  const breaking = preflight.phase === "ready" ? preflight.breaking : [];
  const blocked = breaking.length > 0 && !acknowledged;

  async function submit() {
    setRefusal(null);
    try {
      const trimmed = note.trim();
      const token = preflight.phase === "ready" ? preflight.token : null;
      const res = await publish.mutateAsync({
        workspace: ws,
        id,
        data: {
          ...(trimmed ? { note: trimmed } : {}),
          // Only when there is something to acknowledge. The server refuses a
          // token sent against a publish that breaks nothing, deliberately —
          // see `PublishEndpointInput` — so sending one unconditionally would
          // turn every safe publish into a 409.
          ...(breaking.length > 0 && token ? { acknowledge_breaking: token } : {}),
        },
      });
      if (res.status !== 200) {
        setRefusal("Could not publish.");
        return;
      }
      handleOpenChange(false);
      onPublished();
      toast.success(`Published v${res.data.version}`);
    } catch (e) {
      // `e.message`, not `String(e)`: `ApiError` (api/client.ts) carries the
      // body's `error` sentence as its message, and `String(e)` prefixes it
      // with a bare `Error: ` that says nothing.
      setRefusal(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Publish this endpoint</DialogTitle>
        </DialogHeader>
        <div className="space-y-3">
          <p className="text-sm text-muted-foreground">
            Freezes the saved draft as a new version and makes it what every consumer receives, starting
            immediately. (Publish is unavailable while there are unsaved changes, so what is frozen is
            always what you last saved.)
          </p>
          <SettingsNote>
            Publishing does not grant data access. A call runs as the service account against the workspace
            connection, under whatever the engine enforces — so an admin must also grant that service
            account on the underlying tables in Ranger, or this contract has nothing to serve.
          </SettingsNote>

          {preflight.phase === "checking" && (
            <p className="text-sm text-muted-foreground" data-testid="publish-preflight-checking">
              Checking what this changes for anyone already calling it…
            </p>
          )}
          {/* A warning, not a block. See the component comment: the refusal
              lives on the server, so a failed check here must not be able to
              stop a publish the server would accept. */}
          {preflight.phase === "failed" && (
            <Alert variant="destructive">
              <AlertDescription>
                Could not check what this changes for existing callers: {preflight.message}. Publishing
                is still refused by the server if it breaks them.
              </AlertDescription>
            </Alert>
          )}
          {preflight.phase === "ready" && breaking.length > 0 && (
            <Alert variant="destructive" data-testid="publish-breaking">
              <AlertDescription>
                <p className="font-medium">
                  {breaking.length === 1
                    ? "This breaks callers of the version being served"
                    : `This breaks callers of the version being served, in ${breaking.length} ways`}
                  {liveVersion == null ? "" : ` (v${liveVersion})`}:
                </p>
                <ul className="mt-2 list-disc space-y-1 pl-5">
                  {breaking.map((c, i) => (
                    <li key={`${c.kind}-${c.subject}-${i}`}>
                      <span className="font-medium">{c.subject}</span> — {c.detail}
                    </li>
                  ))}
                </ul>
              </AlertDescription>
            </Alert>
          )}
          {/* Compared and found nothing is a different message from not asking,
              and saying so is what stops the warning above reading as noise the
              one time it appears. */}
          {preflight.phase === "ready" && breaking.length === 0 && preflight.comparedAgainstLive && (
            <p className="text-sm text-muted-foreground" data-testid="publish-preflight-clean">
              {preflight.safeCount === 0
                ? "Nothing a caller can see changes."
                : `${preflight.safeCount} change${preflight.safeCount === 1 ? "" : "s"} compared against the served version; none of them breaks a caller.`}
            </p>
          )}
          {preflight.phase === "ready" && !preflight.comparedAgainstLive && (
            <p className="text-sm text-muted-foreground" data-testid="publish-preflight-first">
              Nothing is serving this endpoint yet, so there is nothing to break.
            </p>
          )}

          {/* Inline and persistent, not a toast — the same rule `Config`'s
              Validate block states, for the same class of message and the same
              reader. Measured in Chrome on this branch before this changed: a
              `default` of `5` against a `minimum` of `10` answered with a
              bottom-right toast that lived 4.3 s and could not be paused,
              because the publish dialog stays open on a refusal and its
              overlay (`z-50`, `data-slot="dialog-overlay"`) takes the pointer
              — `elementFromPoint` at the toast's own centre resolved to the
              overlay, so sonner's pause-on-hover could never fire. The author
              was left looking at the dialog with no trace of why it refused,
              and had to press Publish again to read the sentence. It is the
              refusal that names the parameter, the constraint and the bound,
              so it is the one message that must not expire. */}
          {/* A refusal here used to be a sentence; a transform's is sometimes a
              diagnostic. `probe_transform` reports the Starlark interpreter's
              own traceback for the probe that failed, and that is an ASCII
              diagram whose caret row has to sit under the token it points at:

                the empty-rows probe: … error: Index `0` is out of bound
                 --> transform.star:3:13
                  |
                3 |     first = rows[0]
                  |             ^^^^^^^

              Measured in Chrome on this branch before this changed: every
              newline collapsed, so the dialog showed
              `... transform.star:3:13 | 3 | first = rows[0] | ^^^^^^^ |` on
              one proportional-font line — the line and column survived, the
              alignment that makes a caret a caret did not.

              `font-mono` only when the message HAS a newline, deliberately.
              Every other refusal this dialog shows is prose that names a
              parameter and a bound, and setting the whole alert in monospace
              would make the common case worse to read to help the rare one.
              A multi-line refusal is a diagnostic; a single-line one is a
              sentence. Covered in a real browser by
              `tests/e2e/data-api-transform-refusal-legibility.spec.ts`. */}
          {refusal && (
            <Alert variant="destructive">
              <AlertDescription
                className={
                  refusal.includes("\n") ? "whitespace-pre-wrap font-mono text-xs" : undefined
                }
              >
                {refusal}
              </AlertDescription>
            </Alert>
          )}
          <div className="space-y-1.5">
            <Label htmlFor="publish-note">Note (optional)</Label>
            <Textarea
              id="publish-note"
              rows={2}
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="What changed…"
            />
          </div>
          {/* Rendered only when there is something to acknowledge, so it can
              never be a box an author learns to tick on every publish — which
              is the habit that would make it decoration. */}
          {breaking.length > 0 && (
            <label className="flex items-start gap-2 text-sm" htmlFor="publish-acknowledge">
              <Checkbox
                id="publish-acknowledge"
                checked={acknowledged}
                onCheckedChange={(v) => setAcknowledged(v === true)}
              />
              <span>
                I have read {breaking.length === 1 ? "this change" : "these changes"} and accept that
                deployed callers will break.
              </span>
            </label>
          )}
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={publish.isPending || blocked}>
            {publish.isPending ? "Publishing…" : "Publish"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * What the dialog managed to learn about what this publish would do to
 * existing callers.
 *
 * Three phases and no boolean: "we have not asked yet", "we asked and could
 * not find out" and "we asked" are three different facts, and the middle one
 * is the reason this is not `breaking: ContractChange[]` with `[]` as its
 * initial value. An empty list means "nothing breaks", which is exactly the
 * claim a failed check cannot support — the same distinction `DraftVsServed`
 * draws in `endpointState.tsx`, for the same reason.
 */
type Preflight =
  | { phase: "checking" }
  | { phase: "failed"; message: string }
  | {
      phase: "ready";
      breaking: ContractChange[];
      /** How many other changes were compared and judged survivable. */
      safeCount: number;
      /** The server's acknowledgement for exactly `breaking`. */
      token: string | null;
      /**
       * Whether there WAS a live version to compare against. `false` is a first
       * publish — no consumer has ever received anything — which is a different
       * fact from an empty diff and is worded differently.
       */
      comparedAgainstLive: boolean;
    };
