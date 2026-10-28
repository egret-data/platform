import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";
import type { QueryStatus } from "@/api/sqllab";

const TONE: Record<QueryStatus, StatusTone> = {
  finished: "success",
  failed: "destructive",
  running: "primary",
  queued: "highlight",
  cancelled: "inactive",
};

export function QueryStatusBadge({ status }: { status: QueryStatus }) {
  return <StatusBadge tone={TONE[status]}>{status}</StatusBadge>;
}
