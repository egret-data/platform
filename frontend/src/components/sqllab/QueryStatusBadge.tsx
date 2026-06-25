import { Badge } from "@/components/ui/badge";
import type { QueryStatus } from "@/api/sqllab";

const VARIANT: Record<
  QueryStatus,
  "success" | "destructive" | "warning" | "processing" | "secondary"
> = {
  finished: "success",
  failed: "destructive",
  running: "processing",
  queued: "warning",
  cancelled: "secondary",
};

export function QueryStatusBadge({ status }: { status: QueryStatus }) {
  return (
    <Badge variant={VARIANT[status]} className="capitalize">
      {status}
    </Badge>
  );
}
