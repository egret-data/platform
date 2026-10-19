import { Ban } from "lucide-react";
import type { ItemSummary } from "@/api/generated/model";
import { BUCKET_WORD, namesNobodySentence } from "@/routes/governance/handwritten/handwrittenText";

/**
 * A hand-written policy's items (spec §4.1 `ItemSummary`): the bucket (a deny
 * marked with an icon and the word), who it names, and what it does (accesses,
 * mask type or filter). A group or role it names is followed by Q1's sentence,
 * because no engine resolves it; `public` and `{USER}` read "Everyone".
 */
export function HandWrittenItems({ items }: { items: ItemSummary[] }) {
  if (items.length === 0) return <span className="text-muted-foreground">No items</span>;
  return (
    <ul className="space-y-1.5" aria-label="Items">
      {items.map((item, i) => {
        const who = [
          ...(item.everyone ? ["Everyone"] : []),
          ...item.users,
          ...item.groups.map((g) => `group ${g.name}`),
          ...item.roles.map((r) => `role ${r.name}`),
        ];
        const what =
          item.bucket === "mask"
            ? item.maskType
              ? `mask ${item.maskType}`
              : ""
            : item.bucket === "row_filter"
              ? item.rowFilter
                ? `filter ${item.rowFilter}`
                : ""
              : item.accesses.join(", ");
        const nobody = [
          ...item.groups.filter((g) => !g.onEngines).map((g) => namesNobodySentence("group", g.name)),
          ...item.roles.filter((r) => !r.onEngines).map((r) => namesNobodySentence("role", r.name)),
        ];
        return (
          <li key={i} className="text-sm">
            <span className="inline-flex items-center gap-1 font-medium">
              {item.bucket === "deny" ? <Ban className="h-3.5 w-3.5 text-destructive" aria-hidden /> : null}
              {BUCKET_WORD[item.bucket]}
            </span>
            {": "}
            <span>{who.length > 0 ? who.join(", ") : "nobody named"}</span>
            {what ? <span className="text-muted-foreground">{` — ${what}`}</span> : null}
            {nobody.map((s) => (
              <span key={s} className="block text-xs text-muted-foreground">
                {s}
              </span>
            ))}
          </li>
        );
      })}
    </ul>
  );
}
