import { Link } from "react-router-dom";
import type { UnreadablePolicy } from "@/api/generated/model";
import { Section } from "@/routes/governance/tabParts";
import { editorUrl, type EditorSubject } from "@/routes/governance/engines/engineUrl";
import { policyTypeWord } from "@/routes/governance/handwritten/handwrittenText";

const byName = <T extends { name: string }>(a: T, b: T) => a.name.localeCompare(b.name);

/**
 * Where an unreadable policy is edited: the first engine downloading it; with
 * none, its profile's page for a resource policy, and no link for a tag
 * service's (its profiles are not named).
 */
function editHref(p: UnreadablePolicy): string | null {
  const engine = [...p.engines].sort(byName)[0];
  const subject: EditorSubject | null = engine
    ? { engineId: engine.id }
    : p.inTagService
      ? null
      : { profile: p.service };
  return subject ? `${editorUrl(subject, p.policyType, p.id)}${p.inTagService ? "?scope=tag" : ""}` : null;
}

/**
 * Hand-written policies whose body could not be read (routes B and D): any of
 * them may name this principal or tag, so the list beside it may be
 * incomplete. Each is named with its parse error and an editor link, as route
 * A's "Could not be matched" rows are; never dropped (spec §4.2.6).
 */
export function UnreadablePolicies({
  policies,
  what,
  adjective = "hand-written",
}: {
  policies: UnreadablePolicy[];
  what: "policy" | "tag policy";
  /** The word before the noun: "hand-written" (Data, People), "manual" (a classification's Manual policies tab). */
  adjective?: string;
}) {
  if (policies.length === 0) return null;
  const n = policies.length;
  const noun = n === 1 ? `${adjective} ${what}` : `${adjective} ${what === "policy" ? "policies" : "tag policies"}`;
  return (
    <Section title="Could not be read">
      <p className="text-sm">{`${n} ${noun} could not be read, so this list may be incomplete.`}</p>
      <ul className="list-disc space-y-0.5 pl-5 text-sm">
        {[...policies].sort(byName).map((p) => {
          const href = editHref(p);
          return (
            <li key={`${p.service}/${p.id}`}>
              {href ? (
                <Link to={href} className="break-all font-medium text-primary hover:underline">
                  {p.name}
                </Link>
              ) : (
                <span className="break-all font-medium">{p.name}</span>
              )}
              <span className="text-muted-foreground">{` · ${policyTypeWord(p.policyType)} · ${p.service}`}</span>
              <span className="block break-all text-xs text-muted-foreground">{p.parseError}</span>
            </li>
          );
        })}
      </ul>
    </Section>
  );
}
