import { ConfigSection } from "./ConfigSection";
import { PropertyField } from "./PropertyField";
import { knownProperties } from "./knownProperties";

/**
 * "Spilling" tuning: spill-to-disk toggle, spill path, and per-node spill
 * limits. Enabling spill requires a writable volume for `spiller-spill-path`.
 */
export function SpillingSection({
  getProp,
  setProp,
}: {
  getProp: (file: "config" | "node" | "log", k: string) => string;
  setProp: (file: "config" | "node" | "log", k: string, v: string) => void;
}) {
  return (
    <ConfigSection
      title="Spilling"
      description="Spill operator memory to disk under memory pressure. Requires a writable spill volume when enabled."
      docsUrl="https://trino.io/docs/current/admin/properties-spilling.html"
    >
      <div className="space-y-4">
        {knownProperties
          .filter((p) => p.section === "spilling")
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
