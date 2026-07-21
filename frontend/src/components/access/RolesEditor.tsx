import { useEffect, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";

import type { AccessRole } from "@/api/access";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const SCOPE_LABEL: Record<string, string> = {
  platform: "Platform",
  workspace: "Workspace",
};

/**
 * A role-assignment table shared by the user and group detail pages. Assigned
 * roles show as rows (with a remove ×); new ones are added by browsing scope →
 * role and confirming with Add — the same pattern as the role detail page's
 * permission editor. Assignment names are matched back to roles by name (the
 * principal DTOs carry role *names*, not ids).
 */
export function RolesEditor({
  assignedRoleNames,
  allRoles,
  canEdit,
  onAdd,
  onRemove,
}: {
  assignedRoleNames: string[];
  allRoles: AccessRole[];
  canEdit: boolean;
  onAdd: (roleId: string) => void;
  onRemove: (roleId: string) => void;
}) {
  const roleByName = useMemo(
    () => new Map(allRoles.map((r) => [r.name, r])),
    [allRoles],
  );
  const assigned = useMemo(
    () =>
      assignedRoleNames
        .map((n) => roleByName.get(n))
        .filter((r): r is AccessRole => !!r),
    [assignedRoleNames, roleByName],
  );

  const assignedNames = useMemo(() => new Set(assignedRoleNames), [assignedRoleNames]);
  const availableByScope = useMemo(() => {
    const byScope = new Map<string, AccessRole[]>();
    for (const r of allRoles) {
      if (assignedNames.has(r.name)) continue;
      const arr = byScope.get(r.scope) ?? [];
      arr.push(r);
      byScope.set(r.scope, arr);
    }
    return byScope;
  }, [allRoles, assignedNames]);
  const scopes = [...availableByScope.keys()];

  const [addScope, setAddScope] = useState("");
  const [pendingRole, setPendingRole] = useState("");
  useEffect(() => {
    if (addScope && !availableByScope.has(addScope)) setAddScope("");
  }, [availableByScope, addScope]);
  useEffect(() => setPendingRole(""), [addScope]);
  const rolesInScope = availableByScope.get(addScope) ?? [];

  return (
    <div className="space-y-3">
      {canEdit && (
        <div className="flex flex-wrap items-end gap-2">
          <div className="space-y-1">
            <Label className="text-xs text-muted-foreground">Scope</Label>
            <Select value={addScope} onValueChange={setAddScope} disabled={scopes.length === 0}>
              <SelectTrigger className="w-40" aria-label="Role scope">
                <SelectValue placeholder="Choose a scope" />
              </SelectTrigger>
              <SelectContent>
                {scopes.map((s) => (
                  <SelectItem key={s} value={s}>
                    {SCOPE_LABEL[s] ?? s}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1">
            <Label className="text-xs text-muted-foreground">Role</Label>
            <Select value={pendingRole} onValueChange={setPendingRole} disabled={!addScope}>
              <SelectTrigger className="w-64" aria-label="Role to add">
                <SelectValue placeholder={addScope ? "Select a role" : "Choose a scope first"} />
              </SelectTrigger>
              <SelectContent>
                {rolesInScope.map((r) => (
                  <SelectItem key={r.id} value={r.id}>
                    {r.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button
            disabled={!pendingRole}
            onClick={() => {
              if (pendingRole) {
                onAdd(pendingRole);
                setPendingRole("");
              }
            }}
          >
            <Plus className="mr-1 h-4 w-4" />
            Add
          </Button>
        </div>
      )}

      <div className="rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Role</TableHead>
              <TableHead>Scope</TableHead>
              <TableHead className="w-10 text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {assigned.length === 0 ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={3} className="py-8 text-center text-muted-foreground">
                  No roles assigned yet.
                  {canEdit ? " Pick a scope above to grant one." : ""}
                </TableCell>
              </TableRow>
            ) : (
              assigned.map((r) => (
                <TableRow key={r.id}>
                  <TableCell className="font-medium">{r.name}</TableCell>
                  <TableCell>
                    <Badge variant="outline" className="font-normal">
                      {SCOPE_LABEL[r.scope] ?? r.scope}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-right">
                    {canEdit ? (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${r.name}`}
                        onClick={() => onRemove(r.id)}
                      >
                        <X className="h-4 w-4" />
                      </Button>
                    ) : null}
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
