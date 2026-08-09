import { Info } from "lucide-react";
import { Link } from "react-router-dom";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";

/**
 * A chart card's caption: the title, and the description beside it.
 *
 * Metabase's `ChartCaption` reads exactly these two settings — `card.title` and `card.description`
 * (`visualizations/components/ChartCaption.tsx:40-46`) — and makes the title a link when the card
 * resolves to a single question. `dashcard-href.ts` owns the "when".
 *
 * A POPOVER, WHERE METABASE HOVERS A TOOLTIP, and it is a measured choice rather than a shortcut.
 * `components/ui/tooltip.tsx` renders Radix's `Content` with NO `Portal`, so a tooltip lands inside
 * the dashcard body — which is `overflow-hidden` (`DashboardView.tsx`, the dashcard-body div) and
 * is frequently shorter than the tooltip. The description would be clipped on exactly the cards
 * most likely to need one. `Popover` IS portalled (`ui/popover.tsx` wraps Content in
 * `PopoverPrimitive.Portal`) and is already used across the product. If `ui/tooltip.tsx` ever grows
 * a `Portal`, this becomes a hover tooltip and the assertions above still hold.
 */
export function DashboardCardHeader({ text, href, description }: { text: string; href?: string; description?: string }) {
  return (
    <div className="flex items-center gap-1.5 px-4 pt-3 pb-2 text-sm font-medium text-foreground">
      {href ? (
        <Link to={href} className="dash-nodrag truncate rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          {text}
        </Link>
      ) : (
        <span className="truncate">{text}</span>
      )}
      {description && (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" aria-label="Card description" className="dash-nodrag shrink-0 rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              <Info className="h-3.5 w-3.5" aria-hidden />
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-64 p-3 text-sm font-normal whitespace-pre-wrap">
            {description}
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
