import { ConfigSection } from "./ConfigSection";
import { PropertyField } from "./PropertyField";
import { knownProperties } from "./knownProperties";

/**
 * "Execution & Optimizer" tuning: task concurrency and worker threads, join
 * distribution and reordering strategy, node-scheduler knobs, and exchange
 * compression — the guided `execution` known properties.
 */
export function ExecutionOptimizerSection({
  getProp,
  setProp,
}: {
  getProp: (file: "config" | "node" | "log", k: string) => string;
  setProp: (file: "config" | "node" | "log", k: string, v: string) => void;
}) {
  return (
    <ConfigSection
      title="Execution & Optimizer"
      description="Task concurrency, join distribution/reordering, node scheduling, and exchange compression."
      docsUrl="https://trino.io/docs/current/admin/properties-optimizer.html"
    >
      <div className="space-y-4">
        {knownProperties
          .filter((p) => p.section === "execution")
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
