import { useEffect, useRef, useState } from "react";
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
 *
 * A "Follow" toggle switches to realtime tailing: it opens
 * `GET .../pods/{pod}/logs/follow?tail=N` as a WebSocket (binary frames, decoded
 * as text and appended — same shape as `PodTerminalDialog`'s exec bridge) and
 * pauses the snapshot query + the previous/refresh controls, which don't apply
 * to a live stream.
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
  const [follow, setFollow] = useState(false);
  const [followText, setFollowText] = useState("");
  const preRef = useRef<HTMLPreElement | null>(null);

  const logs = useQuery<PodLogs, ApiError>({
    queryKey: ["cp", "pod-logs", engineId, pod, tail, previous],
    queryFn: () => getPodLogs(engineId, pod!, { tail, previous }),
    enabled: pod != null && !follow,
    retry: false,
  });

  // Stream `logs/follow` while `follow` is on, appending decoded chunks.
  useEffect(() => {
    if (!follow || pod == null) return;

    setFollowText("");

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/api/control-plane/engines/${engineId}/pods/${encodeURIComponent(pod)}/logs/follow?tail=${tail}`;
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";

    let closedMarkerWritten = false;
    // One streaming decoder for the whole stream, so a multi-byte UTF-8
    // character split across two WS frames is reassembled, not mangled.
    const decoder = new TextDecoder();

    ws.onmessage = (e) => {
      const text =
        e.data instanceof ArrayBuffer
          ? decoder.decode(new Uint8Array(e.data), { stream: true })
          : String(e.data);
      setFollowText((prev) => (prev + text).slice(-200000));
    };

    const onCloseOrError = () => {
      if (closedMarkerWritten) return;
      closedMarkerWritten = true;
      setFollowText((prev) => prev + "\n[stream closed]\n");
    };
    ws.onclose = onCloseOrError;
    ws.onerror = onCloseOrError;

    return () => {
      ws.onmessage = null;
      ws.onclose = null;
      ws.onerror = null;
      ws.close();
    };
  }, [engineId, pod, follow, tail]);

  // Jump to the tail (most recent lines) whenever the logs (re)load or grow.
  useEffect(() => {
    if (preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [logs.data, followText]);

  return (
    <Dialog open={pod != null} onOpenChange={onOpenChange}>
      {/* Height cap and scrolling now come from the primitive, which scrolls an inner
          body rather than the content — the close button no longer scrolls away. */}
      <DialogContent className="max-w-3xl">
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
              <Switch
                checked={previous}
                onCheckedChange={setPrevious}
                disabled={follow}
              />
              Previous container
            </label>

            <div className="ml-auto flex items-center gap-3">
              <label className="flex items-center gap-2 text-sm text-muted-foreground">
                <Switch checked={follow} onCheckedChange={setFollow} />
                Follow
              </label>

              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => logs.refetch()}
                disabled={follow || logs.isFetching}
              >
                {logs.isFetching ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <RefreshCw className="h-4 w-4" />
                )}
                Refresh
              </Button>
            </div>
          </div>

          {!follow && logs.isLoading ? (
            <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              Loading…
            </div>
          ) : !follow && logs.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {logs.error?.message ?? "Failed to load logs."}
              </AlertDescription>
            </Alert>
          ) : (
            <pre
              ref={preRef}
              className="max-h-[60vh] overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre-wrap"
            >
              {follow ? (
                followText ? (
                  followText
                ) : (
                  <span className="text-muted-foreground">
                    (waiting for output…)
                  </span>
                )
              ) : logs.data?.logs ? (
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
