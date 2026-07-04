import { Label } from "@/components/ui/label";
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
}) {
  const {
    eventListener,
    setEventListener,
    nodeProps,
    setNodeProps,
    logProps,
    setLogProps,
  } = props;

  return (
    <>
      <div className="space-y-1.5">
        <Label>Event listener</Label>
        <p className="text-xs text-muted-foreground">
          Raw <code>event-listener.properties</code> key/value pairs.
        </p>
        <KeyValueEditor
          rows={eventListener}
          onChange={setEventListener}
          keyPlaceholder="event-listener.name"
          valuePlaceholder="http"
        />
      </div>

      <div className="space-y-1.5">
        <Label>Additional node properties</Label>
        <p className="text-xs text-muted-foreground">
          Extra <code>node.properties</code> key/value pairs.
        </p>
        <KeyValueEditor
          rows={nodeProps}
          onChange={setNodeProps}
          keyPlaceholder="node.internal-address-source"
          valuePlaceholder="FQDN"
        />
      </div>

      <div className="space-y-1.5">
        <Label>Additional log properties</Label>
        <p className="text-xs text-muted-foreground">
          Extra <code>log.properties</code> logger levels.
        </p>
        <KeyValueEditor
          rows={logProps}
          onChange={setLogProps}
          keyPlaceholder="io.trino.execution"
          valuePlaceholder="DEBUG"
        />
      </div>
    </>
  );
}
