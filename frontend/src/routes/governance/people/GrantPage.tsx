import { useState, type ReactNode } from "react";
import { useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Pencil, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { GrantView } from "@/api/generated/model";
import { getPermListGrantsQueryKey, usePermDeleteGrant, usePermListGrants } from "@/api/generated/perm/perm";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { AssignmentBadge, GrantDialog } from "@/routes/governance/GrantDialog";
import { OpenInDataLink, scopeToDataLink, useCatalogNameById, useWorkspaceNames } from "@/routes/governance/openInData";
import {
  formatActions,
  formatScope,
  grantDeleteDescription,
  grantSentence,
  patternLevels,
  type WorkspaceNames,
} from "@/routes/governance/permFormat";
import { GrantSentenceText, PrincipalNames } from "@/routes/governance/PrincipalNames";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TabError } from "@/routes/governance/tabParts";
import { Detail, ItemHeader, ItemMissing } from "@/routes/governance/people/itemParts";

/** What a workspace-assignment grant says instead of Edit and Delete. Also this module's bundle marker. */
export const ASSIGNMENT_READ_ONLY =
  "The workspace assignment sync writes this grant; change the workspace's catalog assignment to change it.";

/**
 * One grant (spec §2 "Single items", P8): `/governance/grants/:id`, where
 * explain's attributions, the overlap report and the People area link. The
 * sentence with each principal linked to its People page, the scope with Open
 * in Data, and Edit and Delete with `policy:update` on an explicit grant (a
 * workspace-assignment grant is read-only, as everywhere). `GET /perm/grants`
 * answers the whole list; there is no read by id, so the page finds its grant
 * there (the People area's tabs read the same list, one shared query).
 */
export default function GrantPage() {
  const { id = "" } = useParams();
  const workspaceName = useWorkspaceNames();
  const grantsQ = usePermListGrants({ query: { retry: false } });
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const queryClient = useQueryClient();
  const deleteGrant = usePermDeleteGrant({
    mutation: {
      // Stays on the page, which then says the id names no grant; the toast says why.
      onSuccess: () => {
        void queryClient.invalidateQueries({ queryKey: getPermListGrantsQueryKey() });
        toast.success("Grant deleted.");
      },
      onError: (e) => toast.error(refusalMessage(e, "policy:update", "The grant could not be deleted.")),
    },
  });

  if (grantsQ.isError) {
    return (
      <ItemFrame title="Grant">
        <TabError what="grants" onRetry={() => void grantsQ.refetch()} />
      </ItemFrame>
    );
  }
  if (grantsQ.data?.status !== 200) {
    return (
      <ItemFrame title="Grant">
        <ListSkeleton rows={4} cols={2} />
      </ItemFrame>
    );
  }
  const grant = grantsQ.data.data.find((g) => g.id === id);
  if (!grant) {
    return (
      <ItemFrame title="Grant">
        <ItemMissing text="No grant has this id; it may have been deleted." />
      </ItemFrame>
    );
  }
  return (
    <>
      <GrantBody grant={grant} workspaceName={workspaceName} onEdit={() => setEditing(true)} onDelete={() => setDeleting(true)} />
      <GrantDialog open={editing} onOpenChange={setEditing} existing={grant} />
      <ConfirmDeleteDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete "${grantSentence(grant, workspaceName)}"?`}
        description={grantDeleteDescription(grant)}
        onConfirm={() => {
          deleteGrant.mutate({ id: grant.id });
          setDeleting(false);
        }}
      />
    </>
  );
}

function ItemFrame({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="space-y-6 p-4">
      <ItemHeader kind="Grant" title={title} />
      {children}
    </div>
  );
}

function GrantBody({
  grant: g,
  workspaceName,
  onEdit,
  onDelete,
}: {
  grant: GrantView;
  workspaceName: WorkspaceNames;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const can = usePolicyPermissions();
  const catalogNameById = useCatalogNameById();
  const assignment = g.origin.type === "workspace_assignment" ? g.origin.workspace_id : null;
  const link = scopeToDataLink(g.scope, catalogNameById, "permissions");
  const patterns = patternLevels(g.scope);
  const sentence = grantSentence(g, workspaceName);

  let actions: ReactNode;
  if (assignment !== null) {
    actions = <p className="text-xs text-muted-foreground">{ASSIGNMENT_READ_ONLY}</p>;
  } else if (can("policy:update")) {
    actions = (
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
    );
  } else {
    actions = <p className="text-xs text-muted-foreground">Editing or deleting a grant needs the policy:update permission.</p>;
  }

  return (
    <div className="space-y-6 p-4">
      <ItemHeader
        kind="Grant"
        title={<GrantSentenceText grant={g} workspaceName={workspaceName} />}
        actions={actions}
      />
      <div className="flex flex-wrap items-center gap-2">
        {g.effect === "deny" ? <Badge variant="destructive">Deny</Badge> : <Badge variant="outline">Allow</Badge>}
        {assignment !== null ? <AssignmentBadge workspaceLabel={workspaceName(assignment) ?? assignment} /> : null}
      </div>
      <dl className="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[10rem_1fr]">
        <Detail term="Actions">{formatActions(g.actions)}</Detail>
        <Detail term="Scope">
          <span className="break-all font-mono">{formatScope(g.scope, g.catalog_name)}</span>
          {patterns.length > 0 ? (
            <span className="block text-xs text-muted-foreground">
              {`Covers ${patterns.map((p) => `every ${p.level} matching “${p.value}”`).join(" and ")}, not one object.`}
            </span>
          ) : null}
          {link ? (
            <span className="block">
              <OpenInDataLink to={link.to} subject={sentence} stoppedAtPattern={link.stoppedAtPattern} />
            </span>
          ) : null}
        </Detail>
        <Detail term="Principals">
          <PrincipalNames list={g.principals.include} workspaceName={workspaceName} />
        </Detail>
        {g.principals.exclude.length > 0 ? (
          <Detail term="Except">
            <PrincipalNames list={g.principals.exclude} workspaceName={workspaceName} />
          </Detail>
        ) : null}
      </dl>
    </div>
  );
}
