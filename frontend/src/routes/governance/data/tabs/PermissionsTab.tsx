import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { getPermListGrantsQueryKey, usePermDeleteGrant, usePermListGrants } from "@/api/generated/perm/perm";
import type { GovernanceCatalog, GrantView } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { AssignmentBadge } from "@/routes/governance/GrantDialog";
import { grantSentence } from "@/routes/governance/permFormat";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { grantCoverage } from "@/routes/governance/data/coverage";
import type { CatalogState } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate, Section, TabError } from "@/routes/governance/data/tabs/TabStates";

/**
 * What a viewer without `policy:read` reads instead of the grants. Also this
 * module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const GRANTS_NEED_POLICY_READ = "Reading grants needs the policy:read permission, which this account does not have.";

const LEVEL_LABEL = { catalog: "Catalog", schema: "Schema", table: "Table" } as const;

type Inherited = { grant: GrantView; from: keyof typeof LEVEL_LABEL };

/**
 * An object's Permissions tab (spec §2.2): every grant that reaches the
 * object, grouped by how it reaches it: set here, inherited from an ancestor
 * (the level named), or set on one of a table's columns. A deny grant is
 * marked.
 *
 * The grants come from the existing list route (`GET /perm/grants`,
 * `policy:read`), filtered here by {@link grantCoverage} (spec §2.3, "Not
 * new"). Edit and delete are the Data permissions page's: an `explicit` grant
 * opens `GrantDialog` (through `onEdit`) or the delete confirmation, each only
 * with `policy:update` (the permission both routes require); a grant written
 * by a workspace assignment is read-only and names its workspace. A save or a
 * delete invalidates the grants list, so this tab refetches.
 */
export function PermissionsTab({
  path,
  kind,
  catalog,
  onEdit,
}: {
  path: ObjectPath;
  /** `null` while an object's kind is not known. */
  kind: NodeKind | null;
  catalog: CatalogState;
  onEdit: (grant: GrantView) => void;
}) {
  return <CatalogGate catalog={catalog}>{(c) => <Grants path={path} kind={kind} catalog={c} onEdit={onEdit} />}</CatalogGate>;
}

function Grants({
  path,
  kind,
  catalog,
  onEdit,
}: {
  path: ObjectPath;
  kind: NodeKind | null;
  catalog: GovernanceCatalog;
  onEdit: (grant: GrantView) => void;
}) {
  const can = usePolicyPermissions();
  const canRead = can("policy:read");
  const canWrite = can("policy:update");
  const grantsQ = usePermListGrants({ query: { enabled: canRead, retry: false } });

  const queryClient = useQueryClient();
  const [deleting, setDeleting] = useState<GrantView | null>(null);
  const deleteGrant = usePermDeleteGrant({
    mutation: {
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: getPermListGrantsQueryKey() });
        toast.success("Grant deleted.");
      },
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The grant could not be deleted.")),
    },
  });

  if (!canRead) {
    return (
      <Alert>
        <AlertDescription>{GRANTS_NEED_POLICY_READ}</AlertDescription>
      </Alert>
    );
  }
  if (grantsQ.isError) return <TabError what="grants" onRetry={() => void grantsQ.refetch()} />;
  if (grantsQ.data?.status !== 200) return <ListSkeleton rows={4} cols={3} />;

  // The object itself: the column panel's `?column=` is not this tab's subject.
  const target: ObjectPath = { catalog: path.catalog, schema: path.schema, object: path.object };
  const here: GrantView[] = [];
  const inherited: Inherited[] = [];
  const onColumns: GrantView[] = [];
  for (const g of grantsQ.data.data) {
    const reach = grantCoverage(g, target, catalog.id);
    if (reach === "here") here.push(g);
    else if (reach === "child") onColumns.push(g);
    else if (reach) inherited.push({ grant: g, from: reach.inheritedFrom });
  }
  const bySentence = (a: GrantView, b: GrantView) => grantSentence(a).localeCompare(grantSentence(b));
  here.sort(bySentence);
  onColumns.sort((a, b) => (a.scope.column ?? "").localeCompare(b.scope.column ?? "") || bySentence(a, b));
  const order = Object.keys(LEVEL_LABEL);
  inherited.sort((a, b) => order.indexOf(a.from) - order.indexOf(b.from) || bySentence(a.grant, b.grant));

  if (here.length + inherited.length + onColumns.length === 0) {
    const noun = kind ? KIND_ICON[kind].label.toLowerCase() : "object";
    return <EmptyState icon={KeyRound} title={`No grant reaches this ${noun}`} />;
  }

  const workspaceName = (id: string) => catalog.workspaces.find((w) => w.id === id)?.name ?? id;
  const table = (label: string, rows: GrantView[], extra?: { head: string; cell: (g: GrantView) => string; mono?: boolean }) => (
    <GrantTable
      label={label}
      rows={rows}
      extra={extra}
      canWrite={canWrite}
      workspaceName={workspaceName}
      onEdit={onEdit}
      onDelete={setDeleting}
    />
  );
  const fromOf = new Map(inherited.map((i) => [i.grant, LEVEL_LABEL[i.from]]));

  return (
    <div className="space-y-6">
      {here.length > 0 ? <Section title="Here">{table("Here", here)}</Section> : null}
      {inherited.length > 0 ? (
        <Section title="Inherited">
          {table(
            "Inherited",
            inherited.map((i) => i.grant),
            { head: "From", cell: (g) => fromOf.get(g) ?? "" },
          )}
        </Section>
      ) : null}
      {onColumns.length > 0 ? (
        <Section title="On columns">
          {table("On columns", onColumns, { head: "Column", cell: (g) => g.scope.column ?? "", mono: true })}
        </Section>
      ) : null}

      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title={deleting ? `Delete "${grantSentence(deleting)}"?` : "Delete grant?"}
        description="This grant is permanently removed and every profile it reaches recompiles."
        onConfirm={() => {
          if (deleting) deleteGrant.mutate({ id: deleting.id });
          setDeleting(null);
        }}
      />
    </div>
  );
}

