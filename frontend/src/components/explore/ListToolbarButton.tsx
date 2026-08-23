import type { ComponentType } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/**
 * One icon-only control in a list page's header toolbar.
 *
 * This cluster — `Tooltip` + `TooltipTrigger asChild` + an icon `Button` with an
 * `aria-label` + `TooltipContent side="bottom"` — is written out four times per
 * page in `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` and `Alerts.tsx`. The
 * copies had already drifted: some pass `variant={active ? "secondary" : "outline"}`
 * and some do not, and none of them set `aria-pressed`, so a screen reader could
 * not tell an engaged filter from an idle one.
 *
 * `label` is required, not optional. The button's only visible content is an
 * icon, so without it the control has no accessible name at all.
 *
 * `filled` is separate from `active` on purpose: the bookmark and trash buttons
 * fill their glyph when engaged, the bulk and folder buttons do not, and that is
 * a per-icon decision rather than a consequence of being active.
 *
 * `active` also decides whether the control reads as a toggle at all: pass it
 * and the button carries `aria-pressed`, announcing engaged/idle to a screen
 * reader. Omit it — as a momentary action like "New folder" does — and React
 * drops the attribute entirely, rather than defaulting it to `false`. A
 * `false` default would announce a plain action button as "toggle button, not
 * pressed", which is a state the control does not have.
 */
export function ListToolbarButton({
  icon: Icon,
  label,
  active,
  filled = false,
  onClick,
}: {
  icon: ComponentType<{ className?: string }>;
  label: string;
  active?: boolean;
  filled?: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          size="icon"
          variant={active ? "secondary" : "outline"}
          aria-label={label}
          aria-pressed={active}
          onClick={onClick}
        >
          <Icon className={`h-4 w-4 ${active && filled ? "fill-current" : ""}`} />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{label}</TooltipContent>
    </Tooltip>
  );
}
