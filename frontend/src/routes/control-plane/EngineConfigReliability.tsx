import { useQuery } from "@tanstack/react-query";

import { type Storage, listStorage } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

/**
 * Cluster-wide "Reliability" config: fault-tolerant execution — retry policy,
 * the S3 storage connection used to spool exchange data, and its prefix.
 */
export function EngineConfigReliability(props: {
  ftEnabled: boolean;
  setFtEnabled: (v: boolean) => void;
  retryPolicy: "TASK" | "QUERY";
  setRetryPolicy: (v: "TASK" | "QUERY") => void;
  exchangeStorageConnId: string;
  setExchangeStorageConnId: (v: string) => void;
  exchangePrefix: string;
  setExchangePrefix: (v: string) => void;
}) {
  const {
    ftEnabled,
    setFtEnabled,
    retryPolicy,
    setRetryPolicy,
    exchangeStorageConnId,
    setExchangeStorageConnId,
    exchangePrefix,
    setExchangePrefix,
  } = props;

  const storage = useQuery<Storage[], ApiError>({
    queryKey: ["cp", "storage"],
    queryFn: listStorage,
    retry: false,
  });

  return (
    <div className="space-y-3 rounded-lg border p-4">
      <div className="flex items-center justify-between gap-4">
        <div className="space-y-0.5">
          <Label htmlFor="ced-ft-enabled">Fault-tolerant execution</Label>
          <p className="text-xs text-muted-foreground">
            Retry failed tasks/queries, spooling exchange data to an S3 storage
            connection.
          </p>
        </div>
        <Switch
          id="ced-ft-enabled"
          checked={ftEnabled}
          onCheckedChange={setFtEnabled}
        />
      </div>
      {ftEnabled && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <div className="space-y-1.5">
            <Label htmlFor="ced-ft-retry">Retry policy</Label>
            <Select
              value={retryPolicy}
              onValueChange={(v) => setRetryPolicy(v as "TASK" | "QUERY")}
            >
              <SelectTrigger id="ced-ft-retry" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="TASK">TASK</SelectItem>
                <SelectItem value="QUERY">QUERY</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ced-ft-storage">Exchange storage</Label>
            <Select
              value={exchangeStorageConnId}
              onValueChange={setExchangeStorageConnId}
            >
              <SelectTrigger id="ced-ft-storage" className="w-full">
                <SelectValue placeholder="Select a connection" />
              </SelectTrigger>
              <SelectContent>
                {(storage.data ?? []).length === 0 ? (
                  <SelectItem value="__none__" disabled>
                    No storage connections
                  </SelectItem>
                ) : (
                  (storage.data ?? []).map((s) => (
                    <SelectItem key={s.id} value={s.id}>
                      {s.name}
                    </SelectItem>
                  ))
                )}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="ced-ft-prefix">Exchange prefix</Label>
            <Input
              id="ced-ft-prefix"
              value={exchangePrefix}
              placeholder="exchange"
              onChange={(e) => setExchangePrefix(e.target.value)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
