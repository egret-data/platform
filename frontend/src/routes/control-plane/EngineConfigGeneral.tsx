import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ConfigSection } from "./config/ConfigSection";
import { FieldLabel } from "./config/FieldLabel";
import { knownPropertyKeys } from "./config/knownProperties";
import { QuantityInput } from "./config/QuantityInput";
import { trinoPropertyNames } from "./config/trinoPropertyNames";
import { type Kv, KeyValueEditor } from "./EngineResourceGroups";

const LOG_LEVELS = ["INFO", "DEBUG", "WARN", "ERROR"] as const;

/**
 * Cluster-wide "General" config: query max memory, log level, raw
 * `config.properties`, and extra environment variables (both roles).
 */
export function EngineConfigGeneral(props: {
  queryMaxMemory: string;
  setQueryMaxMemory: (v: string) => void;
  logLevel: string;
  setLogLevel: (v: string) => void;
  configProps: Kv[];
  setConfigProps: React.Dispatch<React.SetStateAction<Kv[]>>;
  env: Kv[];
  setEnv: React.Dispatch<React.SetStateAction<Kv[]>>;
}) {
  const {
    queryMaxMemory,
    setQueryMaxMemory,
    logLevel,
    setLogLevel,
    configProps,
    setConfigProps,
    env,
    setEnv,
  } = props;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Query & logging"
        description="Cluster-wide query memory ceiling and the server log level."
        docsUrl="https://trino.io/docs/current/admin/properties-resource-management.html"
      >
        <div className="grid grid-cols-2 gap-4">
          <QuantityInput
            fieldKey="queryMaxMemory"
            label="Query max memory"
            value={queryMaxMemory}
            onChange={setQueryMaxMemory}
            placeholder="4GB"
          />
          <div className="space-y-1.5">
            <FieldLabel
              fieldKey="logLevel"
              label="Log level"
              htmlFor="ced-log-level"
            />
            <Select value={logLevel} onValueChange={setLogLevel}>
              <SelectTrigger id="ced-log-level" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {LOG_LEVELS.map((lvl) => (
                  <SelectItem key={lvl} value={lvl}>
                    {lvl}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      </ConfigSection>

      <ConfigSection
        title="Config properties"
        description="Raw config.properties key/value pairs (both roles). Properties with a guided field in the tuning sections are managed there and hidden here."
        docsUrl="https://trino.io/docs/current/admin/properties.html"
      >
        <KeyValueEditor
          rows={configProps.filter((kv) => !knownPropertyKeys.has(kv.key))}
          keySuggestions={trinoPropertyNames.filter(
            (p) => !knownPropertyKeys.has(p.key),
          )}
          onChange={(next) => {
            const custom = next.filter((kv) => !knownPropertyKeys.has(kv.key));
            setConfigProps([
              ...configProps.filter((kv) => knownPropertyKeys.has(kv.key)),
              ...custom,
            ]);
          }}
          keyPlaceholder="query.max-run-time"
          valuePlaceholder="1h"
        />
      </ConfigSection>

      <ConfigSection
        title="Environment variables"
        description="Extra env applied to both coordinator and worker pods."
      >
        <KeyValueEditor
          rows={env}
          onChange={setEnv}
          keyPlaceholder="MY_VAR"
          valuePlaceholder="value"
          addLabel="Add variable"
        />
      </ConfigSection>
    </div>
  );
}
