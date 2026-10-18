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
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { grantDeleteDescription, grantSentence, type WorkspaceNames } from "@/routes/governance/permFormat";
import { GrantSentenceText } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { KIND_ICON, type NodeKind } from "@/routes/governance/data/CatalogTree";
import { grantCoverage, insideGroup, insideName, patternLevels, type PatternLevel } from "@/routes/governance/data/coverage";
import type { CatalogState } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate } from "@/routes/governance/data/tabs/TabStates";
import { ACTIONS_CELL, EgretGrantsOnlyNote, Section, TabError } from "@/routes/governance/tabParts";

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
 * (the level named), or matching a pattern; and the grants set on something
 * inside it (a table's columns; a schema's or a catalog's tables and
 * columns), each naming what it is on, so a grant made from a schema's page
 * on a table below it, or on a pattern such as `table = "*"`, is listed where
 * it was made and where its "Open in Data" link lands. A deny grant is marked.
 *
 * A grant that names a level by pattern (`table = "*"`, `ord*`) reaches every
 * value it matches, not this object alone, so it is never listed as "Here"
 * or beside a grant that leaves the level unset: it has its own group, which
 * names the pattern, its sentence quotes it (`ice.sales.“*”`, see
 * `formatScope`), and deleting it says how far it reaches.
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
  const workspaceName = useWorkspaceNames(catalog);
  const sentenceOf = (g: GrantView) => grantSentence(g, workspaceName);

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
  const inside: GrantView[] = [];
  const byPattern: GrantView[] = [];
  for (const g of grantsQ.data.data) {
    const reach = grantCoverage(g, target, catalog.id);
    if (reach !== null && patternLevels(g.scope).length > 0) byPattern.push(g);
    else if (reach === "here") here.push(g);
    else if (reach === "child") inside.push(g);
    else if (reach) inherited.push({ grant: g, from: reach.inheritedFrom });
  }
  const bySentence = (a: GrantView, b: GrantView) => sentenceOf(a).localeCompare(sentenceOf(b));
  here.sort(bySentence);
  byPattern.sort(bySentence);
  inside.sort((a, b) => insideName(a.scope, target).localeCompare(insideName(b.scope, target)) || bySentence(a, b));
  const order = Object.keys(LEVEL_LABEL);
  inherited.sort((a, b) => order.indexOf(a.from) - order.indexOf(b.from) || bySentence(a.grant, b.grant));

  // The tab renders the grants only with policy:read, which is also what Who can read? needs.
  const note = <EgretGrantsOnlyNote explainLabel={canRead ? "Who can read?" : null} />;
  if (here.length + inherited.length + inside.length + byPattern.length === 0) {
    const noun = kind ? KIND_ICON[kind].label.toLowerCase() : "object";
    return (
      <div className="space-y-2">
        {note}
        <EmptyState icon={KeyRound} title={`No egret grant reaches this ${noun}`} />
      </div>
    );
  }

  const table = (label: string, rows: GrantView[], extra?: { head: string; cell: (g: GrantView) => string; mono?: boolean }) => (
    <GrantTable
      label={label}
      rows={rows}
      extra={extra}
      canWrite={canWrite}
      workspaceName={workspaceName}
      sentenceOf={sentenceOf}
      onEdit={onEdit}
      onDelete={setDeleting}
    />
  );
  const insideTitle = insideGroup(target);
  const fromOf = new Map(inherited.map((i) => [i.grant, LEVEL_LABEL[i.from]]));

  return (
    <div className="space-y-6">
      {note}
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
      {inside.length > 0 ? (
        <Section title={insideTitle.title}>
          {table(insideTitle.title, inside, { head: insideTitle.head, cell: (g) => insideName(g.scope, target), mono: true })}
        </Section>
      ) : null}
      {byPattern.length > 0 ? (
        <Section title="Matching a pattern">
          {table("Matching a pattern", byPattern, {
            head: "Pattern",
            cell: (g) => patternLevels(g.scope).map(patternText).join(", "),
          })}
        </Section>
      ) : null}

      <ConfirmDeleteDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
        title={deleting ? `Delete "${sentenceOf(deleting)}"?` : "Delete grant?"}
        description={grantDeleteDescription(deleting)}
        onConfirm={() => {
          if (deleting) deleteGrant.mutate({ id: deleting.id });
          setDeleting(null);
        }}
      />
    </div>
  );
}

/** `table “*”`. */
function patternText(p: PatternLevel): string {
  return `${p.level} “${p.value}”`;
}

function GrantTable({
  label,
  rows,
  extra,
  canWrite,
  workspaceName,
  sentenceOf,
  onEdit,
  onDelete,
}: {
  label: string;
  rows: GrantView[];
  extra?: { head: string; cell: (g: GrantView) => string; mono?: boolean };
  canWrite: boolean;
  workspaceName: WorkspaceNames;
  sentenceOf: (g: GrantView) => string;
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
            <TableHead className={ACTIONS_CELL}>
              <span className="sr-only">Actions</span>
            </TableHead>
          ) : null}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((g) => {
          const sentence = sentenceOf(g);
          return (
            <TableRow key={g.id}>
              <TableCell className="whitespace-normal">
                <div className="space-y-1">
                  <p className="text-sm break-words">
                    <GrantSentenceText grant={g} workspaceName={workspaceName} />
                  </p>
                  {g.origin.type === "workspace_assignment" ? (
                    <AssignmentBadge workspaceLabel={workspaceName(g.origin.workspace_id) ?? g.origin.workspace_id} />
                  ) : null}
                </div>
              </TableCell>
              {extra ? <TableCell className={extra.mono ? "font-mono text-xs" : undefined}>{extra.cell(g)}</TableCell> : null}
              <TableCell>
                {g.effect === "deny" ? <Badge variant="destructive">Deny</Badge> : <Badge variant="outline">Allow</Badge>}
              </TableCell>
              {canWrite ? (
                <TableCell className={`${ACTIONS_CELL} text-right`}>
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
