import type { Counts } from "@/api/generated/model";
import { SummaryStrip, type SummaryStripItem } from "@/components/explore/SummaryStrip";

/**
 * What the reader may count (spec §2). A count the server sends as `null` is
 * one the reader may not read: it is left out, never shown as 0. "egret
 * grants/rules": hand-written engine policies are not counted, and a label
 * never claims more than it counts.
 */
export function CountsStrip({ counts }: { counts: Counts }) {
  const items: SummaryStripItem[] = [];
  const add = (value: number | null | undefined, label: string, singular: string) => {
    if (value !== null && value !== undefined) items.push({ value, label, singular });
  };
  add(counts.catalogs, "catalogs", "catalog");
  add(counts.grants, "egret grants", "egret grant");
  add(counts.rules, "egret rules", "egret rule");
  add(counts.tags, "classifications", "classification");
  if (items.length === 0) return null;
  return (
    <div data-testid="overview-counts">
      <SummaryStrip items={items} />
    </div>
  );
}
