import { Plus, Type, Link2, LayoutGrid, Filter, ChevronDown } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { layoutOptions, type SectionLayout } from "./dashboard-sections";
import { DashboardViewActionsMenu } from "./DashboardActionsMenu.view";

interface Props {
  onAddChart: () => void;
  onAddText: () => void;
  onAddHeading: () => void;
  onAddLink: () => void;
  onAddSection: (layout: SectionLayout) => void;
  onAddFilter: () => void;
  /** The DRAFT width — the switch reflects what will be saved, not what is stored. */
  width: "fixed" | "full";
  onChangeWidth: (next: "fixed" | "full") => void;
}

const ICON = "inline-flex size-8 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";

// Mini layout sketch with visible gaps so each card reads as a separate block.
const PREVIEW_COLS = 24;
const PREVIEW_WIDTH = 64; // px
const PREVIEW_ROW_PX = 3; // px per grid row
const COL_PX = PREVIEW_WIDTH / PREVIEW_COLS;
const GAP = 1; // px gap inset between blocks

function SectionLayoutPreview({ layout }: { layout: SectionLayout }) {
  const cards = layout.build({ col: 0, row: 0 });
  const rows = Math.max(1, ...cards.map((c) => c.grid_row + c.size_y));
  return (
    <div className="relative shrink-0 overflow-hidden rounded border border-border bg-card" style={{ width: PREVIEW_WIDTH, height: rows * PREVIEW_ROW_PX }}>
      {cards.map((c, i) => (
        <div
          key={i}
          className={`absolute rounded-[1px] ${c.card_type === "heading" ? "bg-foreground/30" : "bg-primary/70"}`}
          style={{
            left: c.grid_col * COL_PX + GAP / 2,
            top: c.grid_row * PREVIEW_ROW_PX + GAP / 2,
            width: Math.max(1, c.size_x * COL_PX - GAP),
            height: Math.max(1, c.size_y * PREVIEW_ROW_PX - GAP),
          }}
        />
      ))}
    </div>
  );
}

/** Edit-mode add controls as a compact icon group (placed on the title row). */
export function DashboardEditToolbar({ onAddChart, onAddText, onAddHeading, onAddLink, onAddSection, onAddFilter, width, onChangeWidth }: Props) {
  return (
    <div className="flex items-center gap-1">
      <button type="button" className={ICON} aria-label="add chart" title="Add a chart" onClick={onAddChart}>
        <Plus className="h-4 w-4" />
      </button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ICON} aria-label="add text or heading" title="Add a heading or text">
            <Type className="h-4 w-4" />
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" onCloseAutoFocus={(e) => e.preventDefault()}>
          <DropdownMenuItem onClick={onAddHeading}>Heading</DropdownMenuItem>
          <DropdownMenuItem onClick={onAddText}>Text</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <button type="button" className={ICON} aria-label="add link" title="Add a link" onClick={onAddLink}>
        <Link2 className="h-4 w-4" />
      </button>

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ICON} aria-label="add section" title="Add a section">
            <LayoutGrid className="h-4 w-4" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {layoutOptions.map((layout) => (
            <DropdownMenuItem key={layout.id} className="gap-2" onClick={() => onAddSection(layout)}>
              <SectionLayoutPreview layout={layout} />
              <span>{layout.label}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>

      <button type="button" className={ICON} aria-label="add filter" title="Add a filter" onClick={onAddFilter}>
        <Filter className="h-4 w-4" />
      </button>

      <DashboardViewActionsMenu width={width} onChangeWidth={onChangeWidth} triggerClassName={ICON} />
    </div>
  );
}
