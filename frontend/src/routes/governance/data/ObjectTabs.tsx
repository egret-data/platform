import type { ReactNode } from "react";
import { useSearchParams } from "react-router-dom";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export type TabId = "overview" | "permissions" | "masks" | "lineage" | "details";

export type ObjectLevel = "catalog" | "schema" | "object";

const LABELS: Record<TabId, string> = {
  overview: "Overview",
  permissions: "Permissions",
  masks: "Masks & filters",
  lineage: "Lineage",
  details: "Details",
};

/** The tabs of each level, in order (spec §2.2's table). A view or MV has a table's tabs. */
export const TABS_BY_LEVEL: Record<ObjectLevel, readonly TabId[]> = {
  catalog: ["overview", "permissions", "masks", "details"],
  schema: ["overview", "permissions", "masks"],
  object: ["overview", "permissions", "masks", "lineage", "details"],
};

/**
 * An object's tabs (Radix Tabs: arrow keys move and select, Home/End jump).
 *
 * The selected tab is `?tab=` (FE-PAGE-2), so a link opens it and Back walks
 * it; Overview is the absence of the parameter. A `tab` this level does not
 * have, or one that is not a tab at all, shows Overview. Changing tabs keeps
 * the rest of the search (`?column=`). Only the selected panel is mounted, so
 * a tab reads its data when it is opened.
 */
export function ObjectTabs({ level, panels }: { level: ObjectLevel; panels: Record<TabId, ReactNode> }) {
  const [search, setSearch] = useSearchParams();
  const tabs = TABS_BY_LEVEL[level];
  const asked = search.get("tab");
  const value = tabs.find((t) => t === asked) ?? "overview";

  const select = (next: string) =>
    setSearch((prev) => {
      const params = new URLSearchParams(prev);
      if (next === "overview") params.delete("tab");
      else params.set("tab", next);
      return params;
    });

  return (
    <Tabs value={value} onValueChange={select} className="min-w-0">
      <TabsList className="max-w-full justify-start overflow-x-auto">
        {tabs.map((t) => (
          <TabsTrigger key={t} value={t}>
            {LABELS[t]}
          </TabsTrigger>
        ))}
      </TabsList>
      {tabs.map((t) => (
        <TabsContent key={t} value={t} className="mt-4">
          {panels[t]}
        </TabsContent>
      ))}
    </Tabs>
  );
}
