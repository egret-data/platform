import { useMemo, useState, type ReactNode } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { focusHeadingOnArrival } from "@/lib/focusArrival";
import type { RuleView } from "@/api/generated/model";
import { getPermListRulesQueryKey, usePermDeleteRule, usePermListRules, usePermRuleEffect } from "@/api/generated/perm/perm";
import { useTags } from "@/api/tags";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { OpenInDataLink, scopeToDataLink, useCatalogNameById, useWorkspaceNames } from "@/routes/governance/openInData";
import { formatSelector, RULE_DELETE_DESCRIPTION } from "@/routes/governance/permFormat";
import { RuleKindLabel } from "@/routes/governance/RuleKindLabel";
import { PrincipalNames } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TabError } from "@/routes/governance/tabParts";
import { NOT_ENFORCED_YET } from "@/routes/governance/NotEnforced";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import { Detail, ItemHeader, ItemMissing, type ItemBack } from "@/routes/governance/people/itemParts";
import { PEOPLE_BASE } from "@/routes/governance/people/peopleUrl";
import { DATA_BASE } from "@/routes/governance/data/objectPath";
import { TERM } from "@/routes/governance/glossary";
import { effectSentence, notEnforcedWarning, PARTLY_ENFORCED, partlyEnforcedLines } from "@/routes/governance/ruleEffect";
import { PageContainer } from "@/components/layout/PageContainer";

/** What a reader without `policy:update` reads instead of Edit and Delete. Also this module's bundle marker. */
export const RULE_NEEDS_UPDATE = "Editing or deleting a rule needs the policy:update permission.";

/**
 * One mask or row-filter rule (spec §2 "Single items", P8):
 * `/governance/rules/:id`, where explain's attributions, the overlap report
 * and the People area link. Its kind, what it selects (a path with Open in
 * Data, or a tag with its classification page for a `tag:read` holder), its
 * exemptions linked to their People pages, and Edit and Delete with
 * `policy:update`. The rule comes from `GET /perm/rules`, the whole list
 * (there is no read by id). Its way back names where it was opened from
 * (`?from=`, `RuleHeader`). Delete asks for the rule's name (spec C11, the one
 * delete convention) and, done, goes back where the page was opened from
 * (`ruleDeletedTo`) with a toast, the page itself being gone.
 */
export default function RulePage() {
  const { id = "" } = useParams();
  const rulesQ = usePermListRules({ query: { retry: false } });
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const canReadTags = usePolicyPermissions()("tag:read");

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
      <PageContainer className="space-y-6">
        <RuleHeader title={TERM.rule} />
        {body}
      </PageContainer>
    );
  }
  const found = rule;
  return (
    <>
      <RuleBody rule={found} onEdit={() => setEditing(true)} onDelete={() => setDeleting(true)} />
      <EditRuleDialog rule={found} open={editing} onOpenChange={setEditing} />
      {canReadTags && found.selector.type === "tag" ? (
        <DeleteRuleDialogWithTags rule={found} open={deleting} onOpenChange={setDeleting} />
      ) : (
        <DeleteRuleDialog rule={found} open={deleting} onOpenChange={setDeleting} />
      )}
    </>
  );
}

type DeleteRuleProps = { rule: RuleView; open: boolean; onOpenChange: (open: boolean) => void };

/** `DeleteRuleDialog` for a `tag:read` holder: the classifications list names where a classification rule goes back to. */
function DeleteRuleDialogWithTags(props: DeleteRuleProps) {
  const tags = useTags();
  return <DeleteRuleDialog {...props} tags={tags.data} />;
}

/**
 * Delete this rule: type its name (spec C11). Done, the page leaves for where
 * it was opened from (`ruleDeletedTo`) and toasts, the page itself being gone
 * (FE-ACTION-2's exception). The cached rules list drops the rule first, so
 * the page it returns to never shows it, and the navigation replaces this
 * page's history entry, so Back does not lead here. The list without this rule
 * never renders here as "No rule has this id": the cache notifies after the
 * navigation has unmounted this page.
 */
