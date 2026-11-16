import { useState } from "react";
import dayjs from "dayjs";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { CalendarClock, Plus, RefreshCw } from "lucide-react";
import { listRoles, listWarehouses } from "@/api/controlPlane";
import {
  credentialsQueryKey,
  listCredentials,
  revokeCredential,
  rotateCredential,
  setCredentialExpiry,
  type CreatedCredential,
  type Credential,
  type ServiceAccountDetailDto,
} from "@/api/serviceAccounts";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { DATE_TIME_FORMAT, DateTimePicker } from "@/components/ui/date-time-picker";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { CreateCredentialDialog } from "@/routes/governance/service-accounts/CreateCredentialDialog";
import { asCreated, asList, credentialErrorText } from "@/routes/governance/service-accounts/credentialErrors";
import { SecretReveal } from "@/routes/governance/service-accounts/SecretReveal";
import { Section, TabError } from "@/routes/governance/tabParts";

const STATUS_TONE_OF: Record<Credential["status"], StatusTone> = {
  active: "success",
  revoked: "inactive",
  expired: "destructive",
};

const when = (iso: string | null | undefined) => (iso ? dayjs(iso).format(DATE_TIME_FORMAT) : null);

function ExpiryDialog({
  credential,
  pending,
  onClose,
  onSave,
}: {
  credential: Credential;
  pending: boolean;
  onClose: () => void;
  onSave: (expiresAt: string | null) => void;
}) {
  const [value, setValue] = useState<Date | null>(credential.expires_at ? new Date(credential.expires_at) : null);
  return (
    <Dialog open onOpenChange={(o) => (o ? undefined : onClose())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Set expiry for {credential.name}</DialogTitle>
          <DialogDescription>A time in the past expires the credential now. Clearing it removes the expiry.</DialogDescription>
        </DialogHeader>
        <div className="space-y-1.5">
          <Label htmlFor="credential-expiry">Expires</Label>
          <DateTimePicker id="credential-expiry" value={value} onChange={setValue} placeholder="Never" />
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button type="button" variant="outline" disabled={pending} onClick={() => onSave(null)}>
            Clear expiry
          </Button>
          <Button type="button" disabled={pending || value === null} onClick={() => value && onSave(value.toISOString())}>
            Save expiry
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** One service account's credentials (SA3): list, create, rotate, revoke, set expiry. */
export function CredentialsPanel({ account }: { account: ServiceAccountDetailDto }) {
  const id = account.user_id;
  const qc = useQueryClient();
  const credentials = useQuery({ queryKey: credentialsQueryKey(id), queryFn: async () => asList<Credential>(await listCredentials(id)) });
  const [creating, setCreating] = useState(false);
  // The one-time secret lives here and nowhere else; closing the reveal drops it.
  const [revealed, setRevealed] = useState<CreatedCredential | null>(null);
  const [revoking, setRevoking] = useState<Credential | null>(null);
  const [expiring, setExpiring] = useState<Credential | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const rows = credentials.data ?? [];
  const catalogRows = rows.filter((c) => c.catalog);
  const warehouseIds = [...new Set(catalogRows.map((c) => c.catalog?.warehouse_id ?? ""))];
  const warehouses = useQuery({ queryKey: ["catalog-warehouses"], queryFn: listWarehouses, enabled: catalogRows.length > 0 });
  const roleQueries = useQueries({
    queries: warehouseIds.map((w) => ({ queryKey: ["catalog-roles", w], queryFn: () => listRoles(w) })),
  });
  // A name that cannot be resolved (a deleted warehouse, a slow load) falls back to its id.
  const warehouseName = (wid: string) => warehouses.data?.find((w) => w.id === wid)?.name ?? wid;
  const roleName = (wid: string, rid: string) => {
    const q = roleQueries[warehouseIds.indexOf(wid)];
    return q?.data?.find((r) => r.id === rid)?.name ?? rid;
  };

  const refresh = () => qc.invalidateQueries({ queryKey: credentialsQueryKey(id) });
  const fail = (e: unknown) => setActionError(credentialErrorText(e));

  const rotate = useMutation({
    mutationFn: async (c: Credential) => asCreated<CreatedCredential>(await rotateCredential(id, c.id)),
    onMutate: () => setActionError(null),
    onSuccess: async (created) => {
      await refresh();
      setRevealed(created);
    },
    onError: fail,
  });
  const revoke = useMutation({
    mutationFn: (c: Credential) => revokeCredential(id, c.id),
    onMutate: () => setActionError(null),
    onSuccess: refresh,
    onError: fail,
  });
  const expiry = useMutation({
    mutationFn: (v: { credential: Credential; expiresAt: string | null }) =>
      setCredentialExpiry(id, v.credential.id, { expires_at: v.expiresAt }),
    onMutate: () => setActionError(null),
    onSuccess: async () => {
      await refresh();
      setExpiring(null);
    },
    onError: (e) => {
      fail(e);
      setExpiring(null);
    },
  });

  return (
    <Section title="Credentials">
      <p className="text-sm text-muted-foreground">
        Credentials let a client act as this service account. Each one is for a single audience.
      </p>
      <Alert role="note">
        <AlertDescription>
          Catalog credentials read the data lake (Iceberg) directly and are outside egret&apos;s data-governance masking,
          row-filter and clearance. Disable, revoke or expire the credential (or the service account) to control them, not
          engine masks.
        </AlertDescription>
      </Alert>
      <div>
        <Button type="button" size="sm" onClick={() => setCreating(true)}>
          <Plus aria-hidden className="mr-2 h-4 w-4" />
          Create credential
        </Button>
      </div>
      {actionError ? (
        <Alert variant="destructive">
          <AlertDescription>{actionError}</AlertDescription>
        </Alert>
      ) : null}
      {credentials.isPending ? (
        <Skeleton className="h-20 w-full" />
      ) : credentials.isError ? (
        <TabError what="the credentials" onRetry={() => void credentials.refetch()} detail={credentials.error.message} />
      ) : rows.length === 0 ? (
        <p className="text-sm text-muted-foreground">No credentials yet.</p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Audience</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Binding</TableHead>
              <TableHead>Created</TableHead>
              <TableHead>Last used</TableHead>
              <TableHead>Expires</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((c) => (
              <TableRow key={c.id}>
                <TableCell className="font-medium">{c.name}</TableCell>
                <TableCell>
                  <Badge variant="outline">{c.audience === "catalog" ? "Catalog" : "Data API"}</Badge>
                </TableCell>
                <TableCell>
                  <StatusBadge tone={STATUS_TONE_OF[c.status]}>{c.status}</StatusBadge>
                </TableCell>
                <TableCell className="text-sm">
                  {c.catalog ? (
                    <>
                      {warehouseName(c.catalog.warehouse_id)} / {roleName(c.catalog.warehouse_id, c.catalog.catalog_role_id)}{" "}
                      <span className="text-muted-foreground">({c.catalog.consumer_type})</span>
                    </>
                  ) : (
                    <span className="text-muted-foreground">None</span>
                  )}
                </TableCell>
                <TableCell className="text-sm tabular-nums">{when(c.created_at)}</TableCell>
                <TableCell className="text-sm tabular-nums">{when(c.last_used_at) ?? "Never"}</TableCell>
                <TableCell className="text-sm tabular-nums">{when(c.expires_at) ?? "Never"}</TableCell>
                <TableCell>
                  {c.status === "revoked" ? null : (
                    <div className="flex justify-end gap-1">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        aria-label={`Rotate ${c.name}`}
                        disabled={rotate.isPending}
                        onClick={() => rotate.mutate(c)}
                      >
                        <RefreshCw aria-hidden className="mr-1 h-3.5 w-3.5" />
                        Rotate
                      </Button>
                      <Button type="button" variant="ghost" size="sm" aria-label={`Set expiry for ${c.name}`} onClick={() => setExpiring(c)}>
                        <CalendarClock aria-hidden className="mr-1 h-3.5 w-3.5" />
                        Expiry
                      </Button>
                      <Button type="button" variant="ghost" size="sm" aria-label={`Revoke ${c.name}`} onClick={() => setRevoking(c)}>
                        Revoke
                      </Button>
                    </div>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <CreateCredentialDialog
        account={account}
        open={creating}
        onOpenChange={setCreating}
        onCreated={(created) => {
          setCreating(false);
          setRevealed(created);
        }}
      />
      <SecretReveal created={revealed} onClose={() => setRevealed(null)} />
      {revoking ? (
        <ConfirmDeleteDialog
          open
          onOpenChange={(o) => (o ? undefined : setRevoking(null))}
          title={`Revoke ${revoking.name}?`}
          description="Clients using this credential stop working at once. This can't be undone."
          confirmText="revoke"
          confirmLabel="Revoke"
          onConfirm={() => revoke.mutate(revoking)}
        />
      ) : null}
      {expiring ? (
        <ExpiryDialog
          credential={expiring}
          pending={expiry.isPending}
          onClose={() => setExpiring(null)}
          onSave={(expiresAt) => expiry.mutate({ credential: expiring, expiresAt })}
        />
      ) : null}
    </Section>
  );
}
