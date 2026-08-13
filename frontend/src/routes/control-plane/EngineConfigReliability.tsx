import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { Link } from "react-router-dom";

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
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "./config/ConfigTable";
import { PropertyValue } from "./config/PropertyField";
import { knownProperties } from "./config/knownProperties";
import { useValidity } from "./config/validityRegistry";

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
  getProp: (file: "config" | "node" | "log", k: string) => string;
  setProp: (file: "config" | "node" | "log", k: string, v: string) => void;
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

  // Fault-tolerant execution needs an exchange storage connection to spool to;
  // the backend drops FTE entirely without one. Block Save (and show a message)
  // so the toggle isn't silently inert. Cleared on unmount by the registry.
  const missingExchangeStorage = ftEnabled && !exchangeStorageConnId;
  const validity = useValidity();
  useEffect(() => {
    validity?.setInvalid("faultTolerant.exchangeStorage", missingExchangeStorage);
    return () => validity?.setInvalid("faultTolerant.exchangeStorage", false);
  }, [validity, missingExchangeStorage]);

  return (
    <ConfigSection
      title="Fault-tolerant execution"
      description="Retry failed tasks/queries (spooling exchange data to S3) plus the per-policy retry attempt caps."
      docsUrl="https://trino.io/docs/current/admin/fault-tolerant-execution.html"
    >
      <ConfigTable>
        <ConfigTableRow
          label="Enable fault-tolerant execution"
          fieldKey="faultTolerantEnabled"
          htmlFor="ced-ft-enabled"
        >
          <Switch
            id="ced-ft-enabled"
            checked={ftEnabled}
            onCheckedChange={setFtEnabled}
          />
        </ConfigTableRow>
        {ftEnabled && (
          <>
            <ConfigTableRow
              label="retry-policy"
              mono
              details="How failures are retried: QUERY (whole query) or TASK (individual tasks). Set in config.properties."
              htmlFor="ced-ft-retry"
            >
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
            </ConfigTableRow>
            <ConfigTableRow
              label="Exchange storage"
              fieldKey="exchangeStorage"
              htmlFor="ced-ft-storage"
            >
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
            </ConfigTableRow>
            <ConfigTableRow
              label="Exchange prefix"
              fieldKey="exchangePrefix"
              htmlFor="ced-ft-prefix"
            >
              <Input
                id="ced-ft-prefix"
                className="w-full"
                value={exchangePrefix}
                placeholder="exchange"
                onChange={(e) => setExchangePrefix(e.target.value)}
              />
            </ConfigTableRow>
            {/* Per-policy retry attempt caps — only relevant once FTE is on,
                so they live in the same table, right below retry-policy. */}
            {knownProperties
              .filter((p) => p.section === "reliability")
              .map((prop) => (
                <ConfigTableRow
                  key={prop.key}
                  mono
                  label={prop.key}
                  details={prop.help ?? null}
                  docsUrl={prop.docsUrl}
                >
                  <PropertyValue
                    prop={prop}
                    value={getProp("config", prop.key)}
                    onChange={(v) => setProp("config", prop.key, v)}
                  />
                </ConfigTableRow>
              ))}
          </>
        )}
      </ConfigTable>
      {ftEnabled && (storage.data ?? []).length === 0 && (
        <p className="text-xs text-muted-foreground">
          No storage connections yet.{" "}
          <Link
            to="/control-plane/storage"
            className="text-primary hover:underline"
          >
            Add a storage connection
          </Link>
          .
        </p>
      )}
      {missingExchangeStorage && (storage.data ?? []).length > 0 && (
        <p className="text-xs text-destructive">
          Select an exchange storage connection — fault-tolerant execution has no
          effect without one.
        </p>
      )}
    </ConfigSection>
  );
}
