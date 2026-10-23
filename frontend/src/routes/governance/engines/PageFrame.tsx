import type { ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Info } from "lucide-react";
import type { EngineGrantsResponse, EngineRef, ProfileDto, Reach } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ENGINES_BASE, type EditorSubject, type EngineTab } from "@/routes/governance/engines/engineUrl";
import type { ProfilePolicies } from "@/routes/governance/engines/profilePolicies";
import { DetailHeader } from "@/components/layout/DetailHeader";

/**
 * The frame the engine page and the profile page share (spec §6): a header
 * region, the banners, and the tab strip whose selection is `?tab=`
 * (FE-PAGE-2). A tab named in `?tab=` that the page does not show falls back to
 * the first; the first tab is the absence of the parameter.
 */

export const TAB_LABEL: Record<EngineTab, string> = {
  "engine-permissions": "Engine permissions",
  policies: "Policies",
  overlaps: "Overlaps",
  migrate: "Migrate",
  plugin: "Plugin",
  advanced: "Advanced",
};

/** An engine page's tabs in order (spec §6); Migrate only with a hand-written policy (EP7). */
export const ENGINE_TABS: readonly EngineTab[] = [
  "engine-permissions",
  "policies",
  "overlaps",
  "migrate",
  "plugin",
  "advanced",
];

/** A profile page's tabs: the engine page's minus Engine permissions and Plugin (EP13). */
export const PROFILE_TABS: readonly EngineTab[] = ["policies", "overlaps", "migrate", "advanced"];

const TYPE_LABEL: Record<string, string> = { trino: "Trino", starrocks: "StarRocks", tag: "Tag", risingwave: "RisingWave" };
const ORIGIN_LABEL: Record<string, string> = {
  provisioned: "Provisioned",
  external: "External",
  manual: "Manual",
  generated: "Generated",
};

export const typeLabel = (t: string) => TYPE_LABEL[t] ?? t;
export const originLabel = (o: string) => ORIGIN_LABEL[o] ?? o;

/** The profile a page is about, as its tabs need it. */
export interface FrameProfile {
  name: string;
  /** `trino` | `starrocks` | …; the engine's type on an engine page. */
  type: string;
  /** The profile list's row, once it answered (origin, tag service, counts). */
  dto: ProfileDto | null;
  tagService: string | null;
}

/**
 * What every tab body receives. On an engine page `engine`, `grants` and
 * `reach` come from `GET /perm/engines/{id}/grants`; on a profile page they
 * are null. `profile` is null for an engine with no profile.
 */
export interface TabContext {
  subject: EditorSubject;
  engine: EngineRef | null;
  grants: EngineGrantsResponse | null;
  reach: Reach | null;
  profile: FrameProfile | null;
  policies: ProfilePolicies;
}

/**
 * The page's header: the way back to Engine & policy, its `h1` and a line of
 * facts, named for the page's subject. The shared `DetailHeader` (KI-224).
 */
export function FrameHeader({
  label,
  title,
  facts,
  actions,
}: {
  label: "Engine" | "Profile";
  title: ReactNode;
  facts: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section aria-label={label}>
      <DetailHeader
        className=""
        back={{ to: ENGINES_BASE, label: "Engine & policy" }}
        title={title}
        meta={<div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground">{facts}</div>}
        actions={actions}
      />
    </section>
  );
}

export function TypeBadge({ type }: { type: string }) {
  return <Badge variant="secondary">{typeLabel(type)}</Badge>;
}

export function OriginBadge({ origin }: { origin: string }) {
  return <Badge variant="outline">{originLabel(origin)}</Badge>;
}

/** A labelled fact of the header ("Profile lake-tr"). */
export function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1">
      {label}
      <span className="text-foreground">{children}</span>
    </span>
  );
}

/** EP14: a legacy shared profile reaches every engine bound to it. */
export function sharedSentence(profile: string, others: string[]): string {
  return `Profile ${profile} is shared with engines ${others.join(", ")}. Everything on this page applies to all of them.`;
}

/** EP19: what the plugin version and the Plugin tab say without `engine:read`. */
export const NEEDS_ENGINE_READ = "Plugin state needs the engine:read permission.";

export const EXTERNAL_SENTENCE =
  "egret compiles this engine's policies but cannot see whether the external engine downloads them.";

export function Banner({ children }: { children: ReactNode }) {
  return (
    <Alert>
      <Info className="h-4 w-4" aria-hidden />
      <AlertDescription>{children}</AlertDescription>
    </Alert>
  );
}

/** "No engine with this id." / "No policy profile named X.", with the way back to the index. */
export function NotFound({ sentence }: { sentence: string }) {
  return (
    <Alert>
      <AlertDescription>
        <p>
          <span>{sentence}</span>
          {" Back to "}
          <Link to={ENGINES_BASE} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
            Engine & policy
          </Link>
          .
        </p>
      </AlertDescription>
    </Alert>
  );
}

/** The tab strip and the selected tab's body; only the selected body renders. */
export function FrameTabs({
  tabs,
  render,
  badges,
}: {
  tabs: readonly EngineTab[];
  render: (tab: EngineTab) => ReactNode;
  /** A count shown on a tab's trigger (Overlaps). */
  badges?: Partial<Record<EngineTab, number>>;
}) {
  const [search, setSearch] = useSearchParams();
  const asked = search.get("tab");
  const value = tabs.find((t) => t === asked) ?? tabs[0];

  const select = (next: string) =>
    setSearch((prev) => {
      const params = new URLSearchParams(prev);
      if (next === tabs[0]) params.delete("tab");
      else params.set("tab", next);
      return params;
    });

  return (
    <Tabs value={value} onValueChange={select} className="min-w-0">
      {/* Wraps, so every tab is in view at 390 px. */}
      <TabsList className="h-auto max-w-full flex-wrap justify-start">
        {tabs.map((t) => (
          <TabsTrigger key={t} value={t} className="gap-1.5">
            {TAB_LABEL[t]}
            {badges?.[t] !== undefined ? (
              <Badge variant="secondary" className="px-1.5 py-0 tabular-nums">
                {badges[t]}
              </Badge>
            ) : null}
          </TabsTrigger>
        ))}
      </TabsList>
      {tabs.map((t) => (
        <TabsContent key={t} value={t} className="mt-4">
          {t === value ? render(t) : null}
        </TabsContent>
      ))}
    </Tabs>
  );
}
