import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
    <>
      <div className="grid grid-cols-2 gap-4">
        <div className="space-y-1.5">
          <Label htmlFor="ced-qmm">Query max memory</Label>
          <Input
            id="ced-qmm"
            value={queryMaxMemory}
            onChange={(e) => setQueryMaxMemory(e.target.value)}
            placeholder="4GB"
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="ced-log-level">Log level</Label>
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

      <div className="space-y-1.5">
        <Label>Config properties</Label>
        <p className="text-xs text-muted-foreground">
          Raw <code>config.properties</code> key/value pairs (both roles).
        </p>
        <KeyValueEditor
          rows={configProps}
          onChange={setConfigProps}
          keyPlaceholder="query.max-run-time"
          valuePlaceholder="1h"
        />
      </div>

      <div className="space-y-1.5">
        <Label>Environment variables</Label>
        <p className="text-xs text-muted-foreground">
          Extra env applied to both coordinator and worker pods.
        </p>
        <KeyValueEditor
          rows={env}
          onChange={setEnv}
          keyPlaceholder="MY_VAR"
          valuePlaceholder="value"
          addLabel="Add variable"
        />
      </div>
    </>
  );
}
