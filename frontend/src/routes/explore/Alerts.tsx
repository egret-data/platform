import { useMemo, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { Bell } from "lucide-react";
import { toast } from "sonner";
import {
  useScheduleMutations,
  useSchedules,
  type ScheduleSummary,
  type ScheduleType,
} from "@/api/alerts";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { EmptyState } from "@/components/explore/EmptyState";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { FacetSelect, FACET_ALL } from "@/components/explore/FacetSelect";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScheduleEditor } from "@/components/alerts/ScheduleEditor";
import { ScheduleActionsMenu } from "@/components/alerts/ScheduleActionsMenu";
import { RunLogDrawer } from "@/components/alerts/RunLogDrawer";
import { humanizeCrontab } from "@/lib/humanizeCron";
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

/** Sentinel for the "has never run" facet option — `last_state` is null there,
 *  and Radix Select values must be non-empty strings. */
const STATE_NEVER = "__never__";

function StateBadge({ state }: { state: RunState | null | undefined }) {
  if (!state) return <span className="text-sm text-muted-foreground">Never run</span>;
  return <Badge variant={STATE_VARIANT[state]}>{STATE_LABEL[state]}</Badge>;
}

export default function Alerts() {
  const ws = useParams().workspace!;

  // The tab lives in the URL so a report list can be linked, bookmarked and
  // reached with the back button — `?type=report`. Held in component state it
  // was unreachable except by clicking, and every deep link landed on Alerts.
  const [params, setParams] = useSearchParams();
  const tab: ScheduleType = params.get("type") === "report" ? "report" : "alert";
  const setTab = (t: ScheduleType) =>
    setParams(
      (p) => {
        if (t === "alert") p.delete("type");
        else p.set("type", t);
        return p;
      },
      { replace: true },
    );

  const { data, isLoading, isError, error } = useSchedules(ws, tab);
  const { remove, runNow, setActive } = useScheduleMutations(ws);

  const [editing, setEditing] = useState<{ id: string | null } | null>(null);
  const [runLogFor, setRunLogFor] = useState<ScheduleSummary | null>(null);
  const [pendingDelete, setPendingDelete] = useState<ScheduleSummary | null>(null);
  const [stateFilter, setStateFilter] = useState(FACET_ALL);

  const all = useMemo(() => data ?? [], [data]);

  const rows = useMemo(() => {
    if (stateFilter === FACET_ALL) return all;
    if (stateFilter === STATE_NEVER) return all.filter((s) => !s.last_state);
    return all.filter((s) => s.last_state === stateFilter);
  }, [all, stateFilter]);

  const counts = useMemo(() => {
    const active = all.filter((s) => s.active).length;
    return {
      total: all.length,
      active,
      paused: all.length - active,
      failing: all.filter((s) => s.last_state === "error").length,
    };
  }, [all]);

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
  const canManage = all.length > 0 ? all[0].can_manage !== false : true;

  const typeLabel = tab === "alert" ? "alert" : "report";

  /** Facet options are built from the rows actually present, so the dropdown
   *  never offers a state that would filter the list to nothing. */
  const presentStates = useMemo(() => {
    const opts: { value: string; label: string }[] = (Object.keys(STATE_LABEL) as RunState[])
      .filter((s) => all.some((r) => r.last_state === s))
      .map((s) => ({ value: s, label: STATE_LABEL[s] }));
    if (all.some((r) => !r.last_state)) opts.push({ value: STATE_NEVER, label: "Never run" });
    return opts;
  }, [all]);

  const columns: Column<ScheduleSummary>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      value: (s) => s.name,
      cell: (s) => (
        <div className="min-w-0">
          <div className="font-medium">{s.name}</div>
          {s.description && (
            <div className="truncate text-xs text-muted-foreground">{s.description}</div>
          )}
        </div>
      ),
    },
    {
      key: "active",
      label: "Active",
      sortable: true,
      className: "w-20",
      value: (s) => (s.active ? 0 : 1),
      cell: (s) =>
        canManage ? (
          <Switch
            checked={s.active}
            disabled={setActive.isPending}
            aria-label={`${s.active ? "Pause" : "Activate"} ${s.name}`}
            onCheckedChange={(active) =>
              setActive.mutateAsync({ id: s.id, active }).catch((e) => toast.error(String(e)))
            }
          />
        ) : s.active ? (
          <span className="text-xs text-muted-foreground">Active</span>
        ) : (
          <Badge variant="outline">Paused</Badge>
        ),
    },
    {
      key: "last_state",
      label: "Last run",
      sortable: true,
      value: (s) => (s.last_state ? STATE_LABEL[s.last_state] : "Never run"),
      cell: (s) => (
        // The badge is the control that opens the run log: "why is this red?"
        // is the question it provokes, so it should be the thing you click.
        <button
          type="button"
          className="rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={() => setRunLogFor(s)}
          aria-label={`Run log for ${s.name}`}
        >
          <StateBadge state={s.last_state} />
          {s.last_eval_at && (
            // A <span class="block">, not a <div>: a button's content model is
            // phrasing content, and a div inside it is invalid HTML.
            <RelativeTime
              iso={s.last_eval_at}
              className="block text-xs text-muted-foreground"
            />
          )}
        </button>
      ),
    },
    {
      key: "crontab",
      label: "Schedule",
      sortable: true,
      value: (s) => humanizeCrontab(s.crontab),
      cell: (s) => (
        <>
          <span title={s.crontab}>{humanizeCrontab(s.crontab)}</span>
          <div className="text-xs text-muted-foreground">{s.timezone}</div>
        </>
      ),
    },
    {
      key: "next_run_at",
      label: "Next run",
      sortable: true,
      // Paused schedules have no next run; sorting them last keeps the answer
      // to "what fires soonest?" at the top in ascending order.
      value: (s) => s.next_run_at ?? "￿",
      cell: (s) => (
        <span className="text-muted-foreground">
          {s.next_run_at ? <RelativeTime iso={s.next_run_at} /> : "—"}
        </span>
      ),
    },
    {
      key: "owners",
      label: "Owners",
      sortable: true,
      value: (s) => s.owners.map((o) => o.email).join(", "),
      cell: (s) => (
        <span className="text-muted-foreground">{s.owners.map((o) => o.email).join(", ")}</span>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "w-8",
      cell: (s) => (
        <ScheduleActionsMenu
          schedule={s}
          canManage={canManage}
          running={runNow.isPending}
          onRunLog={setRunLogFor}
          onEdit={(x) => setEditing({ id: x.id })}
          onDelete={setPendingDelete}
          onRunNow={(x) =>
            runNow
              .mutateAsync(x.id)
              .then((run) => toast.success(`Ran now: ${STATE_LABEL[run.state]}`))
              .catch((e) => toast.error(String(e)))
          }
        />
      ),
    },
  ];

  // `+ Alert` / `+ Report`, matching `+ Chart` and `+ Dashboard` on the sibling
  // Explore pages — the control-plane pages use a `size="sm"` + icon button
  // instead, and mixing the two is what made this page read as a different app.
  const newButton = canManage ? (
    <Button onClick={() => setEditing({ id: null })}>
      + {typeLabel === "alert" ? "Alert" : "Report"}
    </Button>
  ) : null;

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between gap-4">
        <h1 className="text-lg font-semibold">Alerts &amp; Reports</h1>
        {newButton}
      </div>

      <Tabs
        value={tab}
        onValueChange={(v) => {
          setTab(v as ScheduleType);
          setStateFilter(FACET_ALL);
        }}
        className="mb-3"
      >
        <TabsList>
          <TabsTrigger value="alert">Alerts</TabsTrigger>
          <TabsTrigger value="report">Reports</TabsTrigger>
        </TabsList>
      </Tabs>

      {isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(error as Error)?.message ?? "Failed to load schedules."}
          </AlertDescription>
        </Alert>
      ) : isLoading ? (
        <ListSkeleton rows={6} cols={6} />
      ) : (
        <>
          {all.length > 0 && (
            <div className="mb-3">
              <SummaryStrip
                items={[
                  { label: typeLabel + "s", value: counts.total },
                  { label: "active", value: counts.active },
                  { label: "paused", value: counts.paused },
                  // Clicking the failing count filters to it — the one number
                  // on this page you act on the moment it is non-zero.
                  ...(counts.failing
                    ? [
                        {
                          label: "failing",
                          value: counts.failing,
                          active: stateFilter === "error",
                          onClick: () =>
                            setStateFilter((f) => (f === "error" ? FACET_ALL : "error")),
                        },
                      ]
                    : []),
                ]}
              />
            </div>
          )}

          <FilterableTable
            rows={rows}
            columns={columns}
            getId={(s) => s.id}
            searchPlaceholder={`Search ${typeLabel}s…`}
            initialSort={{ key: "name", dir: "asc" }}
            pageSize={25}
            filters={
              presentStates.length > 0 ? (
                <FacetSelect
                  label="Last run"
                  value={stateFilter}
                  onChange={setStateFilter}
                  options={presentStates}
                />
              ) : undefined
            }
            emptyText={
              // A facet narrowing the list to zero is not a first-run list, so
              // it must not show the "create your first one" call to action.
              stateFilter !== FACET_ALL ? (
                `No ${typeLabel}s with that last run state.`
              ) : (
                <EmptyState
                  icon={Bell}
                  title={`No ${typeLabel}s yet`}
                  description={
                    tab === "alert"
                      ? "An alert runs SQL on a schedule and notifies you when the result meets your condition."
                      : "A report delivers a chart or dashboard on a schedule."
                  }
                  action={newButton}
                />
              )
            }
            noMatchText={`No ${typeLabel}s match that search.`}
          />
        </>
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
        <RunLogDrawer ws={ws} schedule={runLogFor} onClose={() => setRunLogFor(null)} />
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
