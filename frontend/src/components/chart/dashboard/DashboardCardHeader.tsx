import type { ReactNode, Ref } from "react";
import { Info } from "lucide-react";
import { Link } from "react-router-dom";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";

/**
 * A chart card's caption: the title, the description beside it, and — since W7 — any dashboard
 * filters the card hosts, at the trailing edge of the same row.
 *
 * Metabase's `ChartCaption` reads exactly these two settings — `card.title` and `card.description`
 * (`visualizations/components/ChartCaption.tsx:40-46`) — and makes the title a link when the card
 * resolves to a single question. `dashcard-href.ts` owns the "when". The filter list is where
 * Metabase puts it too: handed to the visualization as `actionButtons` and drawn in the title row
 * (`DashCardVisualization.tsx:468-487`).
 *
 * A POPOVER, WHERE METABASE HOVERS A TOOLTIP, and it is a measured choice rather than a shortcut.
 * `components/ui/tooltip.tsx` renders Radix's `Content` with NO `Portal`, so a tooltip lands inside
 * the dashcard body — which is `overflow-hidden` (`DashboardView.tsx`, the dashcard-body div) and
 * is frequently shorter than the tooltip. The description would be clipped on exactly the cards
 * most likely to need one. `Popover` IS portalled (`ui/popover.tsx` wraps Content in
 * `PopoverPrimitive.Portal`) and is already used across the product. If `ui/tooltip.tsx` ever grows
 * a `Portal`, this becomes a hover tooltip and the assertions above still hold.
 *
 * THE ROW RENDERS WITH AN EMPTY TITLE when `text` is blank but filters are present. `title: ""` is
 * how an author hides the caption, and the filters still have to live somewhere; without this the
 * card would claim a filter that is on neither the top bar nor the board.
 */
export function DashboardCardHeader({
  text, href, description, filters, containerRef, titleRef, gutterClass = "pr-4",
}: {
  text: string;
  href?: string;
  description?: string;
  /** The card's hosted-filter list, already measured by `useCollapsedParameterList`. */
  filters?: ReactNode;
  /** The row, whose laid-out width the collision test compares against. */
  containerRef?: Ref<HTMLDivElement>;
  /** The TRUNCATING title box, read with `scrollWidth` — see `useCollapsedParameterList`. */
  titleRef?: Ref<HTMLDivElement>;
  /** Right padding reserving the hover cluster; from `inlineFilterGutterClass`. */
  gutterClass?: string;
}) {
  return (
    <div ref={containerRef} data-testid="dashcard-header"
      className={`flex items-center gap-1.5 pl-4 pt-3 pb-2 ${gutterClass}`}>
      <div ref={titleRef} className="min-w-0 truncate text-sm font-medium text-foreground">
        {href ? (
          <Link to={href} className="dash-nodrag rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
            {text}
          </Link>
        ) : (
          <span>{text}</span>
        )}
      </div>
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
      {/* `ml-auto` rather than a spacer: the title must keep its natural width (so `scrollWidth`
          means what the collision test thinks it means) and the filters must sit at the trailing
          edge, which is where Metabase draws them. */}
      {filters && <div className="ml-auto flex min-w-0 items-center">{filters}</div>}
    </div>
  );
}
