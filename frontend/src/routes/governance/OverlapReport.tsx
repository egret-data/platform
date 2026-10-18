import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";

import { usePermListOverlaps } from "@/api/generated/perm/perm";
import type { StoredMember, StoredOverlap } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { listProfiles, type Profile } from "@/api/policies";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

/**
 * Where an operator reads the mask and row-filter pairs that are ALREADY
 * stored and may decide one request two ways (SP4d-4 Task 4).
 *
 * A write refuses only the overlaps it creates, so a pair stored earlier
 * blocks nothing and would otherwise stay invisible: this is its report, shown
 * on the Policies landing (every profile) and on a profile's own page
 * (`serviceName`, so a hand-written pair on a manual profile is visible where
 * it is edited). Nothing is shown when there is none.
 *
 * The API reports a pair once per profile whose download serves it, so a tag
 * service shared by two profiles yields the same pair twice. The report counts
 * DISTINCT pairs and names every profile that serves each.
 */
interface Pair {
  overlap: StoredOverlap;
  /** Every service whose download serves this pair, in the API's order. */
  services: string[];
}

function distinctPairs(all: StoredOverlap[]): Pair[] {
  const byKey = new Map<string, Pair>();
  for (const o of all) {
    const key = [o.kind, o.coordinate, ...o.rules.map((r) => r.id ?? r.policyId ?? r.name)].join("|");
    const seen = byKey.get(key);
    if (seen) {
      if (!seen.services.includes(o.serviceName)) seen.services.push(o.serviceName);
    } else {
      byKey.set(key, { overlap: o, services: [o.serviceName] });
    }
  }
  return [...byKey.values()];
}

export function OverlapReport({ serviceName }: { serviceName?: string }) {
  const q = usePermListOverlaps({ query: { retry: false } });
  // Profiles resolve a service name to the profile whose editor opens its
  // policies: a tag service has no editor route of its own.
  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });

  if (q.isLoading) return null;
  if (q.isError) {
    // A reader without the permission is told so by the page itself; a second
    // alert here would say the same thing twice.
    if (q.error instanceof ApiError && q.error.status === 403) return null;
    return (
      <Alert variant="destructive">
        <AlertDescription>
          Could not check for overlapping rules. Reload the page to try again.
        </AlertDescription>
      </Alert>
    );
  }
  const all: StoredOverlap[] = q.data?.status === 200 ? q.data.data : [];
  const pairs = distinctPairs(serviceName ? all.filter((o) => o.serviceName === serviceName) : all);
  if (pairs.length === 0) return null;

  // The profile whose editor opens `name`'s policies, or null when none does
  // (a tag service no profile names). The page's own profile is trusted.
  const editorProfile = (name: string): string | null => {
    if (name === serviceName) return name;
    const list = profiles.data ?? [];
    return (
      list.find((p) => p.name === name && p.type !== "tag")?.name ??
      list.find((p) => p.tag_service === name)?.name ??
      null
    );
  };

  return (
    <Alert aria-labelledby="overlap-report-title">
      <TriangleAlert className="h-4 w-4" />
      <div>
        <AlertTitle id="overlap-report-title">
          {pairs.length === 1
            ? "1 pair of rules overlaps"
            : `${pairs.length} pairs of rules overlap`}
        </AlertTitle>
        <AlertDescription className="space-y-3">
          <p>
            Each pair can decide one request two different ways. They were stored before overlaps
            were refused, so they block nothing, but the engine&apos;s choice between them is not
            something either one says. Break each pair to fix it.
          </p>
          <ul className="space-y-3">
            {pairs.map(({ overlap: o, services }) => {
              const editor = services.map(editorProfile).find((n): n is string => n !== null) ?? null;
              return (
                <li
                  key={`${o.kind}|${o.coordinate}|${o.rules.map((r) => r.policyId ?? r.id ?? r.name).join("|")}`}
                  className="space-y-1"
                >
                  <div>
                    <span className="font-medium">
                      {o.kind === "mask" ? "Mask" : "Row filter"}
                    </span>{" "}
                    on <code>{o.coordinate}</code>
                    {!serviceName && (
                      <>
                        , {services.length === 1 ? "profile" : "profiles"}{" "}
                        {services.map((n, i) => {
                          const profile = editorProfile(n);
                          return (
                            <span key={n}>
                              {i > 0 && ", "}
                              {profile ? (
                                <Link
                                  className="text-primary hover:underline"
                                  to={`/governance/policies/${encodeURIComponent(profile)}`}
                                >
                                  {n}
                                </Link>
                              ) : (
                                n
                              )}
                            </span>
                          );
                        })}
                      </>
                    )}
                  </div>
                  <div>
                    {o.rules.map((m, i) => (
                      <span key={`${m.policyId ?? m.id ?? m.name}`}>
                        {i > 0 && " and "}
                        <MemberLink member={m} kind={o.kind} editor={editor} />
                      </span>
                    ))}
                    {o.servedToday && <> — today the engine serves {o.servedToday.name}.</>}
                  </div>
                  <details>
                    <summary className="cursor-pointer">How to fix</summary>
                    <p className="mt-1">
                      Delete one of the two, or narrow one so they no longer select a common
                      resource. If either rule selects by classification tag, unassigning the tag
                      from the column where they meet also breaks the pair. An edit that narrows a
                      member but still meets the other is refused as a new overlap.
                    </p>
                    {o.rules.some((m) => !m.id && m.policyId != null && editor) && (
                      <ul className="mt-1 list-disc pl-5">
                        {o.rules
                          .filter((m) => !m.id && m.policyId != null)
                          .map((m) => (
                            <li key={m.policyId}>
                              <Link
                                className="text-primary hover:underline"
                                to={editorPath(editor as string, o.kind, m.policyId as number)}
                              >
                                Switch off policy {m.name}
                              </Link>{" "}
                              with the Enabled switch in its editor.
                            </li>
                          ))}
                      </ul>
                    )}
                    <p className="mt-1 text-muted-foreground">{o.sentence}</p>
                  </details>
                </li>
              );
            })}
          </ul>
        </AlertDescription>
      </div>
    </Alert>
  );
}

const editorPath = (profile: string, kind: string, policyId: number) =>
  `/governance/policies/${encodeURIComponent(profile)}/${kind === "mask" ? "masking" : "rowfilter"}/${policyId}`;

/**
 * A compiled member opens its rule; a hand-written one opens its policy in the
 * editor of the profile that serves it. With no such profile (a tag service on
 * its own) the editor has no route, so the member is plain text.
 */
function MemberLink({
  member,
  kind,
  editor,
}: {
  member: StoredMember;
  kind: string;
  editor: string | null;
}) {
  const label = member.id ? `rule ${member.name}` : `policy ${member.name}`;
  const to = member.id
    ? `/governance/rules/${encodeURIComponent(member.id)}`
    : member.policyId != null && editor
      ? editorPath(editor, kind, member.policyId)
      : null;
  return to ? (
    <Link className="font-medium text-primary hover:underline" to={to}>
      {label}
    </Link>
  ) : (
    <span className="font-medium">{label}</span>
  );
}