function GrantTable({
  label,
  rows,
  extra,
  canWrite,
  workspaceName,
  onEdit,
  onDelete,
}: {
  label: string;
  rows: GrantView[];
  extra?: { head: string; cell: (g: GrantView) => string; mono?: boolean };
  canWrite: boolean;
  workspaceName: (id: string) => string;
  onEdit: (g: GrantView) => void;
  onDelete: (g: GrantView) => void;
}) {
  return (
    <Table aria-label={label}>
      <TableHeader>
        <TableRow>
          <TableHead>Grant</TableHead>
          {extra ? <TableHead>{extra.head}</TableHead> : null}
          <TableHead>Effect</TableHead>
          {canWrite ? (
            <TableHead className="w-24">
              <span className="sr-only">Actions</span>
            </TableHead>
          ) : null}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((g) => {
          const sentence = grantSentence(g);
          return (
            <TableRow key={g.id}>
              <TableCell className="whitespace-normal">
                <div className="space-y-1">
                  <p className="text-sm break-words">{sentence}</p>
                  {g.origin.type === "workspace_assignment" ? (
                    <AssignmentBadge workspaceLabel={workspaceName(g.origin.workspace_id)} />
                  ) : null}
                </div>
              </TableCell>
              {extra ? <TableCell className={extra.mono ? "font-mono text-xs" : undefined}>{extra.cell(g)}</TableCell> : null}
              <TableCell>
                {g.effect === "deny" ? <Badge variant="destructive">Deny</Badge> : <Badge variant="outline">Allow</Badge>}
              </TableCell>
              {canWrite ? (
                <TableCell className="text-right">
                  {g.origin.type === "explicit" ? (
                    <div className="flex justify-end gap-1">
                      <Button variant="ghost" size="icon" aria-label={`Edit ${sentence}`} onClick={() => onEdit(g)}>
                        <Pencil aria-hidden className="size-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="text-muted-foreground hover:text-destructive"
                        aria-label={`Delete ${sentence}`}
                        onClick={() => onDelete(g)}
                      >
                        <Trash2 aria-hidden className="size-4" />
                      </Button>
                    </div>
                  ) : null}
                </TableCell>
              ) : null}
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
