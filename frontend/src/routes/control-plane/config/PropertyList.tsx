import { useMemo } from "react";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { KeyedPropertyList } from "./KeyedPropertyList";
import { knownProperties } from "./knownProperties";
import { trinoCatalog, trinoCatalogByKey } from "./trinoCatalog";

type PropFile = "config" | "node" | "log";

// config.properties keys that have their OWN dedicated controls in the
// Fault-tolerant execution section (retry-policy Select + the retry-attempt
// caps). Hidden from the generic config.properties list + picker so they can't
// be edited in two places (which would desync retry-policy and double-count the
// nav badge).
const FT_OWNED_CONFIG_KEYS = new Set<string>([
  "retry-policy",
  ...knownProperties
    .filter((p) => p.section === "reliability")
    .map((p) => p.key),
]);

/** The property list for one Trino config file, backed by the file-scoped
 *  `trinoCatalog` subset and the hook's `getProp`/`setProp` router. */
export function PropertyList({
  cfg,
  file,
  rows,
}: {
  cfg: EngineConfigForm;
  file: PropFile;
  rows: { key: string }[];
}) {
  const catalog = useMemo(
    () =>
      trinoCatalog.filter(
        (p) => p.file === file && !(file === "config" && FT_OWNED_CONFIG_KEYS.has(p.key)),
      ),
    [file],
  );
  const presentKeys =
    file === "config"
      ? rows.map((r) => r.key).filter((k) => !FT_OWNED_CONFIG_KEYS.has(k))
      : rows.map((r) => r.key);
  return (
    <KeyedPropertyList
      catalog={catalog}
      catalogByKey={trinoCatalogByKey}
      presentKeys={presentKeys}
      getValue={(k) => cfg.getProp(file, k)}
      setValue={(k, v) => cfg.setProp(file, k, v)}
    />
  );
}
