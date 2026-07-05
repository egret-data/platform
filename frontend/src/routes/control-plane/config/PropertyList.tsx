import { useMemo } from "react";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { KeyedPropertyList } from "./KeyedPropertyList";
import { trinoCatalog, trinoCatalogByKey } from "./trinoCatalog";

type PropFile = "config" | "node" | "log";

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
    () => trinoCatalog.filter((p) => p.file === file),
    [file],
  );
  return (
    <KeyedPropertyList
      catalog={catalog}
      catalogByKey={trinoCatalogByKey}
      presentKeys={rows.map((r) => r.key)}
      getValue={(k) => cfg.getProp(file, k)}
      setValue={(k, v) => cfg.setProp(file, k, v)}
    />
  );
}
