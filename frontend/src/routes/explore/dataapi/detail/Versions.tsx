import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { listUsers } from "@/api/access";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import { useDataApiListEndpointVersions } from "@/api/generated/data-api/data-api";
// Publish/rollback move `state` and `live_version_id`, so their invalidation
// spans three queries (this history, the endpoint record, and the list badge).
// That lives in the wrapper — see `src/api/dataApi.ts`.
import { usePublishEndpoint, useRollbackEndpoint } from "@/api/dataApi";
import type { DataApiEndpointVersionDto } from "@/api/generated/model";
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
  // `dirty` is read here, not just in Definition: the five sections share ONE
  // form that survives section navigation (`EndpointDetail` keys the Outlet by
  // id, not by pathname), so an author can edit SQL in Definition, walk over
  // here with "Unsaved changes" still in the header, and publish — freezing
  // the SAVED draft, not the one on screen. Definition already guards the far
  // cheaper Validate this way; Publish changes what every deployed consumer
  // receives and has more reason to.
  const { ws, id, dirty, trashed, refetch } = useEndpointDetail();

  // `created_by` on a version is a raw user id, not a name — the same
  // problem `ActivityCallList.tsx` solves for its trial-actor column
  // (whose own comment credits `AuditLog.tsx` as the original fix). Same
  // query key as that call site: both are the exact same unfiltered
  // `listUsers()` call, so they share the cache rather than each firing
  // it. `retry: false` matters here specifically — `listUsers` is a
  // platform-level call a viewer-level operator may not be permitted, and
  // this column must degrade to a truncated id rather than fail the page.
  const usersQuery = useQuery({
    queryKey: ["data-api", "users"],
    queryFn: listUsers,
    retry: false,
  });
  const userEmailById = useMemo(
    () => new Map((usersQuery.data ?? []).map((u) => [u.id, u.email])),
    [usersQuery.data],
  );

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
    // Capped here, not by the route (§5.1). The history table is five short
    // columns — `v3`, a truncated note, an email, a relative time, one button.
    // Uncapped it is a mostly-empty band the width of the monitor with the Roll
    // back button marooned at the far right of the row it belongs to.
    <div className="max-w-5xl space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Versions</h2>
          <p className="text-sm text-muted-foreground">
            Every frozen snapshot of this endpoint&apos;s contract. A consumer only ever receives the one
            marked Live — never the draft sitting in Definition.
          </p>
        </div>
        {/* `trashed` beside `dirty`, and stated on screen rather than only in a
            `title`. Publishing a trashed endpoint is a 409 naming restore
            (`lifecycle.rs`'s guard), so an enabled button here can only spend a
            round trip to produce a red toast — and for a trashed DISABLED
            endpoint the pre-guard behaviour was worse than that: the publish
            SUCCEEDED and froze a new version into the trash. The shell's
            `fieldset` would disable this button on its own; the explicit check
            is what puts the reason next to it. */}
        <div className="text-right">
          <Button
            type="button"
            onClick={() => setPublishOpen(true)}
            disabled={dirty || trashed}
            title={dirty && !trashed ? "Save your changes before publishing — Publish freezes the saved draft." : undefined}
          >
            Publish saved draft
          </Button>
          {trashed && <p className="mt-1 text-xs text-muted-foreground">Restore it first</p>}
        </div>
      </div>

      <div className="space-y-2">
        <h3 className="text-sm font-medium">History</h3>
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
              <TableHead className="w-32">Version</TableHead>
              <TableHead>Note</TableHead>
              <TableHead className="w-56">Who</TableHead>
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
                  <TableCell className="py-2">
                    <span className="inline-flex items-center gap-1.5">
                      <span className="font-mono text-xs">v{v.version}</span>
                      {/* Same token-based className `Endpoints.tsx` uses for the
                          live state badge — `variant="success"` on `Badge`
                          resolves to a hardcoded `bg-green-600`, not the OKLCH
                          `--success` token, so it would be a different green
                          from the one this row sits one click away from. */}
                      {v.is_live && <Badge className="bg-success/15 text-success">Live</Badge>}
                    </span>
                  </TableCell>
                  <TableCell className="truncate py-2 text-muted-foreground" title={v.note || undefined}>
                    {v.note || "—"}
                  </TableCell>
                  <TableCell className="truncate py-2 text-muted-foreground">
                    {(v.created_by && userEmailById.get(v.created_by)) ??
                      v.created_by?.slice(0, 8) ??
                      "—"}
                  </TableCell>
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
 */
function PublishDialog({
  ws,
  id,
  open,
  onOpenChange,
  publish,
  onPublished,
}: {
  ws: string;
  id: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  publish: ReturnType<typeof usePublishEndpoint>;
  onPublished: () => void;
}) {
  const [note, setNote] = useState("");
  // The refusal, kept on screen until the author acts on it. See `submit`.
  const [refusal, setRefusal] = useState<string | null>(null);

  // Cleared on CLOSE, not on open: the dialog instance persists across opens
  // (it is not remounted), so a note left over from a previous publish must
  // not reappear pre-filled the next time this is opened. Clearing here
  // rather than in a `useEffect` keyed on `open` avoids a synchronous
  // setState-in-effect (and reaches every way the dialog can close — Cancel,
  // Escape, overlay click, and a successful submit — since they all funnel
  // through Radix's `onOpenChange`).
  const handleOpenChange = (o: boolean) => {
    if (!o) {
      setNote("");
      setRefusal(null);
    }
    onOpenChange(o);
  };

  async function submit() {
    setRefusal(null);
    try {
      const trimmed = note.trim();
      const res = await publish.mutateAsync({
        workspace: ws,
        id,
        data: trimmed ? { note: trimmed } : {},
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
          {refusal && (
            <Alert variant="destructive">
              <AlertDescription>{refusal}</AlertDescription>
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
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={publish.isPending}>
            {publish.isPending ? "Publishing…" : "Publish"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
