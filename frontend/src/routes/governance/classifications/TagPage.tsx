import { useRef, useState, type ReactNode } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ArrowLeft, Trash2 } from "lucide-react";
import { type Tag, useTags } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TabError } from "@/routes/governance/tabParts";
import { DeleteTagDialog } from "@/routes/governance/classifications/DeleteTagDialog";
import { TagHeader } from "@/routes/governance/classifications/TagHeader";
import { useCanManageTags } from "@/routes/governance/classifications/tagParts";
import { ClearanceTab } from "@/routes/governance/classifications/tabs/ClearanceTab";
import { HandWrittenTagTab } from "@/routes/governance/classifications/tabs/HandWrittenTagTab";
import { RulesTab } from "@/routes/governance/classifications/tabs/RulesTab";
import { ViewsTab } from "@/routes/governance/classifications/tabs/ViewsTab";
import { WhereUsedTab } from "@/routes/governance/classifications/tabs/WhereUsedTab";
import { CLASSIFICATIONS_BASE, type TagTab } from "@/routes/governance/classifications/tagUrl";
import { TAG_TAB_LABEL, TERM } from "@/routes/governance/glossary";

/** The tabs in order (spec §5), labelled by the glossary's `TAG_TAB_LABEL`; the first is the absence of `?tab=`. */
export const TAG_TABS: readonly TagTab[] = ["where-used", "clearance", "rules", "views", "handwritten"];

export const NO_CLASSIFICATION_WITH_ID = "No classification with this id.";

function BackLink() {
  return (
    <Link
      to={CLASSIFICATIONS_BASE}
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" aria-hidden />
      {TERM.classifications}
    </Link>
  );
}

/** Delete, for `tag:manage` or platform admin, through the consequence dialog (CL14). */
function DeleteControl({ tag }: { tag: Tag }) {
  const canManage = useCanManageTags();
  const [open, setOpen] = useState(false);
  const opener = useRef<HTMLButtonElement | null>(null);
  if (!canManage) return null;
  return (
    <>
      <Button
        ref={opener}
        size="sm"
        variant="outline"
        className="text-destructive hover:text-destructive"
        onClick={() => setOpen(true)}
      >
        <Trash2 className="mr-1 h-4 w-4" aria-hidden />
        Delete
      </Button>
      <DeleteTagDialog tag={tag} open={open} onOpenChange={setOpen} where="this page" opener={opener} />
    </>
  );
}

/** One tab's body; each owns its loading, empty and error states. */
function TabBody({ tab, tag }: { tab: TagTab; tag: Tag }) {
  switch (tab) {
    case "where-used":
      return <WhereUsedTab tagId={tag.id} />;
    case "clearance":
      return <ClearanceTab tag={tag} />;
    case "rules":
      return <RulesTab tag={tag} />;
    case "views":
      return <ViewsTab tag={tag} />;
    case "handwritten":
      return <HandWrittenTagTab tagId={tag.id} />;
  }
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
      <section aria-label={TERM.classification}>
        <TabError what="classification" onRetry={() => void tagsQ.refetch()} />
      </section>
    );
  } else if (!tagsQ.data) {
    content = (
      <section aria-label={TERM.classification}>
        <ListSkeleton rows={4} cols={3} />
      </section>
    );
  } else {
    const tag = tagsQ.data.find((t) => t.id === id);
    if (!tag) {
      content = (
        <section aria-label={TERM.classification}>
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
          <TagHeader tag={tag} actions={<DeleteControl tag={tag} />} />
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
