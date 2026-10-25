import { Fragment, useId, type ComponentType } from "react";
import { Link } from "react-router-dom";
import { Clock, Info, Loader2, Puzzle, Webhook, Workflow } from "lucide-react";
import type { TriggerKindDto } from "@/api/generated/model";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { StateChip } from "@/components/workflows/StateChip";
import { BackfillCell, BackfillStrip } from "@/components/workflows/triggers/BackfillStrip";
import {
  FLOW_DISABLED_STATUS,
  kindLabel,
  MANAGE_REASON,
  shortType,
  firesNext as fires,
  statusText,
  switchEnablement,
  type FlowContext,
  type TriggerRow,
} from "@/components/workflows/triggers/triggersModel";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";

const KIND_ICONS: Record<TriggerKindDto, ComponentType<{ className?: string; "aria-hidden"?: boolean }>> = {
  schedule: Clock,
  webhook: Webhook,
  flow: Workflow,
  other: Puzzle,
};

/** A row as the global page has it: the flow's namespace and YAML id beside the entry. */
export type TableRowData = TriggerRow & { namespace?: string; flow_id?: string };

/**
 * How a row's controls name it: the trigger id on the flow's tab; on the global
 * page, where two flows can declare the same id, the id and its flow
 * (`daily of company.team.hello`).
 */
export const rowName = (row: TableRowData, variant: "flow" | "global") =>
  variant === "global" && row.namespace !== undefined && row.flow_id !== undefined
    ? `${row.trigger_id} of ${row.namespace}.${row.flow_id}`
    : row.trigger_id;

/** The key a row is selected by: a trigger id is unique within its flow only. */
export const selectionKey = (r: Pick<TriggerRow, "flow_uuid" | "trigger_id">) => `${r.flow_uuid}/${r.trigger_id}`;

/** The tooltip of "Last evaluated" (spec §4.1). */
export const LAST_EVALUATED_HELP = "The schedule tick the scheduler last processed, not the wall-clock time it fired.";

/**
 * The trigger table of the flow's Triggers tab and the global Triggers page
 * (spec §4.1, §4.4; FE-PAGE-4). The switches show the server's state: a row
 * changes when the caller's data does, never on a click.
 */
