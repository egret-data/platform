import { ConfigSection } from "./ConfigSection";
import { PropertyField } from "./PropertyField";
import { knownProperties } from "./knownProperties";

/**
 * "Query & Memory" tuning: cluster-wide query time/memory limits, the
 * low-memory killer policy, and hash-partition counts — the guided
 * `queryMemory` known properties written into `config.properties`.
 */
export function QueryMemorySection({
  getProp,
  setProp,
}: {
  getProp: (file: "config" | "node" | "log", k: string) => string;
  setProp: (file: "config" | "node" | "log", k: string, v: string) => void;
}) {
  return (
    <ConfigSection
      title="Query & Memory"
      description="Cluster-wide query time and memory limits, low-memory killer policy, and hash-partition counts."
      docsUrl="https://trino.io/docs/current/admin/properties-query-management.html"
    >
      <div className="space-y-4">
        {knownProperties
          .filter((p) => p.section === "queryMemory")
          .map((prop) => (
            <PropertyField
              key={prop.key}
              prop={prop}
              value={getProp("config", prop.key)}
              onChange={(v) => setProp("config", prop.key, v)}
            />
          ))}
      </div>
    </ConfigSection>
  );
}
