import { useId } from "react";
import { Link } from "react-router-dom";
import { TriangleAlert } from "lucide-react";
import { usePermListRules } from "@/api/generated/perm/perm";
import { orphanRules } from "@/routes/governance/ruleMatch";
import { TabError } from "@/routes/governance/tabParts";

/**
 * The rules whose tag no longer exists (DG redesign #2, P9 and §5): deleting a
 * tag deletes the tag only, and its rules stay behind with nowhere else to be
 * listed (a tag page lists the rules of a tag that exists). Each links to its
 * rule page, where it can be edited onto another tag or deleted. Nothing is
 * shown when there is none; a failed read says so. `tagKeys` is the tag list
 * the page holds, or null while it is not known.
 *
 * Reads `GET /perm/rules`, so the page renders this only for a `policy:read`
 * holder.
 */
export default function OrphanRulesCallout({ tagKeys }: { tagKeys: ReadonlySet<string> | null }) {
  const headingId = useId();
  const rulesQ = usePermListRules({ query: { retry: false } });
  if (rulesQ.isError) return <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  if (rulesQ.data?.status !== 200) return null;
  const orphans = orphanRules(rulesQ.data.data, tagKeys).sort((a, b) => a.name.localeCompare(b.name));
  if (orphans.length === 0) return null;

  return (
    <section aria-labelledby={headingId} className="space-y-2 rounded-lg border border-border px-4 py-3 text-sm">
      <h2 id={headingId} className="flex items-center gap-2 font-semibold">
        <TriangleAlert aria-hidden className="size-4 shrink-0" />
        {orphans.length === 1
          ? "1 rule selects a tag that no longer exists"
          : `${orphans.length} rules select a tag that no longer exists`}
      </h2>
      <p className="text-muted-foreground">
        Deleting a tag leaves its rules in place. Open each one to move it onto another tag, or delete it.
      </p>
      <ul role="list" className="space-y-1">
        {orphans.map((r) => (
          <li key={r.id}>
            <Link to={`/governance/rules/${encodeURIComponent(r.id)}`} className="font-medium text-primary hover:underline">
              {r.name}
            </Link>
            {r.selector.type === "tag" ? (
              <span className="text-muted-foreground">
                {" · tag key "}
                <span className="font-mono">{r.selector.key}</span>
              </span>
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}
