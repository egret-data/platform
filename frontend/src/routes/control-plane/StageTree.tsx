import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";

// The Trino `outputStage` JSON is a deep, partially-optional tree — every field
// is guarded and traversed as `any`. Narrowing it into a full type buys nothing
// here (we only read a handful of leaf fields for display).


/** Colour a stage/task state like the query-state badges. */
function stateVariant(state: string): StatusTone {
  switch (String(state).toUpperCase()) {
    case "RUNNING":
      return "primary";
    case "SCHEDULED":
    case "SCHEDULING":
    case "PLANNED":
      return "highlight";
    case "FINISHED":
      return "success";
    case "FAILED":
    case "ABORTED":
    case "CANCELED":
      return "destructive";
    default:
      return "neutral";
  }
}

/** Compact human-readable count (rows/bytes are raw numbers from Trino). */
function fmtNum(n: unknown): string {
  if (typeof n !== "number" || !Number.isFinite(n)) return "—";
  if (n < 1000) return String(n);
  const units = ["K", "M", "B", "T"];
  let v = n;
  let u = -1;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u += 1;
  }
  return `${v.toFixed(1)}${units[u]}`;
}

/** A per-task-state tally, e.g. "3 tasks (2 RUNNING, 1 FINISHED)". */
function taskSummary(tasks: any): string {
  if (!Array.isArray(tasks) || tasks.length === 0) return "0 tasks";
  const tally: Record<string, number> = {};
  for (const t of tasks) {
    const s = String(t?.taskStatus?.state ?? "UNKNOWN").toUpperCase();
    tally[s] = (tally[s] ?? 0) + 1;
  }
  const parts = Object.entries(tally)
    .map(([s, c]) => `${c} ${s}`)
    .join(", ");
  return `${tasks.length} task${tasks.length === 1 ? "" : "s"} (${parts})`;
}

/**
 * A recursive Trino stage-tree node. Renders the stage id + state badge, a
 * compact rows/bytes + task line, and recurses into `subStages` with indentation.
 * Every field is guarded — the JSON is deep and partially optional.
 */
export default function StageTree({ stage }: { stage: any }) {
  if (!stage || typeof stage !== "object") return null;

  const stageId = stage?.stageId ?? stage?.plan?.id ?? "—";
  const state = stage?.state ?? "UNKNOWN";
  const rows = stage?.stageStats?.totalRows ?? stage?.stageStats?.outputPositions;
  const bytes = stage?.stageStats?.totalBytes ?? stage?.stageStats?.outputDataSize;
  const subStages = Array.isArray(stage?.subStages) ? stage.subStages : [];

  return (
    <div
      className="border-l py-1 pl-3"
      style={{ borderColor: "var(--border)" }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-xs font-medium">Stage {String(stageId)}</span>
        <StatusBadge tone={stateVariant(state)} className="text-[10px]">
          {String(state)}
        </StatusBadge>
        <span className="text-xs tabular-nums text-muted-foreground">
          {fmtNum(rows)} rows · {fmtNum(bytes)} bytes
        </span>
        <span className="text-xs text-muted-foreground">{taskSummary(stage?.tasks)}</span>
      </div>

      {subStages.length > 0 && (
        <div className="mt-1 space-y-1">
          {subStages.map((sub: any, i: number) => (
            <StageTree key={sub?.stageId ?? sub?.plan?.id ?? i} stage={sub} />
          ))}
        </div>
      )}
    </div>
  );
}
