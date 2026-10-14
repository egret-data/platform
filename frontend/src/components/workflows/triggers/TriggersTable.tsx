import { useId, type ComponentType } from "react";
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
import {
  FLOW_DISABLED_STATUS,
  kindLabel,
  MANAGE_REASON,
  shortType,
  statusText,
  switchEnablement,
  type FlowContext,
  type TriggerRow,
} from "@/components/workflows/triggers/triggersModel";

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
          {variant === "global" ? (
            <>
              <TableHead>Namespace</TableHead>
              <TableHead>Flow</TableHead>
            </>
          ) : null}
          <TableHead>Type</TableHead>
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
            <TableRow key={`${key}#${i}`} data-trigger={row.trigger_id}>
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
                <Link className="font-mono text-xs hover:underline" to={detailsHref(row)}>
                  {row.trigger_id}
                </Link>
                {row.duplicate_id ? (
                  <Badge variant="outline" className="ml-2">
                    Duplicate id
                  </Badge>
                ) : null}
              </TableCell>
              {variant === "global" ? (
                <>
                  <TableCell>
                    <Link
                      className="text-muted-foreground hover:underline"
                      to={`${base}/namespaces/${encodeURIComponent(row.namespace ?? "")}/overview`}
                    >
                      {row.namespace}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <Link className="hover:underline" to={`${base}/flows/${encodeURIComponent(row.flow_uuid)}/triggers`}>
                      {row.flow_id}
                    </Link>
                  </TableCell>
                </>
              ) : null}
              <TableCell>
                <span className="inline-flex items-center gap-1.5" title={variant === "flow" ? row.type : undefined}>
                  <KindIcon aria-hidden className="h-4 w-4 text-muted-foreground" />
                  <span className="sr-only">{kindLabel(row.kind)}: </span>
                  {shortType(row.type)}
                </span>
              </TableCell>
              <TableCell className="text-muted-foreground">
                {row.schedule?.next_evaluation_date ? <RelativeTime iso={row.schedule.next_evaluation_date} /> : "—"}
              </TableCell>
              <TableCell className="text-muted-foreground">
                {row.schedule?.evaluated_at ? <RelativeTime iso={row.schedule.evaluated_at} /> : "—"}
              </TableCell>
              <TableCell>
                {row.last_execution ? (
                  <span className="inline-flex flex-wrap items-center gap-1.5">
                    <StateChip state={row.last_execution.state} kind="execution" />
                    <Link
                      className="font-mono text-xs hover:underline"
                      to={`${base}/executions/${encodeURIComponent(row.last_execution.id)}/overview`}
                      title={row.last_execution.id}
                    >
                      {row.last_execution.id.slice(0, 8)}
                    </Link>
                    <RelativeTime className="text-xs text-muted-foreground" iso={row.last_execution.created_at} />
                  </span>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </TableCell>
              <TableCell data-testid="trigger-status">{status}</TableCell>
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
                  {enablement.ok ? null : (
                    <span id={reasonId} className="text-xs text-muted-foreground">
                      {enablement.reason}
                    </span>
                  )}
                </span>
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
          );
        })}
      </TableBody>
    </Table>
    </div>
  );
}
