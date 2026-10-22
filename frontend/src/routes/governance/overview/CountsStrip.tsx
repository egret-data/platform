import type { Counts } from "@/api/generated/model";
import { SummaryStrip, type SummaryStripItem } from "@/components/explore/SummaryStrip";

/**
 * What the reader may count (spec §2). A count the server sends as `null` is
 * one the reader may not read: it is left out, never shown as 0. "grants"
 * and "rules" need no qualifier: by the glossary both are egret's, and an
 * engine's own is a manual policy, which is not counted here.
 */
export function CountsStrip({ counts }: { counts: Counts }) {
  const items: SummaryStripItem[] = [];
  const add = (value: number | null | undefined, label: string, singular: string) => {
    if (value !== null && value !== undefined) items.push({ value, label, singular });
  };
  add(counts.catalogs, "catalogs", "catalog");
  add(counts.grants, "grants", "grant");
  add(counts.rules, "rules", "rule");
  add(counts.tags, "classifications", "classification");
  if (items.length === 0) return null;
  return (
    <div data-testid="overview-counts">
      <SummaryStrip items={items} />
    </div>
  );
}
