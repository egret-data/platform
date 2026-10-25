import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, TableState, VizSettings, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { getDescriptor } from "./registry";
import type { VizDescriptor } from "./types";

/**
 * A viz's own configuration, taken from its descriptor. Descriptors read both stored
 * shapes, so this works for a v2 chart and for one that predates the backfill — which is
 * why retiring `toRenderParams` is not a hard cut-over.
 */
export function vizConfig<Cfg = Record<string, unknown>>(viz: VizType, params: AnyChartParams): Cfg {
  const desc = getDescriptor(viz);
  return (desc ? desc.fromParams(params) : {}) as Cfg;
}

/**
 * The write-side counterpart of `vizConfig`: fold a viz's edited config back into the
 * settings a chart was loaded with.
 *
 * The subtlety this exists for is REMOVAL. Several display controls have an explicit "Auto"
 * state, and Auto is the *absence* of a stored key — the chart applies its own rule instead.
 * The control emits `undefined` for it, `toSettings` correctly omits an undefined key, and
 * then a plain `{ ...settings, ...toSettings(cfg) }` merge quietly re-asserts whatever
 * explicit value was stored before: the control shows Auto, the chart renders as Auto, and
 * saving writes the old value straight back. Auto was reachable but unsavable.
 *
 * A spread cannot express a removal, so the cleared keys are deleted after it. They are
 * derived from the config's own explicitly-undefined entries, namespaced with the
 * descriptor's `prefix` — so this holds for every viz type and every descriptor, not just
 * the ones built by `fieldDescriptor`, and it can never reach a key the shell or another
 * descriptor owns.
 */
export function mergeVizSettings<Cfg>(
  settings: VizSettings,
  descriptor: VizDescriptor<Cfg>,
  cfg: Cfg,
): VizSettings {
  const out: VizSettings = { ...settings, ...descriptor.toSettings(cfg) };
  for (const [k, v] of Object.entries(cfg as Record<string, unknown>)) {
    if (v === undefined) delete out[descriptor.prefix + k];
  }
  return out;
}

/**
 * The settings the shell owns rather than any one viz: number formats, conditional
 * formatting rules and table state. Reads v2's flat keys, falling back to v1's.
 */
export function shellSettings(params: AnyChartParams): {
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];
  tableState: TableState;
} {
  if (isParamsV2(params)) {
    const s = params.settings;
    return {
      columnSettings: (s.column_settings as Record<string, ColumnFormat>) ?? {},
      conditionalFormatting: (s.conditional_formatting as Rule[]) ?? [],
      tableState: (s["table.state"] as TableState) ?? {},
    };
  }
  return {
    columnSettings: params.columnSettings ?? {},
    conditionalFormatting: params.conditionalFormatting ?? [],
    tableState: params.tableState ?? {},
  };
}

/** `params` with `columnSettings` as its column formats, in whichever shape `params` is stored. */
export function withShellColumnSettings(params: AnyChartParams, columnSettings: Record<string, ColumnFormat>): AnyChartParams {
  return isParamsV2(params)
    ? { ...params, settings: { ...params.settings, column_settings: columnSettings } }
    : { ...params, columnSettings };
}
