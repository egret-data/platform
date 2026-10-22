import * as React from "react"
import * as TabsPrimitive from "@radix-ui/react-tabs"
import { cn } from "@/lib/utils"

/**
 * Two tab styles, no more (FE-PAGE-9, KI-216):
 *
 * - `pill` (the default) — a grey strip of pills, for the SECTIONS of a page:
 *   a dataset's Columns / Metrics, an alert's General / Condition / …, a
 *   catalog's eight tabs, a flow's Edit / Revisions / Executions / Triggers.
 * - `underline` — a line under the active label, for PANELS inside an editor or
 *   a tool: the chart editor's Data / Display, SQL Lab's Results / History, an
 *   endpoint's Query / Config / ….
 *
 * The variant is set once on `TabsList` and read by every `TabsTrigger` inside
 * it. A tab strip made of router links (an endpoint's sections) uses the same
 * classes through `tabsListClass` / `tabsTriggerClass`: the trigger classes
 * style `aria-current="page"` exactly like Radix's `data-state="active"`.
 *
 * An icon in a trigger sits `gap-1.5` from its label at `size-4`; it used to
 * touch the label (the alert editor, the catalog detail).
 */
export type TabsVariant = "pill" | "underline"

const LIST_CLASS: Record<TabsVariant, string> = {
  pill: "inline-flex h-10 items-center justify-center rounded-md bg-muted p-1 text-muted-foreground",
  underline:
    "flex h-auto w-full items-end justify-start gap-1 overflow-x-auto border-b border-border bg-transparent p-0 text-muted-foreground",
}

const TRIGGER_BASE =
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap text-sm font-medium transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0"

const TRIGGER_CLASS: Record<TabsVariant, string> = {
  pill: "rounded-sm px-3 py-1.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm aria-[current=page]:bg-background aria-[current=page]:text-foreground aria-[current=page]:shadow-sm",
  underline:
    "-mb-px shrink-0 rounded-none border-b-2 border-transparent px-3 py-2 hover:border-border hover:text-foreground data-[state=active]:border-primary data-[state=active]:font-semibold data-[state=active]:text-foreground aria-[current=page]:border-primary aria-[current=page]:font-semibold aria-[current=page]:text-foreground",
}

/** The tab list's classes, for a strip that is not a Radix `TabsList`. */
export const tabsListClass = (variant: TabsVariant = "pill") => LIST_CLASS[variant]
/** A tab's classes, for a tab that is not a Radix `TabsTrigger` (a `NavLink`). */
export const tabsTriggerClass = (variant: TabsVariant = "pill") =>
  `${TRIGGER_BASE} ${TRIGGER_CLASS[variant]}`

const VariantContext = React.createContext<TabsVariant>("pill")

const Tabs = TabsPrimitive.Root

const TabsList = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.List>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.List> & { variant?: TabsVariant }
>(({ className, variant = "pill", ...props }, ref) => (
  <VariantContext.Provider value={variant}>
    <TabsPrimitive.List
      ref={ref}
      data-variant={variant}
      className={cn(LIST_CLASS[variant], className)}
      {...props}
    />
  </VariantContext.Provider>
))
TabsList.displayName = TabsPrimitive.List.displayName

const TabsTrigger = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>
>(({ className, ...props }, ref) => {
  const variant = React.useContext(VariantContext)
  return (
    <TabsPrimitive.Trigger
      ref={ref}
      className={cn(TRIGGER_BASE, TRIGGER_CLASS[variant], className)}
      {...props}
    />
  )
})
TabsTrigger.displayName = TabsPrimitive.Trigger.displayName

const TabsContent = React.forwardRef<
  React.ElementRef<typeof TabsPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Content>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    className={cn(
      "mt-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      className
    )}
    {...props}
  />
))
TabsContent.displayName = TabsPrimitive.Content.displayName

export { Tabs, TabsList, TabsTrigger, TabsContent }
