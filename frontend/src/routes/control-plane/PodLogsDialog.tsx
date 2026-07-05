import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Loader2, RefreshCw } from "lucide-react";

import { type PodLogs, getPodLogs } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

const TAIL_OPTIONS = [200, 1000, 5000] as const;

/**
 * Control Plane → Engine → Overview → pod row "Logs" action.
 *
 * Fetches `GET .../pods/{pod}/logs` (tail/previous-scoped) and renders it in a
 * scrollable `<pre>`. Open state is driven by `pod !== null` — the caller owns
 * the selected-pod state and clears it via `onOpenChange(false)`.
 */
export default function PodLogsDialog({
  engineId,
  pod,
  onOpenChange,
}: {
  engineId: string;
  pod: string | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [tail, setTail] = useState(1000);
  const [previous, setPrevious] = useState(false);

  const logs = useQuery<PodLogs, ApiError>({
    queryKey: ["cp", "pod-logs", engineId, pod, tail, previous],
    queryFn: () => getPodLogs(engineId, pod!, { tail, previous }),
    enabled: pod != null,
    retry: false,
  });

  return (
    <Dialog open={pod != null} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-3xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle className="font-mono text-sm">{pod}</DialogTitle>
          <DialogDescription>Container logs for this pod.</DialogDescription>
        </DialogHeader>

        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-3">
            <Select
              value={String(tail)}
              onValueChange={(v) => setTail(Number(v))}
            >
              <SelectTrigger className="w-[140px]" aria-label="Tail size">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {TAIL_OPTIONS.map((n) => (
                  <SelectItem key={n} value={String(n)}>
                    Last {n} lines
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <label className="flex items-center gap-2 text-sm text-muted-foreground">
              <Switch checked={previous} onCheckedChange={setPrevious} />
              Previous container
            </label>

            <Button
              type="button"
              variant="outline"
              size="sm"
              className="ml-auto"
              onClick={() => logs.refetch()}
              disabled={logs.isFetching}
            >
              {logs.isFetching ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <RefreshCw className="h-4 w-4" />
              )}
              Refresh
            </Button>
          </div>

          {logs.isLoading ? (
            <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              Loading…
            </div>
          ) : logs.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {logs.error?.message ?? "Failed to load logs."}
              </AlertDescription>
            </Alert>
          ) : (
            <pre className="max-h-[60vh] overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap">
              {logs.data?.logs ? (
                logs.data.logs
              ) : (
                <span className="text-muted-foreground">(no output)</span>
              )}
            </pre>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
