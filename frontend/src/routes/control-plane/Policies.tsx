import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Pencil, Plus, ShieldCheck, Trash2 } from "lucide-react";
import {
  type Profile, type PolicySummary, type AccessPolicyInput, type MaskingPolicyInput, type RowFilterPolicyInput,
  listProfiles, listPolicies, deletePolicy, getPolicy,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@/components/ui/table";
import { AccessPolicyDialog } from "@/components/policy/AccessPolicyDialog";
import { MaskingPolicyDialog } from "@/components/policy/MaskingPolicyDialog";
import { RowFilterPolicyDialog } from "@/components/policy/RowFilterPolicyDialog";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

// Inverse of the backend's access_to_ranger_json: turns a raw Ranger policy
// body (as returned by GET /control-plane/policies/:id) back into the shape
// the AccessPolicyDialog editor works with.
function toInput(service: string, body: Record<string, any>): AccessPolicyInput {
  const lvl = (k: string) => ({
    values: body?.resources?.[k]?.values ?? [],
    isExcludes: !!body?.resources?.[k]?.isExcludes,
    isRecursive: !!body?.resources?.[k]?.isRecursive,
  });
  const map = (arr: any[], effect: "allow" | "deny") =>
    (arr ?? []).map((it: any) => ({
      users: it.users ?? [],
      groups: it.groups ?? [],
      accesses: (it.accesses ?? []).map((a: any) => a.type),
      effect,
    }));
  const items = [...map(body.policyItems, "allow"), ...map(body.denyPolicyItems, "deny")];
  return {
    name: String(body.name ?? ""),
    service,
    isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table"), column: lvl("column") },
    items: items.length ? items : [{ users: [], groups: [], accesses: [], effect: "allow" }],
  };
}

function maskingToInput(service: string, body: Record<string, any>): MaskingPolicyInput {
  const lvl = (k: string) => ({
    values: body?.resources?.[k]?.values ?? [],
    isExcludes: !!body?.resources?.[k]?.isExcludes,
    isRecursive: !!body?.resources?.[k]?.isRecursive,
  });
  const items = (body.dataMaskPolicyItems ?? []).map((it: any) => ({
    users: it.users ?? [],
    groups: it.groups ?? [],
    maskType: it.dataMaskInfo?.dataMaskType ?? "MASK_HASH",
    valueExpr: it.dataMaskInfo?.valueExpr,
  }));
  return {
    kind: "masking",
    name: String(body.name ?? ""),
    service,
    isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table"), column: lvl("column") },
    items: items.length ? items : [{ users: [], groups: [], maskType: "MASK_HASH" }],
  };
}

function rowfilterToInput(service: string, body: Record<string, any>): RowFilterPolicyInput {
  const lvl = (k: string) => ({
    values: body?.resources?.[k]?.values ?? [],
    isExcludes: !!body?.resources?.[k]?.isExcludes,
    isRecursive: !!body?.resources?.[k]?.isRecursive,
  });
  const items = (body.rowFilterPolicyItems ?? []).map((it: any) => ({
    users: it.users ?? [],
    groups: it.groups ?? [],
    filterExpr: it.rowFilterInfo?.filterExpr ?? "",
  }));
  return {
    kind: "rowfilter",
    name: String(body.name ?? ""),
    service,
    isEnabled: body.isEnabled !== false,
    resources: { catalog: lvl("catalog"), schema: lvl("schema"), table: lvl("table") },
    items: items.length ? items : [{ users: [], groups: [], filterExpr: "" }],
  };
}

export default function Policies() {
  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const [service, setService] = useState<string | null>(null);
  const [policyType, setPolicyType] = useState<(typeof TYPES)[number]["key"]>("access");

  const activeService = service ?? profiles.data?.[0]?.name ?? null;

  const policies = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", activeService, policyType],
    queryFn: () => listPolicies(activeService ?? "", policyType),
    enabled: !!activeService,
    retry: false,
  });

  const rows = policies.data ?? [];

  const queryClient = useQueryClient();
  const [editorOpen, setEditorOpen] = useState(false);
  const [editing, setEditing] = useState<{ id: number; input: any } | undefined>(undefined);
  const [deleteTarget, setDeleteTarget] = useState<PolicySummary | null>(null);
  const [editError, setEditError] = useState<string | null>(null);
  const [configOpen, setConfigOpen] = useState(false);

  const del = useMutation({
    mutationFn: (id: number) => deletePolicy(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] }),
  });

  const openNew = () => { setEditError(null); setEditing(undefined); setEditorOpen(true); };
  const openEdit = async (id: number) => {
    setEditError(null);
    try {
      const body = await getPolicy(id) as Record<string, unknown>;
      const input = policyType === "masking" ? maskingToInput(activeService!, body)
        : policyType === "rowfilter" ? rowfilterToInput(activeService!, body)
        : toInput(activeService!, body);
      setEditing({ id, input });
      setEditorOpen(true);
    } catch (e) {
      setEditError(e instanceof Error ? e.message : "Failed to load policy");
    }
  };

  const selectType = (t: (typeof TYPES)[number]["key"]) => {
    setPolicyType(t);
    setEditorOpen(false);
  };

  return (
    <div className="space-y-4 p-6">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-5 w-5" />
          <h1 className="text-xl font-semibold">Policies</h1>
        </div>
        {activeService && (
          <Button size="sm" onClick={openNew}>
            <Plus className="mr-1 h-4 w-4" /> New policy
          </Button>
        )}
      </div>

      {editError && (
        <Alert variant="destructive">
          <AlertDescription>{editError}</AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Profiles</CardTitle>
          <CardDescription>Ranger policy sets. Each engine enforces one profile.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-2">
          {profiles.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {profiles.error?.message ?? "Failed to load profiles."}
              </AlertDescription>
            </Alert>
          ) : profiles.isLoading ? (
            <span className="text-sm text-muted-foreground">Loading…</span>
          ) : (
            (profiles.data ?? []).map((p) => (
              <button
                key={p.id}
                onClick={() => setService(p.name)}
                className={`rounded-md border px-3 py-1.5 text-sm ${
                  p.name === activeService ? "bg-accent text-accent-foreground" : "text-muted-foreground"
                }`}
              >
                {p.name}
                {p.engines.length > 0 && (
                  <span className="ml-2 text-xs opacity-70">{p.engines.map((e) => e.name).join(", ")}</span>
                )}
              </button>
            ))
          )}
        </CardContent>
        {activeService && (
          <CardContent className="border-t pt-4">
            <Button size="sm" variant="outline" onClick={() => setConfigOpen(true)}>
              <Download className="mr-1 h-4 w-4" /> Download Trino config
            </Button>
          </CardContent>
        )}
      </Card>

      <div className="flex gap-1 border-b" role="tablist">
        {TYPES.map((t) => (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={policyType === t.key}
            onClick={() => selectType(t.key)}
            className={`px-3 py-2 text-sm ${policyType === t.key ? "border-b-2 border-primary font-medium" : "text-muted-foreground"}`}
          >
            {t.label}
          </button>
        ))}
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Resource</TableHead>
            <TableHead>Principals</TableHead>
            <TableHead>Permissions</TableHead>
            <TableHead>Enabled</TableHead>
            <TableHead className="text-right">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.id}>
              <TableCell className="font-medium">{r.name}</TableCell>
              <TableCell className="font-mono text-xs">{r.resource_path}</TableCell>
              <TableCell>{r.principals.join(", ")}</TableCell>
              <TableCell>{r.permissions.join(", ")}</TableCell>
              <TableCell>
                <Badge variant={r.is_enabled ? "secondary" : "outline"}>
                  {r.is_enabled ? "enabled" : "disabled"}
                </Badge>
              </TableCell>
              <TableCell className="text-right">
                <div className="flex justify-end gap-1">
                  <Button variant="ghost" size="icon" aria-label={`Edit ${r.name}`} onClick={() => openEdit(r.id)}>
                    <Pencil className="h-4 w-4" />
                  </Button>
                  <Button variant="ghost" size="icon" aria-label={`Delete ${r.name}`} onClick={() => setDeleteTarget(r)}>
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          ))}
          {rows.length === 0 && (
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={6} className="text-center text-muted-foreground">
                {policies.isError ? (
                  <span className="text-destructive">
                    {policies.error?.message ?? "Failed to load policies."}
                  </span>
                ) : !activeService || policies.isLoading ? (
                  "Loading…"
                ) : (
                  `No ${policyType} policies for this profile.`
                )}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>

      {activeService && policyType === "access" && (
        <AccessPolicyDialog
          open={editorOpen}
          service={activeService}
          initial={editing}
          onOpenChange={setEditorOpen}
          onSaved={() => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] })}
        />
      )}
      {activeService && policyType === "masking" && (
        <MaskingPolicyDialog
          open={editorOpen}
          service={activeService}
          initial={editing}
          onOpenChange={setEditorOpen}
          onSaved={() => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] })}
        />
      )}
      {activeService && policyType === "rowfilter" && (
        <RowFilterPolicyDialog
          open={editorOpen}
          service={activeService}
          initial={editing}
          onOpenChange={setEditorOpen}
          onSaved={() => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] })}
        />
      )}
      {activeService && (
        <TrinoConfigDialog open={configOpen} service={activeService} onOpenChange={setConfigOpen} />
      )}

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete "${deleteTarget.name}"?` : "Delete policy?"}
        description="This policy will be permanently removed from Ranger."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) del.mutate(deleteTarget.id);
          setDeleteTarget(null);
        }}
      />
    </div>
  );
}
