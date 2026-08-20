import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { listUsers } from "@/api/access";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
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
    // Capped here, not by the route. The history table is five short columns —
    // `v3`, a truncated note, an email, a relative time, one button. Uncapped
    // it is a mostly-empty band the width of the monitor with the Roll back
    // button marooned at the far right of the row it belongs to.
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

      <Card>
        <CardHeader>
          <CardTitle className="text-base">History</CardTitle>
        </CardHeader>
        <CardContent>
          {versionsQuery.isLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : versionsQuery.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                Could not load versions.{" "}
                {(versionsQuery.error as unknown as Error)?.message ?? "Try again in a moment."}
              </AlertDescription>
            </Alert>
          ) : sorted.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              This endpoint has never been published. Publish it to freeze the current draft as version 1
              — that is what a consumer will start receiving.
            </p>
          ) : (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <th className="pb-1 font-medium">Version</th>
                  <th className="pb-1 font-medium">Note</th>
                  <th className="pb-1 font-medium">Who</th>
                  <th className="pb-1 font-medium">When</th>
                  <th className="pb-1 font-medium" />
                </tr>
              </thead>
              <tbody>
                {sorted.map((v) => (
                  <tr key={v.id} className="border-t border-border align-top">
                    <td className="py-2 pr-2">
                      <span className="inline-flex items-center gap-1.5">
                        <span className="font-mono text-xs">v{v.version}</span>
                        {/* Same token-based className `Endpoints.tsx` uses for the
                            live state badge — `variant="success"` on `Badge`
                            resolves to a hardcoded `bg-green-600`, not the OKLCH
                            `--success` token, so it would be a different green
                            from the one this row sits one click away from. */}
                        {v.is_live && <Badge className="bg-success/15 text-success">Live</Badge>}
                      </span>
                    </td>
                    <td className="max-w-[16rem] truncate py-2 pr-2 text-muted-foreground">{v.note || "—"}</td>
                    <td className="py-2 pr-2 text-muted-foreground">
                      {(v.created_by && userEmailById.get(v.created_by)) ??
                        v.created_by?.slice(0, 8) ??
                        "—"}
                    </td>
                    <td className="py-2 pr-2 text-muted-foreground">
                      <RelativeTime iso={v.created_at} />
                    </td>
                    <td className="py-2 text-right">
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
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </CardContent>
      </Card>

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

  // Cleared on CLOSE, not on open: the dialog instance persists across opens
  // (it is not remounted), so a note left over from a previous publish must
  // not reappear pre-filled the next time this is opened. Clearing here
  // rather than in a `useEffect` keyed on `open` avoids a synchronous
  // setState-in-effect (and reaches every way the dialog can close — Cancel,
  // Escape, overlay click, and a successful submit — since they all funnel
  // through Radix's `onOpenChange`).
  const handleOpenChange = (o: boolean) => {
    if (!o) setNote("");
    onOpenChange(o);
  };

  async function submit() {
    try {
      const trimmed = note.trim();
      const res = await publish.mutateAsync({
        workspace: ws,
        id,
        data: trimmed ? { note: trimmed } : {},
      });
      if (res.status !== 200) {
        toast.error("Could not publish.");
        return;
      }
      handleOpenChange(false);
      onPublished();
      toast.success(`Published v${res.data.version}`);
    } catch (e) {
      toast.error(String(e));
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
