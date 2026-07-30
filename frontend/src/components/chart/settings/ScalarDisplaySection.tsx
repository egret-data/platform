import type { ScalarConfig, ScalarSegment } from "@/api/charts";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SegmentRangesSection, type SegmentRow } from "./SegmentRangesSection";

/**
 * The scalar's display settings: optional captions, and the conditional-colour bands.
 *
 * Bands are stored and rendered in AUTHORED ORDER because the first matching band wins, so this
 * editor must not sort them — unlike the gauge, where order is the option builder's business.
 */
export function ScalarDisplaySection({ config, onChange }: {
  config: ScalarConfig;
  onChange: (patch: Partial<ScalarConfig>) => void;
}) {
  const segments = Array.isArray(config.segments) ? config.segments : [];

  return (
    <div className="space-y-3 pt-1.5">
      <div className="space-y-1.5">
        <Label htmlFor="scalar-label" className="text-xs text-muted-foreground">Label</Label>
        <Input
          id="scalar-label"
          className="h-7 text-sm"
          placeholder="None"
          value={config.label ?? ""}
          onChange={(e) => onChange({ label: e.target.value.trim() ? e.target.value : undefined })}
        />
        <p className="text-xs text-muted-foreground">
          Off by default — a dashboard card already shows the chart&apos;s name in its header.
        </p>
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="scalar-sublabel" className="text-xs text-muted-foreground">Sublabel</Label>
        <Input
          id="scalar-sublabel"
          className="h-7 text-sm"
          placeholder="None"
          value={config.sublabel ?? ""}
          onChange={(e) => onChange({ sublabel: e.target.value.trim() ? e.target.value : undefined })}
        />
      </div>

      <div>
        <div className="text-xs font-medium">Conditional colours</div>
        <p className="pt-0.5 text-xs text-muted-foreground">
          The number takes the colour of the first range it falls in. Leave a bound empty for an
          open-ended range. A named range also prints its name under the number, so the colour is
          never the only thing carrying the meaning.
        </p>
        <SegmentRangesSection
          rows={segments}
          nullableBounds
          minRows={0}
          emptyHint="No ranges — the number uses the default text colour."
          onChange={(next) => onChange({ segments: next.map(toScalarSegment) })}
        />
      </div>
    </div>
  );
}

function toScalarSegment(s: SegmentRow): ScalarSegment {
  return { min: s.min, max: s.max, color: s.color, label: s.label };
}
