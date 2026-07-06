import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Pencil, Plus, ShieldCheck, Trash2 } from "lucide-react";
import {
  type Profile, type PolicySummary,
  listProfiles, listPolicies, deletePolicy, setPolicyEnabled,
} from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { Switch } from "@/components/ui/switch";
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@/components/ui/table";
import { TrinoConfigDialog } from "@/components/policy/TrinoConfigDialog";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

export default function Policies() {
  const navigate = useNavigate();
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
  const [deleteTarget, setDeleteTarget] = useState<PolicySummary | null>(null);
  const [configOpen, setConfigOpen] = useState(false);

  const del = useMutation({
    mutationFn: (id: number) => deletePolicy(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] }),
  });

  const [togglingId, setTogglingId] = useState<number | null>(null);
  const toggleEnabled = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) => setPolicyEnabled(id, enabled),
    onMutate: ({ id }) => setTogglingId(id),
    onSettled: () => setTogglingId(null),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] }),
  });

  const goToNew = () => {
    if (!activeService) return;
    navigate(`/control-plane/policies/${encodeURIComponent(activeService)}/${policyType}/new`);
  };
  const goToEdit = (id: number) => {
    if (!activeService) return;
    navigate(`/control-plane/policies/${encodeURIComponent(activeService)}/${policyType}/${id}`);
  };

  return (
    <div className="space-y-4 p-6">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-5 w-5" />
          <h1 className="text-xl font-semibold">Policies</h1>
        </div>
        {activeService && (
          <Button size="sm" onClick={goToNew}>
            <Plus className="mr-1 h-4 w-4" /> New policy
          </Button>
        )}
      </div>

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
            onClick={() => setPolicyType(t.key)}
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
            <TableHead>Labels</TableHead>
            <TableHead>Enabled</TableHead>
            <TableHead className="text-right">Actions</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.id}>
              <TableCell className="font-medium">
                {r.name}
                {r.description && (
                  <div className="text-xs font-normal text-muted-foreground">{r.description}</div>
                )}
              </TableCell>
              <TableCell className="font-mono text-xs">{r.resource_path}</TableCell>
              <TableCell>{r.principals.join(", ")}</TableCell>
              <TableCell>{r.permissions.join(", ")}</TableCell>
              <TableCell>
                {r.labels && r.labels.length > 0 ? (
                  <div className="flex flex-wrap gap-1">
                    {r.labels.map((l) => (
                      <Badge key={l} variant="secondary">{l}</Badge>
                    ))}
                  </div>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </TableCell>
              <TableCell>
                <Switch
                  aria-label={`toggle ${r.name}`}
                  checked={r.is_enabled}
                  disabled={toggleEnabled.isPending && togglingId === r.id}
                  onCheckedChange={(c) => toggleEnabled.mutate({ id: r.id, enabled: c })}
                />
              </TableCell>
              <TableCell className="text-right">
                <div className="flex justify-end gap-1">
                  <Button variant="ghost" size="icon" aria-label={`Edit ${r.name}`} onClick={() => goToEdit(r.id)}>
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
              <TableCell colSpan={7} className="text-center text-muted-foreground">
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
