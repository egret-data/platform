import { EngineConfigReliability } from "../EngineConfigReliability";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import {
  eventListenerCatalog,
  eventListenerCatalogByKey,
} from "./eventListenerCatalog";
import { KeyedPropertyList } from "./KeyedPropertyList";
import { PropertyList } from "./PropertyList";

/**
 * "Server" section — cluster-wide server config: the config/node/log property
 * files (query.max-memory + the io.trino log level are ordinary entries here),
 * the raw event listener, and fault-tolerant execution (exchange manager).
 */
export function ServerSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    eventListener,
    configProps,
    nodeProps,
    logProps,
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
  } = cfg;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="config.properties"
        description="Cluster-wide Trino config.properties (incl. query.max-memory). Pick from the catalog or add a custom key."
      >
        <PropertyList cfg={cfg} file="config" rows={configProps} />
      </ConfigSection>

      <ConfigSection
        title="node.properties"
        description="Per-node identity and paths (node.properties)."
      >
        <PropertyList cfg={cfg} file="node" rows={nodeProps} />
      </ConfigSection>

      <ConfigSection
        title="log.properties (io.trino sets the log level)"
        description="Logger levels and log output (log.properties)."
      >
        <PropertyList cfg={cfg} file="log" rows={logProps} />
      </ConfigSection>

      <ConfigSection
        title="Event listener"
        description="Raw event-listener.properties."
      >
        <KeyedPropertyList
          catalog={eventListenerCatalog}
          catalogByKey={eventListenerCatalogByKey}
          presentKeys={eventListener.map((kv) => kv.key)}
          getValue={(k) => cfg.getEventListenerProp(k)}
          setValue={(k, v) => cfg.setEventListenerProp(k, v)}
        />
      </ConfigSection>

      <EngineConfigReliability
        ftEnabled={ftEnabled}
        setFtEnabled={setFtEnabled}
        retryPolicy={retryPolicy}
        setRetryPolicy={setRetryPolicy}
        exchangeStorageConnId={exchangeStorageConnId}
        setExchangeStorageConnId={setExchangeStorageConnId}
        exchangePrefix={exchangePrefix}
        setExchangePrefix={setExchangePrefix}
        getProp={getProp}
        setProp={setProp}
      />
    </div>
  );
}
