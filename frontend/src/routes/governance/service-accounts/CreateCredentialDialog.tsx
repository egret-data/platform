import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listRoles, listWarehouses } from "@/api/controlPlane";
import {
  createCredential,
  credentialsQueryKey,
  type Audience,
  type CreateCredentialBody,
  type CreatedCredential,
  type ServiceAccountDetailDto,
} from "@/api/serviceAccounts";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DateTimePicker } from "@/components/ui/date-time-picker";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { asCreated, credentialErrorText } from "@/routes/governance/service-accounts/credentialErrors";

const CONSUMER_TYPES = ["engine", "external"] as const;

interface Props {
  account: ServiceAccountDetailDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Hands the created credential (with its one-time secret) to the caller. */
  onCreated: (created: CreatedCredential) => void;
}

/** Create a credential: audience, an optional name and expiry, and for `catalog` its binding. */
export function CreateCredentialDialog({ account, open, onOpenChange, onCreated }: Props) {
  const qc = useQueryClient();
  const platform = account.scope === "platform";
  const ids = { audience: useId(), name: useId(), expiry: useId(), warehouse: useId(), consumer: useId(), role: useId() };
  const [audience, setAudience] = useState<Audience>(platform ? "catalog" : "data-api");
  const [name, setName] = useState("");
  const [expiry, setExpiry] = useState<Date | null>(null);
  const [warehouseId, setWarehouseId] = useState("");
  const [consumerType, setConsumerType] = useState("");
  const [roleId, setRoleId] = useState("");
  const [error, setError] = useState<string | null>(null);

  const warehouses = useQuery({ queryKey: ["catalog-warehouses"], queryFn: listWarehouses, enabled: open && audience === "catalog" });
  const roles = useQuery({
    queryKey: ["catalog-roles", warehouseId],
    queryFn: () => listRoles(warehouseId),
    enabled: open && audience === "catalog" && warehouseId !== "",
  });
  // A workspace account can only bind its own workspace's warehouses.
  const warehouseOptions = (warehouses.data ?? []).filter((w) => platform || w.workspaceId === account.workspace_id);
  const roleOptions = (roles.data ?? []).filter((r) => r.consumerType === consumerType);

  const create = useMutation({
    mutationFn: async (body: CreateCredentialBody) => asCreated<CreatedCredential>(await createCredential(account.user_id, body)),
    onSuccess: async (created) => {
      await qc.invalidateQueries({ queryKey: credentialsQueryKey(account.user_id) });
      reset();
      onCreated(created);
    },
    onError: (e) => setError(credentialErrorText(e)),
  });

  const reset = () => {
    setName("");
    setExpiry(null);
    setWarehouseId("");
    setConsumerType("");
    setRoleId("");
    setError(null);
  };

  const catalogIncomplete = audience === "catalog" && (warehouseId === "" || consumerType === "" || roleId === "");
  const submit = () => {
    setError(null);
    const body: CreateCredentialBody = {
      audience,
      name: name.trim() === "" ? null : name.trim(),
      expires_at: expiry ? expiry.toISOString() : null,
      catalog: audience === "catalog" ? { warehouse_id: warehouseId, catalog_role_id: roleId, consumer_type: consumerType } : null,
    };
    create.mutate(body);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) reset();
        onOpenChange(o);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create credential</DialogTitle>
          <DialogDescription>The secret is shown once, right after you create the credential.</DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-1.5">
            <Label htmlFor={ids.audience}>Audience</Label>
            <Select value={audience} onValueChange={(v) => setAudience(v as Audience)}>
              <SelectTrigger id={ids.audience}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="data-api" disabled={platform}>
                  Data API
                </SelectItem>
                <SelectItem value="catalog">Catalog</SelectItem>
              </SelectContent>
            </Select>
            {platform ? (
              <p className="text-xs text-muted-foreground">
                Data API credentials are tied to a workspace, so a platform-scoped service account can&apos;t have one.
              </p>
            ) : null}
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={ids.name}>Name (optional)</Label>
            <Input id={ids.name} value={name} onChange={(e) => setName(e.target.value)} placeholder="Defaults to the account and audience" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor={ids.expiry}>Expires (optional)</Label>
            <DateTimePicker id={ids.expiry} value={expiry} onChange={setExpiry} placeholder="Never" />
          </div>
          {audience === "catalog" ? (
            <fieldset className="space-y-4 rounded-md border p-3">
              <legend className="px-1 text-sm font-medium">Catalog binding</legend>
              <div className="space-y-1.5">
                <Label htmlFor={ids.warehouse}>Warehouse</Label>
                <Select
                  value={warehouseId}
                  onValueChange={(v) => {
                    setWarehouseId(v);
                    setRoleId("");
                  }}
                >
                  <SelectTrigger id={ids.warehouse}>
                    <SelectValue placeholder={warehouses.isPending ? "Loading warehouses" : "Choose a warehouse"} />
                  </SelectTrigger>
                  <SelectContent>
                    {warehouseOptions.map((w) => (
                      <SelectItem key={w.id} value={w.id}>
                        {w.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {warehouses.isError ? <p className="text-xs text-destructive">The warehouses could not be loaded.</p> : null}
              </div>
              <div className="space-y-1.5">
                <Label htmlFor={ids.consumer}>Consumer type</Label>
                <Select
                  value={consumerType}
                  onValueChange={(v) => {
                    setConsumerType(v);
                    setRoleId("");
                  }}
                >
                  <SelectTrigger id={ids.consumer}>
                    <SelectValue placeholder="Choose a consumer type" />
                  </SelectTrigger>
                  <SelectContent>
                    {CONSUMER_TYPES.map((t) => (
                      <SelectItem key={t} value={t}>
                        {t === "engine" ? "Engine" : "External"}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor={ids.role}>Catalog role</Label>
                <Select value={roleId} onValueChange={setRoleId} disabled={warehouseId === "" || consumerType === ""}>
                  <SelectTrigger id={ids.role}>
                    <SelectValue placeholder={warehouseId === "" || consumerType === "" ? "Choose a warehouse and consumer type first" : "Choose a role"} />
                  </SelectTrigger>
                  <SelectContent>
                    {roleOptions.map((r) => (
                      <SelectItem key={r.id} value={r.id}>
                        {r.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {roles.isError ? <p className="text-xs text-destructive">The roles could not be loaded.</p> : null}
              </div>
            </fieldset>
          ) : null}
          {error ? (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={create.isPending || catalogIncomplete}>
            Create credential
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
