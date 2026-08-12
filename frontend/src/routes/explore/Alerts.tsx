import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { AlertCircle, Bell, FileClock, Plus } from "lucide-react";
import {
  useScheduleMutations,
  useSchedules,
  type ScheduleSummary,
  type ScheduleType,
} from "@/api/alerts";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScheduleEditor } from "@/components/alerts/ScheduleEditor";
import { RunLogDrawer } from "@/components/alerts/RunLogDrawer";
import type { RunState } from "@/api/alerts";

/**
 * How each run state reads in the list.
 *
 * Superset's five states are kept verbatim because they are the vocabulary that
 * makes the page legible — `noop` and `grace` are the two distinct answers to
 * "why didn't I get an email?" ("it ran, the condition was false" vs "it would
 * have fired, the grace period suppressed it"), and showing either as a plain
 * success would lose the answer the page exists to give.
 */
const STATE_LABEL: Record<RunState, string> = {
  success: "Success",
  working: "Working",
  error: "Error",
  noop: "Not triggered",
  grace: "On grace",
};

const STATE_VARIANT: Record<RunState, "default" | "secondary" | "destructive" | "outline"> = {
  success: "default",
  working: "secondary",
  error: "destructive",
  noop: "outline",
  grace: "secondary",
};

function StateBadge({ state }: { state: RunState | null | undefined }) {
  if (!state) return <span className="text-sm text-muted-foreground">Never run</span>;
  return <Badge variant={STATE_VARIANT[state]}>{STATE_LABEL[state]}</Badge>;
}

export default function Alerts() {
  const ws = useParams().workspace!;
  const [tab, setTab] = useState<ScheduleType>("alert");
  const { data, isLoading, isError } = useSchedules(ws, tab);
  const { remove } = useScheduleMutations(ws);

  const [editing, setEditing] = useState<{ id: string | null } | null>(null);
  const [runLogFor, setRunLogFor] = useState<ScheduleSummary | null>(null);
  const [pendingDelete, setPendingDelete] = useState<ScheduleSummary | null>(null);

  const rows = useMemo(() => data ?? [], [data]);

  /**
   * Whether to offer the mutating affordances at all. Withheld rather than
   * disabled: a greyed Delete advertises a capability the reader does not have,
   * and these controls are destructive.
   *
   * It cannot be derived from the session — `me.permissions` carries PLATFORM
   * grants only, so workspace-scoped `alert:manage` never appears there. The
   * server answers on every row (the answer is workspace-wide, so row 0 is not
   * a shortcut). An empty list has no row to carry it; defaulting to `true`
   * there means a legitimate author in a fresh workspace can still press New
   * and, if wrong, gets a 403 — blocking real work would be the worse error.
   */
  const canManage = rows.length > 0 ? rows[0].can_manage !== false : true;

  const typeLabel = tab === "alert" ? "alert" : "report";

  return (
    <div className="flex flex-col gap-4 p-6">
      <header className="flex items-center justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Alerts &amp; Reports</h1>
          <p className="text-sm text-muted-foreground">
            Run a query on a schedule and get told when it matters, or deliver a saved
            dashboard on a timer.
          </p>
        </div>
        {canManage && (
          <Button onClick={() => setEditing({ id: null })}>
            <Plus className="mr-2 h-4 w-4" />
            New {typeLabel}
          </Button>
        )}
      </header>

      <Tabs value={tab} onValueChange={(v) => setTab(v as ScheduleType)}>
        <TabsList>
          <TabsTrigger value="alert">Alerts</TabsTrigger>
          <TabsTrigger value="report">Reports</TabsTrigger>
        </TabsList>
      </Tabs>

      {isError && (
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>Could not load schedules.</AlertDescription>
        </Alert>
      )}

      {isLoading ? (
        <ListSkeleton />
      ) : rows.length === 0 ? (
        <div className="flex flex-col items-center gap-2 rounded-lg border border-dashed p-12 text-center">
          <Bell className="h-8 w-8 text-muted-foreground" />
          <p className="font-medium">No {typeLabel}s yet</p>
          <p className="max-w-md text-sm text-muted-foreground">
            {tab === "alert"
              ? "An alert runs SQL on a schedule and notifies you when the result meets your condition."
              : "A report delivers a chart or dashboard on a schedule."}
          </p>
        </div>
      ) : (
        <table className="w-full text-sm">
          <thead className="border-b text-left text-muted-foreground">
            <tr>
              <th className="py-2 font-medium">Name</th>
              <th className="py-2 font-medium">Last run</th>
              <th className="py-2 font-medium">Schedule</th>
              <th className="py-2 font-medium">Next run</th>
              <th className="py-2 font-medium">Owners</th>
              <th className="py-2" />
            </tr>
          </thead>
          <tbody>
            {rows.map((s) => (
              <tr key={s.id} className="border-b last:border-0">
                <td className="py-3">
                  <div className="font-medium">{s.name}</div>
                  {s.description && (
                    <div className="text-xs text-muted-foreground">{s.description}</div>
                  )}
                  {!s.active && (
                    <Badge variant="outline" className="mt-1">
                      Paused
                    </Badge>
                  )}
                </td>
                <td className="py-3">
                  <StateBadge state={s.last_state} />
                  {s.last_eval_at && (
                    <div className="text-xs text-muted-foreground">
                      <RelativeTime iso={s.last_eval_at} />
                    </div>
                  )}
                </td>
                <td className="py-3">
                  <code className="text-xs">{s.crontab}</code>
                  <div className="text-xs text-muted-foreground">{s.timezone}</div>
                </td>
                <td className="py-3 text-muted-foreground">
                  {s.next_run_at ? <RelativeTime iso={s.next_run_at} /> : "—"}
                </td>
                <td className="py-3 text-muted-foreground">
                  {s.owners.map((o) => o.email).join(", ")}
                </td>
                <td className="py-3">
                  <div className="flex justify-end gap-2">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setRunLogFor(s)}
                    >
                      <FileClock className="mr-2 h-4 w-4" />
                      Run log
                    </Button>
                    {canManage && (
                      <>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setEditing({ id: s.id })}
                        >
                          Edit
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setPendingDelete(s)}
                        >
                          Delete
                        </Button>
                      </>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {editing && (
        <Dialog open onOpenChange={(o) => !o && setEditing(null)}>
          <DialogContent className="max-h-[90vh] max-w-2xl overflow-y-auto">
            <DialogHeader>
              <DialogTitle>
                {editing.id ? `Edit ${typeLabel}` : `New ${typeLabel}`}
              </DialogTitle>
            </DialogHeader>
            <ScheduleEditor
              ws={ws}
              type={tab}
              scheduleId={editing.id}
              onDone={() => setEditing(null)}
            />
          </DialogContent>
        </Dialog>
      )}

      {runLogFor && (
        <RunLogDrawer
          ws={ws}
          schedule={runLogFor}
          onClose={() => setRunLogFor(null)}
        />
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => !o && setPendingDelete(null)}
        title={`Delete ${pendingDelete?.name ?? ""}?`}
        description="Its run history and recipients are deleted with it. This cannot be undone."
        onConfirm={async () => {
          if (pendingDelete) await remove.mutateAsync(pendingDelete.id);
          setPendingDelete(null);
        }}
      />
    </div>
  );
}

/** Exported for the run-log drawer, which shows the same vocabulary. */
export { STATE_LABEL, STATE_VARIANT, StateBadge };
