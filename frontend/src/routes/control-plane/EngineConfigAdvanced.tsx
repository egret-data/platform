import { ConfigSection } from "./config/ConfigSection";
import { PropertyField } from "./config/PropertyField";
import { knownProperties } from "./config/knownProperties";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";

/**
 * Cluster-wide "Advanced" config: raw `event-listener.properties`, additional
 * `node.properties`, and additional `log.properties` logger levels.
 */
export function EngineConfigAdvanced(props: {
  eventListener: Kv[];
  setEventListener: React.Dispatch<React.SetStateAction<Kv[]>>;
  nodeProps: Kv[];
  setNodeProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  logProps: Kv[];
  setLogProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  getProp: (file: "config" | "node" | "log", k: string) => string;
  setProp: (file: "config" | "node" | "log", k: string, v: string) => void;
}) {
  const {
    eventListener,
    setEventListener,
    nodeProps,
    setNodeProps,
    logProps,
    setLogProps,
    getProp,
    setProp,
  } = props;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Server logging"
        description="Server log output format for the coordinator and workers."
        docsUrl="https://trino.io/docs/current/installation/deployment.html#log-levels"
      >
        <div className="space-y-4">
          {knownProperties
            .filter((p) => p.section === "advanced")
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

      <ConfigSection
        title="Event listener"
        description="Raw event-listener.properties key/value pairs."
        docsUrl="https://trino.io/docs/current/develop/event-listener.html"
      >
        <KeyValueEditor
          rows={eventListener}
          onChange={setEventListener}
          keyPlaceholder="event-listener.name"
          valuePlaceholder="http"
        />
      </ConfigSection>

      <ConfigSection
        title="Additional node properties"
        description="Extra node.properties key/value pairs."
        docsUrl="https://trino.io/docs/current/admin/properties-node-scheduler.html"
      >
        <KeyValueEditor
          rows={nodeProps}
          onChange={setNodeProps}
          keyPlaceholder="node.internal-address-source"
          valuePlaceholder="FQDN"
        />
      </ConfigSection>

      <ConfigSection
        title="Additional log properties"
        description="Extra log.properties logger levels."
        docsUrl="https://trino.io/docs/current/installation/deployment.html#log-levels"
      >
        <KeyValueEditor
          rows={logProps}
          onChange={setLogProps}
          keyPlaceholder="io.trino.execution"
          valuePlaceholder="DEBUG"
        />
      </ConfigSection>
    </div>
  );
}
