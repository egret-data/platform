import type { ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";

/** Metabase's fixed canvas measure. Measured live at an 1800px viewport on localhost:3033: the
 *  dashboard grid stayed 1048px wide, centred. egret's previous 1200px was not a Metabase
 *  number. Exported for the test that pins it to the class below — nothing composes it. */
export const DASHBOARD_FIXED_WIDTH_PX = 1048;

/** The wrapper classes for a dashboard canvas at the given width.
 *
 *  The measure is written out as a LITERAL and not interpolated from DASHBOARD_FIXED_WIDTH_PX:
 *  Tailwind's scanner emits only the classes it can see in the source, so an interpolated
 *  `max-w-[…]` would render as a class name with no rule behind it and the canvas would silently
 *  go full width. A test asserts the two agree. */
export function dashboardWidthClass(width: "fixed" | "full"): string {
  return width === "fixed" ? "mx-auto w-full max-w-[1048px]" : "";
}

/** Ghost icon button, matching the density of the surfaces this menu is rendered on. Callers
 *  that sit in a toolbar with its own icon class pass that class instead. */
const DEFAULT_TRIGGER =
  "inline-flex size-8 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";

interface Props {
  /** Rendered as the "Full width" switch when supplied. Edit mode passes the DRAFT width; view
   *  mode (W4) passes nothing and the switch is absent. */
  width?: "fixed" | "full";
  onChangeWidth?: (next: "fixed" | "full") => void;
  /** Action rows below the switch — W4's Duplicate / Move / Trash / Edit settings / Reset
   *  filters. They must be plain buttons: this is a Popover, not a DropdownMenu (a role="switch"
   *  control cannot nest inside a role="menuitem"), so a <DropdownMenuItem> here would throw for
   *  want of a DropdownMenu.Root ancestor. */
  children?: ReactNode;
  triggerClassName?: string;
}

/** The dashboard PAGE's "…" menu. Deliberately distinct from `DashboardActionsMenu.tsx`, which is
 *  the row menu on the dashboard LIST page and shares nothing with this but a shape.
 *
 *  Mirrors Metabase's ExtraEditButtonsMenu.tsx:27-49 — a popover holding a single "Full width"
 *  switch — and is the component W4 fills with the view-mode actions. */
export function DashboardViewActionsMenu({ width, onChangeWidth, children, triggerClassName }: Props) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={triggerClassName ?? DEFAULT_TRIGGER}
          aria-label="dashboard actions"
          title="More"
        >
          <MoreHorizontal className="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-56 p-1">
        {width && onChangeWidth && (
          <div className="flex items-center justify-between gap-3 rounded-sm px-2 py-1.5">
            <Label htmlFor="dashboard-full-width" className="cursor-pointer font-normal">
              Full width
            </Label>
            {/* The <Label htmlFor> makes the text a click target; the aria-label makes the
                accessible name independent of how the label is wired, so a future restyle of the
                row cannot quietly rename the control out from under its tests. */}
            <Switch
              id="dashboard-full-width"
              aria-label="Full width"
              checked={width === "full"}
              onCheckedChange={(on) => onChangeWidth(on ? "full" : "fixed")}
            />
          </div>
        )}
        {children}
      </PopoverContent>
    </Popover>
  );
}