export function TriggersTable({
  rows,
  variant,
  base,
  manage,
  selection,
  onSelectionChange,
  onToggle,
  isBusy,
  contextOf,
  detailsHref,
  selectionBusy = false,
  onBackfill,
  backfillActions,
}: {
  rows: TableRowData[];
  /** `global` adds the Namespace and Flow columns (links), and names each row by its flow too. */
  variant: "flow" | "global";
  /** `/explore/{ws}/workflows`: execution and flow links start here. */
  base: string;
  /** The caller holds `workflow:manage`: selection is usable. */
  manage: boolean;
  selection: ReadonlySet<string>;
  onSelectionChange: (next: Set<string>) => void;
  onToggle: (row: TableRowData, on: boolean) => void;
  isBusy: (row: TableRowData) => boolean;
  contextOf: (row: TableRowData) => FlowContext;
  /** Where Details and the trigger's id lead: the drawer, through the URL. */
  detailsHref: (row: TableRowData) => string;
  /**
   * The rows are a placeholder while the next page or filter loads (the global page): the
   * selection boxes ignore clicks, so no bulk action is aimed at rows about to be replaced.
   * `aria-disabled`, not `disabled`, so a focused box keeps the focus.
   */
  selectionBusy?: boolean;
  /** Opens the create form for a schedule with no backfill; absent, the cell shows a dash for "none". */
  onBackfill?: (row: TableRowData) => void;
  /** Pause, resume and delete a row's backfill, and whether one of them is in flight. */
  backfillActions: {
    isBusy: (row: TableRowData) => boolean;
    pause: (row: TableRowData, paused: boolean) => void;
    remove: (row: TableRowData) => void;
  };
}) {
  const idBase = useId();
  const keys = rows.map(selectionKey);
  const selectedCount = keys.filter((k) => selection.has(k)).length;
  const all = rows.length > 0 && selectedCount === rows.length;
  const selectNoteId = `${idBase}-select-note`;
  const selectDescribedBy = manage ? undefined : selectNoteId;

  return (
    <div className="grid gap-1">
    {manage ? null : (
      // Why the selection boxes are disabled, as text the boxes name in `aria-describedby` (FE-ACTION-4).
      <p id={selectNoteId} className="text-xs text-muted-foreground">
        <span aria-hidden>Selecting triggers: </span>
        {MANAGE_REASON}
      </p>
    )}
    <Table data-testid="triggers-table">
      <TableHeader>
        <TableRow>
          <TableHead className="w-8">
            <Checkbox
              aria-label="Select all triggers"
              aria-describedby={selectDescribedBy}
              disabled={!manage || rows.length === 0}
              aria-disabled={manage && selectionBusy ? true : undefined}
              checked={all ? true : selectedCount > 0 ? "indeterminate" : false}
              onCheckedChange={(c) => {
                if (!selectionBusy) onSelectionChange(c === true ? new Set(keys) : new Set());
              }}
            />
          </TableHead>
          <TableHead>ID</TableHead>
          {variant === "global" ? <TableHead>Flow</TableHead> : null}
          <TableHead>Type</TableHead>
          {/* One line per header and per row (KI-226): no column wraps its header, and every
              cell is one line high, its detail in the drawer. */}
          <TableHead>Next run</TableHead>
          <TableHead>
            <span className="inline-flex items-center gap-1">
              Last evaluated
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    aria-label="About Last evaluated"
                    className="rounded-sm text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Info aria-hidden className="h-3.5 w-3.5" />
                  </button>
                </TooltipTrigger>
                <TooltipContent>{LAST_EVALUATED_HELP}</TooltipContent>
              </Tooltip>
            </span>
          </TableHead>
          <TableHead>Last execution</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>Enabled</TableHead>
          <TableHead>Backfill</TableHead>
          <TableHead>
            <span className="sr-only">Details</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row, i) => {
          const key = keys[i];
          const ctx = contextOf(row);
          const busy = isBusy(row);
          const enablement = switchEnablement(row, { manage });
          const reasonId = `${idBase}-reason-${i}`;
          const busyId = `${idBase}-busy-${i}`;
          const on = !row.runtime_disabled && !row.yaml_disabled;
          const KindIcon = KIND_ICONS[row.kind] ?? Puzzle;
          const status = statusText(row, ctx);
          const name = rowName(row, variant);
          return (
            <Fragment key={`${key}#${i}`}>
              <TableRow data-trigger={row.trigger_id}>
                <TableCell>
                  <Checkbox
                    aria-label={`Select ${name}`}
                    aria-describedby={selectDescribedBy}
                    disabled={!manage}
                    aria-disabled={manage && selectionBusy ? true : undefined}
                    checked={selection.has(key)}
                    onCheckedChange={(c) => {
                      if (selectionBusy) return;
                      const next = new Set(selection);
                      if (c === true) next.add(key);
                      else next.delete(key);
                      onSelectionChange(next);
                    }}
                  />
                </TableCell>
                <TableCell>
                  <Link className={`font-mono text-xs ${ENTITY_LINK_CLASS}`} to={detailsHref(row)}>
                    {row.trigger_id}
                  </Link>
                  {row.duplicate_id ? (
                    <Badge variant="outline" className="ml-2">
                      Duplicate id
                    </Badge>
                  ) : null}
                </TableCell>
                {variant === "global" ? (
                  <TableCell className="max-w-64 truncate" title={`${row.namespace}.${row.flow_id}`}>
                    <Link
                      className="text-muted-foreground hover:underline"
                      to={`${base}/namespaces/${encodeURIComponent(row.namespace ?? "")}/overview`}
                    >
                      {row.namespace}
                    </Link>
                    <span className="text-muted-foreground">.</span>
                    <Link className="hover:underline" to={`${base}/flows/${encodeURIComponent(row.flow_uuid)}/triggers`}>
                      {row.flow_id}
                    </Link>
                  </TableCell>
                ) : null}
                <TableCell>
                  <span className="inline-flex items-center gap-1.5" title={variant === "flow" ? row.type : undefined}>
                    <KindIcon aria-hidden className="h-4 w-4 text-muted-foreground" />
                    <span className="sr-only">{kindLabel(row.kind)}: </span>
                    <span className={variant === "global" ? "hidden 2xl:inline" : undefined}>{shortType(row.type)}</span>
                  </span>
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {/* A trigger that does not fire has no next run (KI-189): its stored date is
                      the one it would resume from, in the past once it has been off a while. */}
                  {row.schedule?.next_evaluation_date && fires(row, ctx) ? <RelativeTime iso={row.schedule.next_evaluation_date} /> : "—"}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {row.schedule?.evaluated_at ? <RelativeTime iso={row.schedule.evaluated_at} /> : "—"}
                </TableCell>
                <TableCell>
                  {row.last_execution ? (
                    <span className="inline-flex items-center gap-1.5">
                      <StateChip state={row.last_execution.state} kind="execution" />
                      {/* The time is the link; the execution id is its title and in its name. */}
                      <Link
                        className="text-xs text-muted-foreground hover:underline"
                        to={`${base}/executions/${encodeURIComponent(row.last_execution.id)}/overview`}
                        title={row.last_execution.id}
                        aria-label={`Execution ${row.last_execution.id}`}
                      >
                        <RelativeTime iso={row.last_execution.created_at} />
                      </Link>
                    </span>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
                <TableCell data-testid="trigger-status" className="max-w-40 truncate" title={status}>
                  {status}
                </TableCell>
                <TableCell>
                  <span className="inline-flex items-center gap-2">
                    <Switch
                      checked={on}
                      disabled={!enablement.ok}
                      // In flight: still focusable, so the keyboard focus stays on it (a `disabled`
                      // button drops focus to the page); the click is ignored until the answer.
                      aria-disabled={enablement.ok && busy ? true : undefined}
                      aria-label={`Trigger ${name} enabled`}
                      aria-describedby={!enablement.ok ? reasonId : busy ? busyId : undefined}
                      title={enablement.ok ? (on && ctx.flowDisabled ? FLOW_DISABLED_STATUS : undefined) : enablement.reason}
                      onCheckedChange={(c) => {
                        if (!busy) onToggle(row, c);
                      }}
                    />
                    {busy ? (
                      <>
                        <Loader2 aria-hidden className="h-4 w-4 animate-spin text-muted-foreground" />
                        <span id={busyId} className="sr-only">
                          Switching…
                        </span>
                      </>
                    ) : null}
                    {/* The reason is the switch's description and its tooltip; the Status column
                        says it in words already, so it is not repeated beside the switch (KI-190). */}
                    {enablement.ok ? null : (
                      <span id={reasonId} className="sr-only">
                        {enablement.reason}
                      </span>
                    )}
                  </span>
                </TableCell>
                <TableCell>
                  <BackfillCell row={row} manage={manage} onBackfill={onBackfill} />
                </TableCell>
                <TableCell>
                  <Link
                    className="text-sm text-muted-foreground hover:underline"
                    to={detailsHref(row)}
                    aria-label={`Details of ${name}`}
                  >
                    Details
                  </Link>
                </TableCell>
              </TableRow>
              {row.backfill ? (
                <BackfillStrip
                  row={row}
                  colSpan={variant === "global" ? 11 : 10}
                  name={name}
                  manage={manage}
                  busy={backfillActions.isBusy(row)}
                  onPause={(paused) => backfillActions.pause(row, paused)}
                  onDelete={() => backfillActions.remove(row)}
                />
              ) : null}
            </Fragment>
          );
        })}
      </TableBody>
    </Table>
    </div>
  );
}
