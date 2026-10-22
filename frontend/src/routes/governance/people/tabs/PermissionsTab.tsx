import { useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { EngineGrantView, GrantView, Via } from "@/api/generated/model";
import {
  getPermListGrantsQueryKey,
  usePermDeleteGrant,
  usePermListAllEngineGrants,
  usePermListEngineCapabilities,
  usePermListGrants,
} from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { FACET_ALL, FacetSelect } from "@/components/explore/FacetSelect";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { AssignmentBadge } from "@/routes/governance/GrantDialog";
import { grantDeleteDescription, grantSentence, type WorkspaceNames } from "@/routes/governance/permFormat";
import { GrantSentenceText } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { ACTIONS_CELL, HandWrittenPointer, Section, TabError, STACKED_TABLE } from "@/routes/governance/tabParts";
import { principalSubject, principalTitle, workspaceNamesOf, type Principal } from "@/routes/governance/people/principalData";
import { PrincipalHandWrittenSection } from "@/routes/governance/handwritten/PrincipalHandWrittenSection";
import {
  REACH_GROUPS,
  reachesAtAll,
  reachGroup,
  setReach,
  type Reach,
  type ReachGroup,
} from "@/routes/governance/people/reach";
import { appliedText } from "@/routes/governance/people/tabs/OverviewTab";
import { engineUrl } from "@/routes/governance/engines/engineUrl";
import { StatusBadge } from "@/components/ui/status-badge";

export const GROUP_TITLE: Record<ReachGroup, string> = {
  direct: "Directly",
  group: "Through a group",
  role: "Through a role",
  workspace: "Through a workspace",
  public: "Everyone",
  excluded: "Excluded",
  // Final review I1: a role or workspace held through a group is egret's only, but egret's own
  // checks never read grants (or rules), so what reaches only that way applies nowhere.
  egret_only: "Not applied anywhere",
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

/**
 * Under the "Not applied anywhere" group of grants (final review I1). Engines
 * do not apply a role or workspace a group holds, and egret's own checks never
 * read `perm.grants` (grants are compiled to engine policies only), so such a
 * grant applies to the principal nowhere. The Masks tab says the same of rules
 * (`EXEMPTION_NOT_ON_ENGINES`).
 */
export const GRANT_NOT_ON_ENGINES =
  "egret's own checks do not read grants either, so a grant that reaches only this way applies to this principal nowhere.";

/** What an exclude engines do not apply means: egret applies neither grants nor rules itself. */
const EXCLUDE_NOT_ON_ENGINES: Record<"grant" | "rule", string> = {
  grant: "engines do not apply it, and egret's own checks do not read grants",
  rule: "engines do not apply it, and egret applies no mask or row filter itself",
};

/**
 * `Through group g · applied on engines`, or the reason engines do not apply
 * the path; an exclude says so first. A path held through a group applies
 * nowhere, as neither engines nor egret apply it (`kind` names what is
 * reached: a grant, or a rule's exemption), and an exclude engines do not
 * apply excludes nobody.
 */
export function reachText(r: Reach, kind: "grant" | "rule" = "grant"): string {
  const how = howText(r.via);
  const lower = `${how.charAt(0).toLowerCase()}${how.slice(1)}`;
  if (r.position === "exclude") {
    // An exclude engines do not apply leaves nobody out anywhere (`reachGroup`).
    return r.onEngines ? `Excluded: ${lower} · applied on engines` : `Excludes nobody: ${lower} — ${EXCLUDE_NOT_ON_ENGINES[kind]}`;
  }
  if (r.onEngines) return `${how} · applied on engines`;
  if (r.notOnEngines === "held_through_group") return `${how} · not applied anywhere: held through a group`;
  return `${how} · ${appliedText(r)}`;
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
  // Spec §7 (EP20): engine permissions close the tab. `public` has none to list: an
  // engine grant is include-only and names users, groups or roles (F11 E5).
  const isPublic = principal.kind === "group" && principal.data.isPublic;
  return (
    <div className="space-y-6">
      <DataGrants principal={principal} onEdit={onEdit} />
      {/* #4b (spec §6): the hand-written access policies naming the principal. */}
      {isPublic ? null : <PrincipalHandWrittenSection subject={principalSubject(principal)} show="access" />}
      {isPublic ? null : <EngineGrantsSection principal={principal} />}
    </div>
  );
}

function DataGrants({ principal, onEdit }: { principal: Principal; onEdit: (grant: GrantView) => void }) {
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
  const note = noUsername ? <p className="text-sm">{NO_USERNAME_NOTE}</p> : null;
  const pointer = (
    <HandWrittenPointer
      lead="To see the engines' answer, use"
      explainLabel={isPublic ? null : "Check access"}
      explainTo={isPublic ? undefined : "?tab=check"}
    />
  );

  if (reaching.length === 0) {
    return (
      <div className="space-y-2">
        {note}
        <EmptyState icon={KeyRound} title={`No egret grant reaches ${name}`} />
        {pointer}
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
            {g === "egret_only" ? (
              <p className="text-sm text-muted-foreground">
                {`These reach this principal only through a role or workspace a group holds, which engines do not apply. ${GRANT_NOT_ON_ENGINES}`}
              </p>
            ) : null}
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
      {pointer}
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

type EngineRow = { grant: EngineGrantView; reach: Reach[] };

/**
 * The engine grants (F11) one of whose principals stands for this principal,
 * matched as data grants are (`setReach` over `standsFor`), so a grant that
 * reaches a user through a group says "Through group g". An engine grant is
 * include-only, so only include paths count. Each row links to the grant on
 * its engine's Engine permissions tab. Its own loading and error: a failure
 * here never hides the data grants above (FE-STATE-1).
 */
function EngineGrantsSection({ principal }: { principal: Principal }) {
  const grantsQ = usePermListAllEngineGrants({ query: { retry: false } });
  const capsQ = usePermListEngineCapabilities({ query: { retry: false } });
  const labels = new Map<string, string>();
  if (capsQ.data?.status === 200) {
    for (const e of capsQ.data.data.engines) for (const c of e.capabilities) labels.set(c.id, c.label);
  }
  const labelOf = (id: string) => labels.get(id) ?? id;

  let body: ReactNode;
  if (grantsQ.isError) {
    body = <TabError what="engine permissions" onRetry={() => void grantsQ.refetch()} />;
  } else if (grantsQ.data?.status !== 200) {
    body = <ListSkeleton rows={2} cols={4} />;
  } else {
    const rows: EngineRow[] = [];
    for (const g of grantsQ.data.data) {
      const reach = setReach({ include: g.principals, exclude: [] }, principal.data.standsFor);
      if (reach.some((r) => r.position === "include")) rows.push({ grant: g, reach });
    }
    rows.sort(
      (a, b) =>
        a.grant.engineName.localeCompare(b.grant.engineName) ||
        labelOf(a.grant.capability).localeCompare(labelOf(b.grant.capability)) ||
        valuesText(a.grant).localeCompare(valuesText(b.grant)),
    );
    body =
      rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">{`No engine permission reaches this ${principal.kind}.`}</p>
      ) : (
        <Table aria-label="Engine permissions" className={STACKED_TABLE}>
          <TableHeader>
            <TableRow>
              <TableHead>Engine</TableHead>
              <TableHead>Capability</TableHead>
              <TableHead>Values</TableHead>
              <TableHead>How it reaches</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map(({ grant: g, reach }) => (
              <TableRow key={g.id}>
                <TableCell>
                  <Link
                    to={engineUrl(g.engineId, "engine-permissions", { grant: g.id })}
                    className="break-all text-primary hover:underline"
                  >
                    {g.engineName}
                  </Link>
                </TableCell>
                <TableCell data-label="Capability" className="whitespace-normal">
                  {labelOf(g.capability)}
                </TableCell>
                <TableCell data-label="Values" className="whitespace-normal break-words">
                  {valuesText(g)}
                </TableCell>
                <TableCell data-label="How it reaches" className="whitespace-normal">
                  <ul role="list" className="space-y-0.5 text-sm">
                    {reach.map((r, i) => (
                      <li key={i}>{reachText(r)}</li>
                    ))}
                  </ul>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      );
  }
  return <Section title="Engine permissions">{body}</Section>;
}

/** An engine grant's values in the catalogue's key order; an impersonation target with no username says so. */
function valuesText(g: EngineGrantView): string {
  if (g.values.length > 0) return g.values.join(", ");
  return g.targetUserId ? "a user with no username" : "";
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
                {g.effect === "deny" ? <StatusBadge tone="destructive">Deny</StatusBadge> : <StatusBadge tone="success">Allow</StatusBadge>}
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
