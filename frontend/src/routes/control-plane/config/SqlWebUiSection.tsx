import { ConfigSection } from "./ConfigSection";
import { PropertyField } from "./PropertyField";
import { knownProperties } from "./knownProperties";

/**
 * "SQL & Web UI" tuning: default catalog/schema, forced session time zone,
 * the Trino Web UI toggle and auth, and dynamic catalog management — the
 * guided `sqlWebUi` known properties.
 */
export function SqlWebUiSection({
  getProp,
  setProp,
}: {
  getProp: (k: string) => string;
  setProp: (k: string, v: string) => void;
}) {
  return (
    <ConfigSection
      title="SQL & Web UI"
      description="Default catalog/schema, forced session time zone, and the Trino Web UI toggle and authentication."
      docsUrl="https://trino.io/docs/current/admin/properties-web-interface.html"
    >
      <div className="space-y-4">
        {knownProperties
          .filter((p) => p.section === "sqlWebUi")
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
  );
}
