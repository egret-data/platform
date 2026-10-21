import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import type { ApiError } from "@/api/client";
import { useControlPlaneCatalogUnassignResourceTag } from "@/api/generated/control-plane-catalog/control-plane-catalog";
import { usePermListRules } from "@/api/generated/perm/perm";
import { invalidateTagAssignment } from "@/api/tags";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { usePolicyPermissions, refusalMessage } from "@/routes/governance/policyWrites";
import { rulesOnTag } from "@/routes/governance/ruleMatch";
import { TERM } from "@/routes/governance/glossary";

/*
 * Kept outside both lazy areas (spec C4): the Data pages and the
 * Classification's Where used open it, and it imports nothing from `data/`.
 */

/** The table or column a classification is removed from, as stored (the server matches the coordinate exactly). */
export type RemoveTarget = {
  warehouseId: string;
  schema: string;
  table: string;
  /** Absent (or empty) for the table itself. */
  column?: string;
  /** What the dialog and the toast call it: `ice.sales.orders.email`. */
  label: string;
};

/**
 * Remove one classification from a table or a column (spec C4): a plain
 * confirm, no typing (C11: typing the name is for deleting a classification,
 * not for taking it off one object). The body says what stops applying: the
 * rules that select by this classification (`rulesOnTag`, named with
 * `policy:read`) and, for a table, its columns' inheritance.
 *
 * Remove calls `DELETE /warehouses/{warehouse_id}/resource-tags`
 * (`catalog_registry:update`; idempotent). On success it refreshes what an
 * attach refreshes (`invalidateTagAssignment`), closes and toasts: the page
 * stays (FE-ACTION-2). On failure it stays open with the server's sentence
 * (FE-ACTION-3).
 */
export function RemoveClassificationDialog({
  open,
  onOpenChange,
  tag,
  target,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tag: { id: string; name: string; key: string };
  target: RemoveTarget;
}) {
  const queryClient = useQueryClient();
  const canReadRules = usePolicyPermissions()("policy:read");
  const rulesQ = usePermListRules({ query: { enabled: open && canReadRules, retry: false } });
  const remove = useControlPlaneCatalogUnassignResourceTag<ApiError>();
  const onTable = !target.column;

  const close = (next: boolean) => {
    if (remove.isPending) return;
    if (!next) remove.reset();
    onOpenChange(next);
  };

  const confirm = () => {
    remove
      .mutateAsync({
        warehouseId: target.warehouseId,
        data: { schema: target.schema, table: target.table, column: target.column ?? "", tag_id: tag.id },
      })
      .then(
        () => {
          invalidateTagAssignment(queryClient, tag.id, target.warehouseId);
          toast.success(`Removed ${tag.name} from ${target.label}.`);
          remove.reset();
          onOpenChange(false);
        },
        () => {
          // Shown in the dialog from `remove.error`; nothing is cleared.
        },
      );
  };

  let rules = null;
  if (canReadRules) {
    if (rulesQ.isError) {
      rules = <p>{`The ${TERM.rules.toLowerCase()} could not be read.`}</p>;
    } else if (rulesQ.data?.status === 200) {
      const named = rulesOnTag(rulesQ.data.data, tag)
        .map((r) => r.name)
        .sort((a, b) => a.localeCompare(b));
      rules =
        named.length > 0 ? (
          <ul aria-label={`${TERM.rules} on ${tag.name}`} className="list-disc pl-5">
            {named.map((n) => (
              <li key={n} className="break-words">
                {n}
              </li>
            ))}
          </ul>
        ) : (
          <p>No rule selects it.</p>
        );
    } else {
      rules = <Skeleton className="h-4 w-48" />;
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={close}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="break-words">{`Remove ${tag.name} from ${target.label}?`}</AlertDialogTitle>
          <AlertDialogDescription>{`Rules that apply to ${tag.name} stop applying here.`}</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="space-y-2 text-sm text-muted-foreground">
          {rules}
          {onTable ? <p>Its columns stop inheriting it.</p> : null}
        </div>
        {remove.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              {refusalMessage(remove.error, "catalog_registry:update", "The classification could not be removed. Try again.")}
            </AlertDescription>
          </Alert>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={remove.isPending}>Cancel</AlertDialogCancel>
          <Button variant="destructive" onClick={confirm} disabled={remove.isPending}>
            {remove.isPending ? <Loader2 aria-hidden className="animate-spin" /> : null}
            {TERM.remove}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

type Removing = { tag: { id: string; name: string; key: string }; target: RemoveTarget };

/**
 * A remove dialog owned by the list that offers Remove: `ask` opens it on one
 * assignment, and `dialog` is rendered once beside the list. The list owns it,
 * not the chip, so the dialog outlives the chip it removes.
 */
export function useRemoveClassification(): { ask: (r: Removing) => void; dialog: ReactNode } {
  const [removing, setRemoving] = useState<Removing | null>(null);
  const dialog = removing ? (
    <RemoveClassificationDialog
      open
      onOpenChange={(open) => {
        if (!open) setRemoving(null);
      }}
      tag={removing.tag}
      target={removing.target}
    />
  ) : null;
  return { ask: setRemoving, dialog };
}
