import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EngineConfigReliability } from "../EngineConfigReliability";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "./ConfigSection";
import { ConfigTable, ConfigTableRow } from "./ConfigTable";
import {
  eventListenerCatalog,
  eventListenerCatalogByKey,
} from "./eventListenerCatalog";
import { KeyedPropertyList } from "./KeyedPropertyList";
import { PropertyList } from "./PropertyList";
import { QuantityValue } from "./QuantityInput";

const LOG_LEVELS = ["INFO", "DEBUG", "WARN", "ERROR"] as const;

/**
 * "Server" section — cluster-wide server config mirroring the chart `server` +
 * `additional*Properties` + exchange manager: query memory ceiling & log level,
 * the three split property files (config/node/log), the raw event listener, and
 * fault-tolerant execution (exchange manager).
 */
export function ServerSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    queryMaxMemory,
    setQueryMaxMemory,
    logLevel,
    setLogLevel,
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
        title="Server"
        description="Cluster-wide query memory ceiling and server log level."
        docsUrl="https://trino.io/docs/current/admin/properties-resource-management.html"
      >
        <ConfigTable>
          <ConfigTableRow
            label="query.max-memory"
            mono
            fieldKey="queryMaxMemory"
            htmlFor="ced-query-max-memory"
          >
            <QuantityValue
              id="ced-query-max-memory"
              value={queryMaxMemory}
              onChange={setQueryMaxMemory}
              placeholder="4GB"
            />
          </ConfigTableRow>
          <ConfigTableRow label="Log level" fieldKey="logLevel" htmlFor="ced-log-level">
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
          </ConfigTableRow>
        </ConfigTable>
      </ConfigSection>

      <ConfigSection
        title="config.properties"
        description="Cluster-wide Trino config.properties. Pick from the catalog or add a custom key."
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
        title="log.properties"
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
