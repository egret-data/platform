import { useQuery } from "@tanstack/react-query";

import { type Storage, listStorage } from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";
import { PropertyField } from "./config/PropertyField";
import { knownProperties } from "./config/knownProperties";

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
  getProp: (k: string) => string;
  setProp: (k: string, v: string) => void;
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
    getProp,
    setProp,
  } = props;

  const storage = useQuery<Storage[], ApiError>({
    queryKey: ["cp", "storage"],
    queryFn: listStorage,
    retry: false,
  });

  return (
    <div className="space-y-5">
    <ConfigSection
      title="Fault-tolerant execution"
      description="Retry failed tasks/queries, spooling exchange data to an S3 storage connection."
      docsUrl="https://trino.io/docs/current/admin/fault-tolerant-execution.html"
    >
      <div className="flex items-center justify-between gap-4">
        <FieldLabel
          fieldKey="faultTolerantEnabled"
          label="Enable fault-tolerant execution"
          htmlFor="ced-ft-enabled"
        />
        <Switch
          id="ced-ft-enabled"
          checked={ftEnabled}
          onCheckedChange={setFtEnabled}
        />
      </div>
      {ftEnabled && (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <div className="space-y-1.5">
            <FieldLabel
              fieldKey="retryPolicy"
              label="Retry policy"
              htmlFor="ced-ft-retry"
            />
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
            <FieldLabel
              fieldKey="exchangeStorage"
              label="Exchange storage"
              htmlFor="ced-ft-storage"
            />
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
            <FieldLabel
              fieldKey="exchangePrefix"
              label="Exchange prefix"
              htmlFor="ced-ft-prefix"
            />
            <Input
              id="ced-ft-prefix"
              value={exchangePrefix}
              placeholder="exchange"
              onChange={(e) => setExchangePrefix(e.target.value)}
            />
          </div>
        </div>
      )}
    </ConfigSection>

      <ConfigSection
        title="Retry limits"
        description="Per-policy retry attempt caps applied when fault-tolerant execution is enabled."
        docsUrl="https://trino.io/docs/current/admin/fault-tolerant-execution.html"
      >
        <div className="space-y-4">
          {knownProperties
            .filter((p) => p.section === "reliability")
            .map((prop) => (
              <PropertyField
                key={prop.key}
                prop={prop}
                value={getProp(prop.key)}
                onChange={(v) => setProp(prop.key, v)}
              />
            ))}
        </div>
      </ConfigSection>
    </div>
  );
}
