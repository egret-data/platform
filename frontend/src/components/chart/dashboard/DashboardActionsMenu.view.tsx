import { useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, Folder, FolderMinus, FolderPlus, MoreHorizontal, RotateCcw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { useDashboardMutations, useDashboards } from "@/api/dashboards";
import { useFolders, useFolderMutations, useSetFolder } from "@/api/folders";
import { FolderNameDialog } from "@/components/explore/FolderNameDialog";
import { ConfirmActionDialog } from "@/components/explore/ConfirmActionDialog";

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
export function dashboardWidthClass(width: "fixed" | "full", sidePanel: "edit" | "info" | null = null): string {
  if (width !== "fixed") return "";
  // The measure is the CANVAS's. A side panel (the 340px edit panel, the 300px info panel, each
  // after a 16px gap) sits beside the canvas, so the wrapper grows by it. Inside the 1048px it
  // left the canvas 692px, under the grid's 753px breakpoint, and the cards flipped to the
  // one-column phone layout while the author was editing them (KI-068).
  if (sidePanel === "edit") return "mx-auto w-full max-w-[1404px]";
  if (sidePanel === "info") return "mx-auto w-full max-w-[1364px]";
  return "mx-auto w-full max-w-[1048px]";
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
  /** Controlled popover state. Optional, and both halves must be supplied together: the edit-mode
   *  toolbar leaves the popover uncontrolled (Radix treats `open === undefined` as uncontrolled,
   *  so W3's call site is byte-for-byte unaffected), while the view-mode rows control it because
   *  two of them have to CLOSE the menu before opening a dialog — a Radix dialog's overlay takes
   *  the outside-pointerdown that would otherwise dismiss the popover mid-interaction. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  triggerClassName?: string;
}

/** The dashboard PAGE's "…" menu. Deliberately distinct from `DashboardActionsMenu.tsx`, which is
 *  the row menu on the dashboard LIST page and shares nothing with this but a shape.
 *
 *  Mirrors Metabase's ExtraEditButtonsMenu.tsx:27-49 — a popover holding a single "Full width"
 *  switch — and is the component W4 fills with the view-mode actions. */
export function DashboardViewActionsMenu({ width, onChangeWidth, children, triggerClassName, open, onOpenChange }: Props) {
  return (
    <Popover open={open} onOpenChange={onOpenChange}>
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

/** Every row in this popover. Plain <button>s, because this is a Popover and not a DropdownMenu —
 *  a <DropdownMenuItem> here throws for want of a DropdownMenu.Root ancestor. */
const ROW =
  "flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50";

interface RowsProps {
  ws: string;
  dashboardId: string;
  /** Whether any of the dashboard's parameters currently carries a value. Mirrors Metabase's
   *  getCanResetFilters (dashboard/selectors.ts:753-756): the row is absent when there is nothing
   *  to reset, rather than present and inert. */
  canResetFilters: boolean;
  onResetFilters: () => void;
}

/** The dashboard PAGE's view-mode "…" menu contents: Reset all filters, the folder rows,
 *  Duplicate, Move to trash. Metabase's DashboardActionMenu.tsx:88-163, minus subscriptions
 *  (declined in design §5 — egret has no such backend), Enter fullscreen (already its own header
 *  button) and Edit settings (egret has no dashboard-settings sidebar to open). Every mutation it
 *  calls has existed and been unreachable from this page since the dashboard route was written —
 *  design §3.2.
 *
 *  IT RENDERS THE MENU rather than being passed to it as children, because two rows have to close
 *  the popover before opening a dialog; hence the controlled `open`.
 *
 *  BEHAVIOUR IS COPIED FROM THE LIST PAGE (Dashboards.tsx:112-121) DELIBERATELY. Duplicate
 *  navigates to the copy; a folder move applies immediately; Move to trash asks first and then
 *  trashes and toasts "Moved to Trash", as the list page does (KI-075): one action must not behave
 *  two ways. "Delete permanently" is not on Metabase's dashboard-page menu and is not on this one. */
export function DashboardViewActionRows({ ws, dashboardId, canResetFilters, onResetFilters }: RowsProps) {
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const [newFolderOpen, setNewFolderOpen] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  const { duplicate, trash } = useDashboardMutations(ws);
  const { data: folders = [] } = useFolders(ws, "dashboards");
  const { create: createFolder } = useFolderMutations(ws, "dashboards");
  const setFolder = useSetFolder(ws, "dashboards");
  // The detail DTO carries no folder_id (bi.rs:360-372), so the folder this dashboard sits in has
  // to come from the list query — already in the react-query cache whenever the user arrived from
  // the list, and a cheap call when they did not.
  const { data: summaries } = useDashboards(ws);
  const currentFolderId = summaries?.find((d) => d.id === dashboardId)?.folder_id ?? null;

  /** Close the menu, then let the mutation run; surface its failure the way every other dashboard
   *  action does (`guard` in Dashboards.tsx:76). */
  const run = (p: Promise<unknown>) => { setOpen(false); p.catch((e) => toast.error(String(e))); };

  return (
    <>
      <DashboardViewActionsMenu open={open} onOpenChange={setOpen}>
        {canResetFilters && (
          <>
            <button type="button" className={ROW} onClick={() => { setOpen(false); onResetFilters(); }}>
              <RotateCcw className="h-4 w-4 shrink-0" aria-hidden /> Reset all filters
            </button>
            <div role="separator" className="my-1 h-px bg-border" />
          </>
        )}
        {folders.map((f) => (
          <button
            key={f.id}
            type="button"
            className={ROW}
            disabled={f.id === currentFolderId}
            onClick={() => run(setFolder.mutateAsync({ id: dashboardId, folder_id: f.id }).then(() => toast.success("Moved")))}
          >
            <Folder className="h-4 w-4 shrink-0" aria-hidden /> Move to &quot;{f.name}&quot;
          </button>
        ))}
        {currentFolderId !== null && (
          <button
            type="button"
            className={ROW}
            onClick={() => run(setFolder.mutateAsync({ id: dashboardId, folder_id: null }).then(() => toast.success("Moved")))}
          >
            <FolderMinus className="h-4 w-4 shrink-0" aria-hidden /> Remove from folder
          </button>
        )}
        <button type="button" className={ROW} onClick={() => { setOpen(false); setNewFolderOpen(true); }}>
          <FolderPlus className="h-4 w-4 shrink-0" aria-hidden /> Move to new folder&hellip;
        </button>
        <button
          type="button"
          className={ROW}
          onClick={() => run(duplicate.mutateAsync(dashboardId).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
        >
          <Copy className="h-4 w-4 shrink-0" aria-hidden /> Duplicate
        </button>
        <div role="separator" className="my-1 h-px bg-border" />
        <button type="button" className={ROW} onClick={() => { setOpen(false); setTrashOpen(true); }}>
          <Trash2 className="h-4 w-4 shrink-0" aria-hidden /> Move to trash
        </button>
      </DashboardViewActionsMenu>
      <ConfirmActionDialog
        open={trashOpen}
        onOpenChange={setTrashOpen}
        title="Move to Trash?"
        description="The dashboard moves to the Trash. You can restore it from there."
        confirmLabel="Move to Trash"
        onConfirm={() => run(trash.mutateAsync(dashboardId).then(() => {
          toast.success("Moved to Trash");
          // Leave the page: `trash`'s onSuccess invalidates only the LIST key
          // (api/dashboards.ts:129-132), so staying here would show a dashboard the server now
          // considers trashed with no banner to say so.
          nav(`/explore/${ws}/dashboards`);
        }))}
      />
      {/* Sibling of the popover, not a child: the popover is closed by the time this opens. Same
          dialog, same labels and same flow as the list page (Dashboards.tsx:259-272). */}
      <FolderNameDialog
        open={newFolderOpen}
        title="Move to new folder"
        submitLabel="Move"
        onOpenChange={setNewFolderOpen}
        onSubmit={(name) =>
          createFolder
            .mutateAsync(name)
            .then((f) => setFolder.mutateAsync({ id: dashboardId, folder_id: f.id }))
            .then(() => toast.success("Moved"))
            .catch((e) => toast.error(String(e)))
        }
      />
    </>
  );
}
