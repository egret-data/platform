import type { ReactNode, RefObject } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import type { ApiError } from "@/api/client";
import { useControlPlaneGovernanceTagUsage } from "@/api/generated/control-plane-governance/control-plane-governance";
import { usePermListRules } from "@/api/generated/perm/perm";
import { type Tag, useDeleteTag } from "@/api/tags";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { toastTagDeleteFailure } from "@/routes/governance/tagDeleteFailure";
import { CLASSIFICATIONS_BASE, tagUrl } from "@/routes/governance/classifications/tagUrl";

const n = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

/**
 * Delete a classification (spec §5.6, CL14). The body states what the delete
 * removes, from route C (Data counts, BI objects readable and not) and the tag
 * itself (its clearance), and how many rules select this tag's KEY: those, and
 * only those, are the foreign key that refuses the delete (premise 11; a rule
 * on another tag with the same name does not). The button stays: the server
 * decides, and a 409 is toasted as before. Confirming needs the exact key.
 * Success from the tag's page opens the index without a toast; from the index
 * it stays there and toasts (FE-ACTION-2). A failure closes
 * the dialog and focus returns to the control that opened it.
 */
export function DeleteTagDialog({
  tag,
  open,
  onOpenChange,
  where,
  opener,
}: {
  tag: Tag;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Where the delete was asked: names the rules section in the 409 toast (`tagDeleteFailure.ts`). */
  where: "this page" | "the classification's page";
  /** The control that opened the dialog; focus returns to it on close (#4a D3), the dialog being opened from state. */
  opener?: RefObject<HTMLElement | null>;
}) {
  const navigate = useNavigate();
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

  if (!canReadRules) {
    lines.push("Rules: needs policy:read to count.");
  } else if (rulesQ.isError) {
    const message = (rulesQ.error as unknown as ApiError | null)?.message ?? "unknown error";
    lines.push(`Rules could not be read: ${message}`);
  } else if (rulesQ.data?.status === 200) {
    const blocking = rulesQ.data.data.filter((r) => r.selector.type === "tag" && r.selector.key === tag.key).length;
    lines.push(
      blocking === 0
        ? "No rule selects it."
        : `${blocking === 1 ? "1 rule selects" : `${blocking} rules select`} it: the delete will be refused until they are deleted or retargeted.`,
    );
  } else {
    lines.push(<Skeleton key="rules" className="h-4 w-48" />);
  }

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
        }
      },
      (e: unknown) =>
        toastTagDeleteFailure(e, {
          where,
          canReadRules,
          showRules: where === "this page" ? undefined : () => navigate(tagUrl(tag.id, "rules")),
        }),
    );
  };

  return (
    <ConfirmDeleteDialog
      open={open}
      onOpenChange={onOpenChange}
      title={`Delete ${tag.name}?`}
      description={`Deleting ${tag.key} removes the classification and every assignment of it.`}
      confirmText={tag.key}
      caseSensitive
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
