import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { SankeyConfig } from "@/api/charts";
import { Toggle } from "@/components/chart/settings/ChartAxesSection";

/**
 * The sankey's Display controls, in Metabase's own order: Align, Show edge labels, Edge color.
 *
 * A sankey has no axes, so it keeps Metabase's two tabs — Data | Display — rather than the three
 * the waterfall and box plot get.
 */
export function SankeyDisplaySection({ config, onChange }: {
  config: SankeyConfig;
  onChange: (patch: Partial<SankeyConfig>) => void;
}) {
  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <Label htmlFor="sk-align" className="text-sm font-normal">Align</Label>
        <Select value={config.nodeAlign ?? "left"}
          onValueChange={(v) => onChange({ nodeAlign: v === "left" ? undefined : v as SankeyConfig["nodeAlign"] })}>
          <SelectTrigger id="sk-align" aria-label="Align" className="h-8 w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="left">Left</SelectItem>
            <SelectItem value="right">Right</SelectItem>
            <SelectItem value="justify">Justify</SelectItem>
          </SelectContent>
        </Select>
      </div>

      <Toggle id="sk-edge-labels" label="Show edge labels" checked={!!config.showEdgeLabels}
        onChange={(v) => onChange({ showEdgeLabels: v || undefined })} />

      {/* Metabase hides the formatting choice until there are labels to format. */}
      {config.showEdgeLabels && (
        <div className="space-y-1">
          <Label htmlFor="sk-fmt" className="text-sm font-normal">Auto formatting</Label>
          <Select value={config.labelFormatting ?? "auto"}
            onValueChange={(v) => onChange({ labelFormatting: v === "auto" ? undefined : v as "compact" | "full" })}>
            <SelectTrigger id="sk-fmt" aria-label="Auto formatting" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto">Auto</SelectItem>
              <SelectItem value="compact">Compact</SelectItem>
              <SelectItem value="full">Full</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}

      {/* An edge belongs to two nodes, so colouring it has to pick one. A gradient — which is what
          egret drew — belongs to neither and cannot be followed with the eye, which is the one
          thing a sankey is read for. */}
      <div className="space-y-1">
        <Label htmlFor="sk-edge-color" className="text-sm font-normal">Edge color</Label>
        <Select value={config.edgeColor ?? "source"}
          onValueChange={(v) => onChange({ edgeColor: v === "source" ? undefined : v as "gray" | "target" })}>
          <SelectTrigger id="sk-edge-color" aria-label="Edge color" className="h-8 w-full"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="gray">Gray</SelectItem>
            <SelectItem value="source">Source</SelectItem>
            <SelectItem value="target">Target</SelectItem>
          </SelectContent>
        </Select>
      </div>
    </div>
  );
}
