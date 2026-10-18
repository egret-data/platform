import { useState, type ReactNode } from "react";
import { useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { RuleView } from "@/api/generated/model";
import { getPermListRulesQueryKey, usePermDeleteRule, usePermListRules } from "@/api/generated/perm/perm";
import { useTags } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { OpenInDataLink, scopeToDataLink, useCatalogNameById, useWorkspaceNames } from "@/routes/governance/openInData";
import { formatRuleKind, formatSelector } from "@/routes/governance/permFormat";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TabError } from "@/routes/governance/tabParts";
import { Detail, ItemHeader, ItemMissing } from "@/routes/governance/people/itemParts";

/** What a reader without `policy:update` reads instead of Edit and Delete. Also this module's bundle marker. */
export const RULE_NEEDS_UPDATE = "Editing or deleting a rule needs the policy:update permission.";

/**
 * One mask or row-filter rule (spec §2 "Single items", P8):
 * `/governance/rules/:id`, where explain's attributions, the overlap report
 * and the People area link. Its kind, what it selects (a path with Open in
 * Data, or a tag with its classification page for a `tag:read` holder), its
 * exemptions linked to their People pages, and Edit and Delete with
 * `policy:update`. The rule comes from `GET /perm/rules`, the whole list
 * (there is no read by id).
 */
export default function RulePage() {
  const { id = "" } = useParams();
  const rulesQ = usePermListRules({ query: { retry: false } });
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const queryClient = useQueryClient();
  const deleteRule = usePermDeleteRule({
    mutation: {
      // Stays on the page, which then says the id names no rule; the toast says why.
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
        toast.success("Rule deleted.");
      },
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The rule could not be deleted.")),
    },
  });

  let body: ReactNode;
  let rule: RuleView | undefined;
  if (rulesQ.isError) body = <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  else if (rulesQ.data?.status !== 200) body = <ListSkeleton rows={4} cols={2} />;
  else {
    rule = rulesQ.data.data.find((r) => r.id === id);
    if (!rule) body = <ItemMissing text="No rule has this id; it may have been deleted." />;
  }

  if (!rule) {
    return (
      <div className="space-y-6 p-4">
        <ItemHeader kind="Rule" title="Rule" />
        {body}
      </div>
    );
  }
  const found = rule;
  return (
    <>
      <RuleBody rule={found} onEdit={() => setEditing(true)} onDelete={() => setDeleting(true)} />
      <RuleDialog open={editing} onOpenChange={setEditing} existing={found} />
      <ConfirmDeleteDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete "${found.name}"?`}
        description="This rule is permanently removed and every profile it reaches recompiles."
        onConfirm={() => {
          deleteRule.mutate({ id: found.id });
          setDeleting(false);
        }}
      />
    </>
  );
}

function RuleBody({ rule: r, onEdit, onDelete }: { rule: RuleView; onEdit: () => void; onDelete: () => void }) {
  const can = usePolicyPermissions();
  const catalogNameById = useCatalogNameById();
  const workspaceName = useWorkspaceNames();
  const link = r.selector.type === "path" ? scopeToDataLink(r.selector, catalogNameById, "masks") : null;

  const actions = can("policy:update") ? (
    <>
      <Button variant="outline" size="sm" onClick={onEdit}>
        <Pencil aria-hidden />
        Edit
      </Button>
      <Button variant="outline" size="sm" className="text-destructive" onClick={onDelete}>
        <Trash2 aria-hidden />
        Delete
      </Button>
    </>
  ) : (
    <p className="text-xs text-muted-foreground">{RULE_NEEDS_UPDATE}</p>
  );

  return (
    <div className="space-y-6 p-4">
      <ItemHeader kind={r.kind.type === "mask" ? "Mask rule" : "Row-filter rule"} title={r.name} actions={actions} />
      <dl className="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[10rem_1fr]">
        <Detail term="Kind">
          <span className="break-words font-mono">{formatRuleKind(r)}</span>
        </Detail>
        <Detail term="Selects">
          <span className="break-all">{formatSelector(r)}</span>
          {r.selector.type === "tag" ? (
            r.tag_name ? (
              can("tag:read") ? (
                <TagLink tagKey={r.selector.key} subject={r.name} />
              ) : null
            ) : (
              <span className="block text-xs text-muted-foreground">{`No tag has the key ${r.selector.key} now.`}</span>
            )
          ) : link ? (
            <span className="block">
              <OpenInDataLink to={link.to} subject={r.name} stoppedAtPattern={link.stoppedAtPattern} />
            </span>
          ) : null}
        </Detail>
        <Detail term="Exempt">
          <PrincipalNames list={r.exempt.include} workspaceName={workspaceName} />
          {r.exempt.exclude.length > 0 ? (
            <>
              {" except "}
              <PrincipalNames list={r.exempt.exclude} workspaceName={workspaceName} />
            </>
          ) : null}
        </Detail>
      </dl>
    </div>
  );
}

/**
 * The classification page of a tag rule's tag. Rendered only for a `tag:read`
 * holder: `GET /tags` needs it, and `policy:read` does not imply it. A key the
 * list does not hold gets no link, never a broken one.
 */
function TagLink({ tagKey, subject }: { tagKey: string; subject: string }) {
  const tags = useTags();
  const id = tags.data?.find((t) => t.key === tagKey)?.id;
  if (!id) return null;
  return (
    <span className="block">
      <OpenInDataLink to={`/governance/classifications/${encodeURIComponent(id)}`} subject={subject} label="Open classification" />
    </span>
  );
}
