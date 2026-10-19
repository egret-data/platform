import type { ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { type Tag, useTags } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TabError } from "@/routes/governance/tabParts";
import { TagHeader } from "@/routes/governance/classifications/TagHeader";
import { CLASSIFICATIONS_BASE, type TagTab } from "@/routes/governance/classifications/tagUrl";

/** The tabs in order (spec §5); the first is the absence of `?tab=`. */
export const TAG_TABS: readonly TagTab[] = ["where-used", "clearance", "rules", "views", "handwritten"];

export const TAG_TAB_LABEL: Record<TagTab, string> = {
  "where-used": "Where used",
  clearance: "Clearance",
  rules: "Mask & filter",
  views: "View exposure",
  handwritten: "Hand-written",
};

export const NO_CLASSIFICATION_WITH_ID = "No classification with this id.";

function BackLink() {
  return (
    <Link
      to={CLASSIFICATIONS_BASE}
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" aria-hidden />
      Classifications
    </Link>
  );
}

/** One tab's body. */
function TabBody({ tab }: { tab: TagTab; tag: Tag }) {
  return <p className="text-sm text-muted-foreground">{TAG_TAB_LABEL[tab]}</p>;
}

/**
 * A classification's page (spec §5): `/governance/classifications/:id`,
 * `tag:read`. The tag comes from `GET /tags` (`useTags`, the one key for the
 * list, CL16). A failing list is an error with Retry, never "not found"; an
 * id the list does not hold is "No classification with this id." Each tab
 * owns its states (FE-STATE-1); `?tab=` selects one (FE-PAGE-2, CL17).
 */
export default function TagPage() {
  const { id = "" } = useParams();
  const tagsQ = useTags();
  const [search, setSearch] = useSearchParams();

  let content: ReactNode;
  if (tagsQ.isError) {
    content = (
      <section aria-label="Classification">
        <TabError what="classification" onRetry={() => void tagsQ.refetch()} />
      </section>
    );
  } else if (!tagsQ.data) {
    content = (
      <section aria-label="Classification">
        <ListSkeleton rows={4} cols={3} />
      </section>
    );
  } else {
    const tag = tagsQ.data.find((t) => t.id === id);
    if (!tag) {
      content = (
        <section aria-label="Classification">
          <Alert>
            <AlertDescription>{NO_CLASSIFICATION_WITH_ID}</AlertDescription>
          </Alert>
        </section>
      );
    } else {
      const asked = search.get("tab");
      const value = TAG_TABS.find((t) => t === asked) ?? TAG_TABS[0];
      const select = (next: string) =>
        setSearch((prev) => {
          const params = new URLSearchParams(prev);
          if (next === TAG_TABS[0]) params.delete("tab");
          else params.set("tab", next);
          return params;
        });
      content = (
        <>
          <TagHeader tag={tag} />
          <Tabs value={value} onValueChange={select} className="min-w-0">
            {/* Wraps, so every tab is in view at 390 px. */}
            <TabsList className="h-auto max-w-full flex-wrap justify-start">
              {TAG_TABS.map((t) => (
                <TabsTrigger key={t} value={t}>
                  {TAG_TAB_LABEL[t]}
                </TabsTrigger>
              ))}
            </TabsList>
            {TAG_TABS.map((t) => (
              <TabsContent key={t} value={t} className="mt-4">
                {t === value ? <TabBody tab={t} tag={tag} /> : null}
              </TabsContent>
            ))}
          </Tabs>
        </>
      );
    }
  }

  return (
    <div className="space-y-6 p-4">
      <BackLink />
      {content}
    </div>
  );
}