function DeleteRuleDialog({ rule, open, onOpenChange, tags }: DeleteRuleProps & { tags?: { id: string; key: string }[] }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [search] = useSearchParams();
  const deleteRule = usePermDeleteRule({
    mutation: {
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The rule could not be deleted.")),
    },
  });
  return (
    <ConfirmDeleteDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`${TERM.delete} "${rule.name}"?`}
      confirmLabel={TERM.delete}
      description={RULE_DELETE_DESCRIPTION}
      confirmText={rule.name}
      onConfirm={() => {
        const to = ruleDeletedTo(parseRuleFrom(search.get("from")), rule, tags);
        deleteRule.mutate(
          { id: rule.id },
          {
            onSuccess: () => {
              // The cached list drops the rule before the page it returns to
              // mounts, so that page never shows it while the list refetches;
              // replace, so Back does not lead to the deleted rule's page.
              queryClient.setQueriesData<{ status: number; data: unknown }>(
                { queryKey: getPermListRulesQueryKey() },
                (old) =>
                  old?.status === 200 && Array.isArray(old.data)
                    ? { ...old, data: (old.data as RuleView[]).filter((r) => r.id !== rule.id) }
                    : old,
              );
              focusHeadingOnArrival();
              navigate(to, { replace: true });
              toast.success(`${TERM.rule} ${rule.name} deleted.`);
              void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
            },
          },
        );
      }}
    />
  );
}

/**
 * The rule dialog on this rule. The entry keeps its identity while the rule's
 * content is the same, so a background refetch of the rules list does not hand
 * the open form a new entry; when the content does change, an untouched form
 * follows it and an edited one keeps its edits (`RuleDialog`).
 */
function EditRuleDialog({ rule, open, onOpenChange }: { rule: RuleView; open: boolean; onOpenChange: (open: boolean) => void }) {
  const content = JSON.stringify(rule);
  const entry = useMemo(() => ({ kind: "edit" as const, rule: JSON.parse(content) as RuleView }), [content]);
  return <RuleDialog open={open} onOpenChange={onOpenChange} entry={entry} />;
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
        {TERM.delete}
      </Button>
    </>
  ) : (
    <p className="text-xs text-muted-foreground">{RULE_NEEDS_UPDATE}</p>
  );

  return (
    <PageContainer className="space-y-6">
      <RuleHeader title={r.name} actions={actions} />
      <dl className="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[10rem_1fr]">
        <Detail term={TERM.kind}>
          <span className="break-words">
            <RuleKindLabel rule={r} />
          </span>
        </Detail>
        <Detail term={TERM.appliesTo}>
          <span className="break-all">{formatSelector(r)}</span>
          {r.selector.type === "tag" ? (
            r.tag_name ? (
              can("tag:read") ? (
                <TagLink tagKey={r.selector.key} subject={r.tag_name} />
              ) : null
            ) : (
              <span className="block text-xs text-muted-foreground">{`No classification has the key ${r.selector.key} now.`}</span>
            )
          ) : link ? (
            <span className="block">
              <OpenInDataLink to={link.to} subject={r.name} stoppedAtPattern={link.stoppedAtPattern} />
            </span>
          ) : null}
        </Detail>
        <Detail term={TERM.exempt}>
          <PrincipalNames list={r.exempt.include} workspaceName={workspaceName} />
          {r.exempt.exclude.length > 0 ? (
            <>
              {" except "}
              <PrincipalNames list={r.exempt.exclude} workspaceName={workspaceName} />
            </>
          ) : null}
        </Detail>
        <RuleEffectDetail rule={r} />
      </dl>
    </PageContainer>
  );
}

/**
 * What the stored rule does (`GET /perm/rules/{id}/effect`, the preview's own
 * computation): what it reaches, by how many engines, who is exempt, and, when
 * no engine enforces it, "Not enforced yet." with the preview's reasons.
 * Rendered only once the effect is loaded: never a claim while it is loading,
 * failed or refused.
 */
function RuleEffectDetail({ rule }: { rule: RuleView }) {
  const effectQ = usePermRuleEffect(rule.id, { query: { retry: false, staleTime: 0 } });
  if (effectQ?.data?.status !== 200) return null;
  const effect = effectQ.data.data;
  const warning = notEnforcedWarning(effect, rule.selector.type);
  const partly = partlyEnforcedLines(effect, rule.selector.type);
  return (
    <Detail term="Effect">
      <span className="block">{effectSentence(effect)}</span>
      {warning ? (
        <span role="status" className="mt-1 flex items-start gap-1.5 text-sm">
          <AlertTriangle aria-hidden className="mt-0.5 size-4 shrink-0" />
          <span>
            <strong>{`${NOT_ENFORCED_YET}.`}</strong> {warning.join(" ")}
          </span>
        </span>
      ) : null}
      {partly ? (
        <span role="status" className="mt-1 flex items-start gap-1.5 text-sm">
          <AlertTriangle aria-hidden className="mt-0.5 size-4 shrink-0" />
          <span>
            <strong>{`${PARTLY_ENFORCED}.`}</strong> {partly.join(" ")}
          </span>
        </span>
      ) : null}
    </Detail>
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
      <OpenInDataLink to={tagUrl(id, "rules")} subject={subject} label="Open classification" />
    </span>
  );
}

