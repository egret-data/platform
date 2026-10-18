import { useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { GrantView, Via } from "@/api/generated/model";
import { getPermListGrantsQueryKey, usePermDeleteGrant, usePermListGrants } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { FACET_ALL, FacetSelect } from "@/components/explore/FacetSelect";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { AssignmentBadge } from "@/routes/governance/GrantDialog";
import { grantDeleteDescription, grantSentence, type WorkspaceNames } from "@/routes/governance/permFormat";
import { GrantSentenceText } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { ACTIONS_CELL, EgretGrantsOnlyNote, Section, TabError, STACKED_TABLE } from "@/routes/governance/tabParts";
import { principalTitle, workspaceNamesOf, type Principal } from "@/routes/governance/people/principalData";
import {
  REACH_GROUPS,
  reachesAtAll,
  reachGroup,
  setReach,
  type Reach,
  type ReachGroup,
} from "@/routes/governance/people/reach";
import { appliedText } from "@/routes/governance/people/tabs/OverviewTab";

export const GROUP_TITLE: Record<ReachGroup, string> = {
  direct: "Directly",
  group: "Through a group",
  role: "Through a role",
  workspace: "Through a workspace",
  public: "Everyone",
  excluded: "Excluded",
  egret_only: "egret only",
  not_on_engines: "Not applied on engines",
};

/** How one path reaches the principal, in words. */
function howText(via: Via): string {
  switch (via.kind) {
    case "itself":
      return "Directly";
    case "group":
      return `Through group ${via.name}`;
    case "role":
      return `Through role ${via.name}`;
    case "workspace":
      return `Through workspace ${via.name}`;
    case "public":
      return "Everyone (public)";
    case "group_role":
      return `Through role ${via.role}, held by group ${via.group}`;
    case "group_workspace":
      return `Through workspace ${via.name}, held by group ${via.group}`;
  }
}

/** `Through group g · applied on engines`, or the reason engines do not apply the path; an exclude says so first. */
export function reachText(r: Reach): string {
  const how = howText(r.via);
  const lower = `${how.charAt(0).toLowerCase()}${how.slice(1)}`;
  if (r.position === "exclude") {
    // An exclude engines do not apply leaves nobody out on engines (`reachGroup`).
    return r.onEngines ? `Excluded: ${lower} · applied on engines` : `Excluded in egret only — not on engines: ${lower}`;
  }
  return `${how} · ${r.onEngines ? "applied on engines" : appliedText(r)}`;
}

type Row = { grant: GrantView; reach: Reach[] };

export const NO_USERNAME_NOTE = "No engine knows this user until they have a username; nothing below applies on engines.";

/**
 * A principal's Permissions tab (spec §2): every egret grant one of whose
 * principals stands for this principal (`standsFor`, the server's list, spec
 * P4; matched in `reach.ts`), grouped by how it reaches (`reachGroup`), each
 * row saying every path and whether engines apply it. Deny is marked; effect
 * and action are facets in the URL (`?effect=`, `?action=`). Edit and delete
 * are Data's: an explicit grant, with `policy:update`; a workspace-assignment
 * grant is read-only and names its workspace.
 */
export function PermissionsTab({ principal, onEdit }: { principal: Principal; onEdit: (grant: GrantView) => void }) {
  const can = usePolicyPermissions();
  const canWrite = can("policy:update");
  const grantsQ = usePermListGrants({ query: { retry: false } });
  const [search, setSearch] = useSearchParams();
  const effect = search.get("effect") ?? "";
  const action = search.get("action") ?? "";
  const workspaceName = workspaceNamesOf(principal);
  const name = principalTitle(principal, principal.data.name);
  const isPublic = principal.kind === "group" && principal.data.isPublic;
  const noUsername = principal.kind === "user" && !principal.data.username;

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

  const setFacet = (key: string, value: string | null) =>
    setSearch((prev) => {
      const next = new URLSearchParams(prev);
      if (value === null || value === FACET_ALL) next.delete(key);
      else next.set(key, value);
      return next;
    });

  if (grantsQ.isError) return <TabError what="grants" onRetry={() => void grantsQ.refetch()} />;
  if (grantsQ.data?.status !== 200) return <ListSkeleton rows={4} cols={3} />;

  const sentenceOf = (g: GrantView) => grantSentence(g, workspaceName);
  const reaching: Row[] = [];
  for (const g of grantsQ.data.data) {
    const reach = setReach(g.principals, principal.data.standsFor);
    if (reachesAtAll(reach)) reaching.push({ grant: g, reach });
  }
  // Relative: this page, on its Check access tab.
  // `public` has no Check access tab (spec §2), so its note names none.
  const note = (
    <div className="space-y-1">
      <EgretGrantsOnlyNote
        explainLabel={isPublic ? null : "Check access"}
        explainTo={isPublic ? undefined : "?tab=check"}
      />
      {noUsername ? <p className="text-sm">{NO_USERNAME_NOTE}</p> : null}
    </div>
  );

  if (reaching.length === 0) {
    return (
      <div className="space-y-2">
        {note}
        <EmptyState icon={KeyRound} title={`No egret grant reaches ${name}`} />
      </div>
    );
  }

  const shown = reaching.filter(
    (r) => (!effect || r.grant.effect === effect) && (!action || r.grant.actions.includes(action as "read" | "write")),
  );
  const byGroup = new Map<ReachGroup, Row[]>();
  for (const r of shown) {
    const g = reachGroup(r.reach);
    byGroup.set(g, [...(byGroup.get(g) ?? []), r]);
  }
  for (const rows of byGroup.values()) rows.sort((a, b) => sentenceOf(a.grant).localeCompare(sentenceOf(b.grant)));

  return (
    <div className="space-y-6">
      {note}
      <div className="flex flex-wrap items-center gap-2">
        <FacetSelect
          label="Effect"
          value={effect}
          onChange={(v) => setFacet("effect", v)}
          options={[
            { value: "allow", label: "Allow" },
            { value: "deny", label: "Deny" },
          ]}
        />
        <FacetSelect
          label="Action"
          value={action}
          onChange={(v) => setFacet("action", v)}
          options={[
            { value: "read", label: "Read" },
            { value: "write", label: "Write" },
          ]}
        />
      </div>
      {shown.length === 0 ? (
        <EmptyState
          title="No grant matches these filters."
          action={
            <Button
              variant="outline"
              size="sm"
              onClick={() =>
                setSearch((prev) => {
                  const next = new URLSearchParams(prev);
                  next.delete("effect");
                  next.delete("action");
                  return next;
                })
              }
            >
              Clear filters
            </Button>
          }
        />
      ) : (
        REACH_GROUPS.filter((g) => byGroup.has(g)).map((g) => (
          <Section key={g} title={GROUP_TITLE[g]}>
            <GrantTable
              label={GROUP_TITLE[g]}
              rows={byGroup.get(g) ?? []}
              canWrite={canWrite}
              workspaceName={workspaceName}
              sentenceOf={sentenceOf}
              onEdit={onEdit}
              onDelete={setDeleting}
            />
          </Section>
        ))
      )}
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

function GrantTable({
  label,
  rows,
  canWrite,
  workspaceName,
  sentenceOf,
  onEdit,
  onDelete,
}: {
  label: string;
  rows: Row[];
  canWrite: boolean;
  workspaceName: WorkspaceNames;
  sentenceOf: (g: GrantView) => string;
  onEdit: (g: GrantView) => void;
  onDelete: (g: GrantView) => void;
}) {
  return (
    <Table aria-label={label} className={STACKED_TABLE}>
      <TableHeader>
        <TableRow>
          <TableHead>Grant</TableHead>
          <TableHead>How it reaches</TableHead>
          <TableHead>Effect</TableHead>
          {canWrite ? (
            <TableHead className={ACTIONS_CELL}>
              <span className="sr-only">Actions</span>
            </TableHead>
          ) : null}
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map(({ grant: g, reach }) => {
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
              <TableCell data-label="How it reaches" className="whitespace-normal">
                <ul role="list" className="space-y-0.5 text-sm">
                  {reach.map((r, i) => (
                    <li key={i}>{reachText(r)}</li>
                  ))}
                </ul>
              </TableCell>
              <TableCell data-label="Effect">
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
