import type { ReactNode, RefObject } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import type { ApiError } from "@/api/client";
import { useControlPlaneGovernanceTagUsage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { getPermListRulesQueryKey, usePermListRules } from "@/api/generated/perm/perm";
import { type Tag, useDeleteTag } from "@/api/tags";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { toastTagDeleteFailure } from "@/routes/governance/tagDeleteFailure";
import { tagInUse } from "@/routes/governance/apiErrors";
import { CLASSIFICATIONS_BASE, classificationRuleUrl, tagUrl } from "@/routes/governance/classifications/tagUrl";

const n = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/**
 * Delete a classification (spec §5.6, CL14, C11). The body states what the
 * delete removes, from route C (Data counts, BI objects readable and not) and
 * the tag itself (its clearance). The rules that select this tag's KEY are
 * the ones that refuse the delete (premise 11; a rule on another tag with the
 * same name does not): while a `policy:read` holder can see any, Delete is
 * `aria-disabled` with "Delete or change these rules first:" and a link to
 * each rule, and there is nothing to type. Otherwise confirming needs the
 * classification's name (case-insensitive, the one delete convention), and the
 * server stays the guard: its `tag_in_use` is toasted (`tagDeleteFailure.ts`).
 * Success from the tag's page opens the index without a toast; from the index
 * it stays there, toasts (FE-ACTION-2) and calls `onDeleted`, whose caller
 * puts focus on a control that stays. A failure closes the dialog and focus
 * returns to the control that opened it.
 */
export function DeleteTagDialog({
  tag,
  open,
  onOpenChange,
  where,
  opener,
  onDeleted,
}: {
  tag: Tag;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Where the delete was asked: on the classification's page success opens the index, elsewhere it stays. */
  where: "this page" | "the classification's page";
  /** The control that opened the dialog; focus returns to it on close (#4a D3), the dialog being opened from state. */
  opener?: RefObject<HTMLElement | null>;
  /** Called after a delete that stays on the page (from the index): the opener went with the row. */
  onDeleted?: () => void;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const canReadRules = usePolicyPermissions()("policy:read");
  const usageQ = useControlPlaneGovernanceTagUsage(tag.id, { query: { enabled: open, retry: false } });
  const rulesQ = usePermListRules({ query: { enabled: open && canReadRules, retry: false } });
  const del = useDeleteTag();

  const lines: ReactNode[] = [];
  if (usageQ.isError) {
    const message = (usageQ.error as unknown as ApiError | null)?.message ?? "unknown error";
    lines.push(`Usage could not be read: ${message}`);
  } else if (usageQ.data?.status === 200) {
    const { data, bi } = usageQ.data.data;
    lines.push(
      `Removes it from ${n(data.tableCount, "table", "tables")} and ${n(data.columnCount, "column", "columns")}.`,
    );
    const extra = [
      bi.unreadable.count > 0 ? `${bi.unreadable.count} you cannot read` : null,
      bi.remaining > 0 ? `${bi.remaining} not checked` : null,
    ].filter(Boolean);
    lines.push(
      `Removes it from ${n(bi.readable.length, "BI object", "BI objects")}${extra.length ? `, and ${extra.join(" and ")}` : ""}.`,
    );
  } else {
    lines.push(<Skeleton key="usage" className="h-4 w-64" />);
  }

  const roles = tag.required_roles.length;
  const groups = tag.required_groups.length;
  if (roles + groups > 0) {
    lines.push(`Removes its clearance (${n(roles, "role", "roles")}, ${n(groups, "group", "groups")}).`);
  }

  // The rules that refuse the delete: written on this classification's key.
  const blocking =
    canReadRules && rulesQ.data?.status === 200
      ? rulesQ.data.data.filter((r) => r.selector.type === "tag" && r.selector.key === tag.key)
      : [];
  if (!canReadRules) {
    lines.push("Rules: needs policy:read to count.");
  } else if (rulesQ.isError) {
    const message = (rulesQ.error as unknown as ApiError | null)?.message ?? "unknown error";
    lines.push(`Rules could not be read: ${message}`);
  } else if (rulesQ.data?.status === 200) {
    if (blocking.length === 0) lines.push("No rule selects it.");
  } else {
    lines.push(<Skeleton key="rules" className="h-4 w-48" />);
  }
  const blockedReason =
    blocking.length > 0 ? (
      <div className="space-y-1">
        <p>Delete or change these rules first:</p>
        <ul className="list-disc space-y-1 pl-5">
          {blocking.map((r) => (
            <li key={r.id}>
              <Link to={classificationRuleUrl(r.id, tag.id)} className="font-medium text-primary hover:underline">
                {r.name}
              </Link>
            </li>
          ))}
        </ul>
      </div>
    ) : undefined;

  // `mutateAsync`, not `mutate`'s per-call callbacks: the dialog closes on
  // confirm, and from the index that unmounts it, which drops those callbacks
  // (no toast either way).
  const confirm = () => {
    del.mutateAsync(tag.id).then(
      () => {
        if (where === "this page") {
          navigate(CLASSIFICATIONS_BASE);
        } else {
          // Opened from the index: the page stays, so a toast confirms it (FE-ACTION-2).
          toast.success(`Classification ${tag.name} deleted.`);
          onDeleted?.();
        }
      },
      (e: unknown) => {
        // A rule written meanwhile: the next look at this dialog lists it.
        if (tagInUse(e) && canReadRules) void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
        toastTagDeleteFailure(e, {
          tagName: tag.name,
          // The Rules tab lists the rules to a `policy:read` holder only.
          showRules: canReadRules ? () => navigate(tagUrl(tag.id, "rules")) : undefined,
        });
      },
    );
  };

  return (
    <ConfirmDeleteDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Delete ${tag.name}?`}
      description={`Deleting ${tag.name} removes the classification and every assignment of it.`}
      confirmText={tag.name}
      confirmDisabledReason={blockedReason}
      onConfirm={confirm}
      onCloseAutoFocus={(event) => {
        const element = opener?.current?.isConnected ? opener.current : null;
        if (element) {
          event.preventDefault();
          element.focus();
        }
      }}
    >
      <ul className="list-disc space-y-1 pl-5">
        {lines.map((l, i) => (
          <li key={i}>{l}</li>
        ))}
      </ul>
    </ConfirmDeleteDialog>
  );
}
