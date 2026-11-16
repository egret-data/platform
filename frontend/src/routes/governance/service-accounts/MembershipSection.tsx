import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, X } from "lucide-react";
import { ApiError } from "@/api/client";
import { listGroups, listRoles } from "@/api/access";
import {
  addServiceAccountGroup,
  addServiceAccountRole,
  removeServiceAccountGroup,
  removeServiceAccountRole,
  serviceAccountQueryKey,
  type ServiceAccountDetailDto,
} from "@/api/serviceAccounts";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Section } from "@/routes/governance/tabParts";

/**
 * What a refused membership change says. The server's guards: a group the
 * identity provider owns (409), `platform_admin` (never grantable to a
 * machine identity) and a group that holds it. The server's own sentence is
 * kept (FE-ACTION-3); a refusal with no message gets a plain fallback.
 */
export function membershipErrorText(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.message && e.message !== "Conflict" && e.message !== "Bad Request" && e.message !== "Forbidden") {
      return e.message;
    }
    if (e.status === 409) return "This group's membership is owned by the identity provider, so it can't be changed here.";
    if (e.status === 400 || e.status === 403) {
      return "A service account can never hold platform_admin, directly or through a group.";
    }
  }
  return e instanceof Error && e.message ? e.message : "The change could not be saved. Try again.";
}

type Kind = "group" | "role";

function MemberList({
  kind,
  names,
  pending,
  onRemove,
}: {
  kind: Kind;
  names: string[];
  pending: boolean;
  onRemove: (name: string) => void;
}) {
  if (names.length === 0) {
    return <p className="text-sm text-muted-foreground">No {kind}s yet.</p>;
  }
  return (
    <ul className="flex flex-wrap gap-2">
      {names.map((name) => (
        <li key={name} className="inline-flex items-center gap-1 rounded-md border bg-muted/40 py-0.5 pl-2 pr-0.5 text-sm">
          <span>{name}</span>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6"
            aria-label={`Remove ${name}`}
            disabled={pending}
            onClick={() => onRemove(name)}
          >
            <X aria-hidden className="h-3.5 w-3.5" />
          </Button>
        </li>
      ))}
    </ul>
  );
}

function AddPicker({
  kind,
  options,
  loading,
  failed,
  pending,
  onAdd,
}: {
  kind: Kind;
  options: { id: string; name: string }[];
  loading: boolean;
  failed: boolean;
  pending: boolean;
  onAdd: (id: string) => void;
}) {
  const id = useId();
  const [picked, setPicked] = useState("");
  const label = kind === "group" ? "Add to group" : "Grant role";
  const reason = loading
    ? `Loading ${kind}s.`
    : failed
      ? `The ${kind}s could not be loaded.`
      : options.length === 0
        ? `No more ${kind}s to add.`
        : null;
  return (
    <div className="flex flex-wrap items-end gap-2">
      <div className="space-y-1">
        <Label htmlFor={id}>{label}</Label>
        <Select value={picked} onValueChange={setPicked} disabled={reason !== null}>
          <SelectTrigger id={id} className="w-56">
            <SelectValue placeholder={`Choose a ${kind}`} />
          </SelectTrigger>
          <SelectContent>
            {options.map((o) => (
              <SelectItem key={o.id} value={o.id}>
                {o.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <Button
        variant="outline"
        disabled={!picked || pending}
        onClick={() => {
          onAdd(picked);
          setPicked("");
        }}
      >
        <Plus aria-hidden />
        {kind === "group" ? "Add group" : "Add role"}
      </Button>
      {reason ? <span className="text-sm text-muted-foreground">{reason}</span> : null}
    </div>
  );
}

/** The groups and platform roles a service account holds, each removable, and a picker for more. */
export function MembershipSection({ account }: { account: ServiceAccountDetailDto }) {
  const id = account.user_id;
  const qc = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const groups = useQuery({ queryKey: ["access", "groups"], queryFn: listGroups });
  const roles = useQuery({ queryKey: ["access", "roles"], queryFn: listRoles });

  const settle = {
    onMutate: () => setError(null),
    onSuccess: () => qc.invalidateQueries({ queryKey: serviceAccountQueryKey(id) }),
    onError: (e: unknown) => setError(membershipErrorText(e)),
  };
  const addGroup = useMutation({ mutationFn: (groupId: string) => addServiceAccountGroup(id, { group_id: groupId }), ...settle });
  const addRole = useMutation({
    mutationFn: (roleId: string) => addServiceAccountRole(id, { role_id: roleId, scope_type: "platform" }),
    ...settle,
  });
  const removeGroup = useMutation({ mutationFn: (groupId: string) => removeServiceAccountGroup(id, groupId), ...settle });
  const removeRole = useMutation({ mutationFn: (roleId: string) => removeServiceAccountRole(id, roleId), ...settle });

  const allGroups = groups.data ?? [];
  const platformRoles = (roles.data ?? []).filter((r) => r.scope === "platform");
  const busy = addGroup.isPending || addRole.isPending || removeGroup.isPending || removeRole.isPending;

  // The detail carries names; the remove routes want ids.
  const removeBy = (rows: { id: string; name: string }[], run: (id: string) => void) => (name: string) => {
    const row = rows.find((r) => r.name === name);
    if (row) run(row.id);
    else setError(`Couldn't find "${name}" to remove it. Reload and try again.`);
  };

  return (
    <Section title="Membership">
      {error ? (
        <Alert variant="destructive" role="alert">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <div className="space-y-6">
        <div className="space-y-3">
          <h3 className="text-sm font-medium">Groups</h3>
          {groups.isPending ? (
            <Skeleton className="h-7 w-48" />
          ) : (
            <MemberList kind="group" names={account.groups} pending={busy} onRemove={removeBy(allGroups, removeGroup.mutate)} />
          )}
          <AddPicker
            kind="group"
            options={allGroups.filter((g) => !account.groups.includes(g.name))}
            loading={groups.isPending}
            failed={groups.isError}
            pending={busy}
            onAdd={addGroup.mutate}
          />
        </div>
        <div className="space-y-3">
          <h3 className="text-sm font-medium">Platform roles</h3>
          {roles.isPending ? (
            <Skeleton className="h-7 w-48" />
          ) : (
            <MemberList kind="role" names={account.roles} pending={busy} onRemove={removeBy(platformRoles, removeRole.mutate)} />
          )}
          <AddPicker
            kind="role"
            options={platformRoles.filter((r) => !account.roles.includes(r.name))}
            loading={roles.isPending}
            failed={roles.isError}
            pending={busy}
            onAdd={addRole.mutate}
          />
        </div>
      </div>
    </Section>
  );
}