/**
 * Where the rule page's way back goes, from `?from=` (spec C1): the page
 * names where it was opened from.
 * - `classification:<id>`: that classification's Rules tab, named by the
 *   classification (`TagPage` lists rules with this, through `TagRulesSection`).
 * - `data:<url-encoded path>`: that Data-area page, named by its last path
 *   segment. Only a path inside the Data area is followed; anything else (an
 *   absolute URL, `//host`, an empty or `..` segment) falls back, so `from`
 *   cannot send the reader off egret or out of the Data area.
 * - Anything else, or none: People & groups (`ItemHeader`'s default).
 */
type RuleFrom = { kind: "classification"; id: string } | { kind: "data"; back: ItemBack } | null;

function parseRuleFrom(from: string | null): RuleFrom {
  if (!from) return null;
  if (from.startsWith("classification:")) {
    const id = from.slice("classification:".length);
    return id ? { kind: "classification", id } : null;
  }
  if (from.startsWith("data:")) {
    let to: string;
    try {
      to = decodeURIComponent(from.slice("data:".length));
    } catch {
      return null;
    }
    if (!to.startsWith(`${DATA_BASE}/`)) return null;
    const segments = to.slice(DATA_BASE.length + 1).split(/[?#]/)[0].split("/");
    // An empty, `.` or `..` segment (raw or encoded) is not a Data page; a router would resolve it elsewhere.
    const dotty = (s: string) => /^(\.|%2e){1,2}$/i.test(s);
    if (segments.some((s) => s === "" || dotty(s))) return null;
    const segment = segments[segments.length - 1];
    let label = segment;
    try {
      label = decodeURIComponent(segment);
    } catch {
      // A segment that is not valid percent-encoding reads as written.
    }
    return { kind: "data", back: { to, label } };
  }
  return null;
}

/**
 * Where a deleted rule's page goes: where it was opened from (`?from=`); else,
 * for a rule on a classification the reader can see, that classification's
 * Rules tab (where such rules are listed); else People & groups, where the
 * page's way back points.
 */
function ruleDeletedTo(from: RuleFrom, rule: RuleView, tags: { id: string; key: string }[] | undefined): string {
  if (from?.kind === "classification") return tagUrl(from.id, "rules");
  if (from?.kind === "data") return from.back.to;
  const selector = rule.selector;
  const tagId = selector.type === "tag" ? tags?.find((t) => t.key === selector.key)?.id : undefined;
  return tagId ? tagUrl(tagId, "rules") : PEOPLE_BASE;
}

/** The rule page's header: kind "Rule", and the way back `?from=` names. */
function RuleHeader({ title, actions }: { title: string; actions?: ReactNode }) {
  const [search] = useSearchParams();
  const from = parseRuleFrom(search.get("from"));
  const can = usePolicyPermissions();
  if (from?.kind === "classification") {
    // `GET /tags` needs `tag:read`, which `policy:read` does not imply: without it, ask nothing.
    return can("tag:read") ? (
      <ClassificationBackHeader tagId={from.id} title={title} actions={actions} />
    ) : (
      <ItemHeader kind={TERM.rule} title={title} actions={actions} back={{ to: tagUrl(from.id, "rules"), label: TERM.classification }} />
    );
  }
  return <ItemHeader kind={TERM.rule} title={title} actions={actions} back={from?.kind === "data" ? from.back : undefined} />;
}

/** Back to classification `tagId`'s Rules tab, named by the classification once the list has it. */
function ClassificationBackHeader({ tagId, title, actions }: { tagId: string; title: string; actions?: ReactNode }) {
  const tags = useTags();
  const name = tags.data?.find((t) => t.id === tagId)?.name ?? TERM.classification;
  return <ItemHeader kind={TERM.rule} title={title} actions={actions} back={{ to: tagUrl(tagId, "rules"), label: name }} />;
}
