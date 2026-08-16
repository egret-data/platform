import { useMemo, useState } from "react";
import { useParams } from "react-router-dom";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import {
  getDataApiListClientsQueryKey,
  useDataApiCreateServiceAccount,
  useDataApiDeleteServiceAccount,
  useDataApiIssueClient,
  useDataApiListClients,
  useDataApiListServiceAccounts,
  useDataApiRevokeClient,
} from "@/api/generated/data-api/data-api";
import type { DataApiClientDto, DataApiNewClientDto, DataApiServiceAccountDto } from "@/api/generated/model";

/**
 * Clients — service accounts and the OAuth2 clients each one owns.
 *
 * Two levels, one table: each row IS a service account (the entity
 * `FilterableTable` sorts/searches/paginates over — see `DatabasesPage`, the
 * only other page that hand this component a plain, ungrouped list); its
 * clients render inline in that same row's "Clients" cell via `ClientsCell`.
 * That is deliberate, not a shortcut around nesting `FilterableTable`: a
 * reader must be able to tell which account a client belongs to without
 * clicking anything (no expand, no row select), and a cell rendered directly
 * in the account's own row satisfies that by construction. `ClientsCell` is
 * its own component (not an inline function called per row) specifically so
 * it can call `useDataApiListClients` — one query per account, same shape as
 * `AccessSection.tsx`'s per-row `useDataApiGetOpenapi` probe; hooks cannot be
 * called from inside a `.map()` callback that isn't its own component.
 */
export default function Clients() {
  const ws = useParams().workspace!;

  const { data: envelope, isLoading, isError, error } = useDataApiListServiceAccounts(ws);
  const accounts = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const createMutation = useDataApiCreateServiceAccount();
  const deleteMutation = useDataApiDeleteServiceAccount();

  const [createOpen, setCreateOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<DataApiServiceAccountDto | null>(null);

  const hasData = accounts.length > 0;

  const columns: Column<DataApiServiceAccountDto>[] = [
    {
      key: "name",
      label: "Service account",
      sortable: true,
      value: (a) => a.name,
      cell: (a) => (
        <div>
          <div className="font-medium">{a.name}</div>
          <div className="font-mono text-xs text-muted-foreground">{a.email}</div>
        </div>
      ),
    },
    {
      key: "created_at",
      label: "Created",
      sortable: true,
      value: (a) => a.created_at,
      cell: (a) => <RelativeTime iso={a.created_at} className="text-muted-foreground" />,
    },
    {
      key: "clients",
      label: "Clients",
      cell: (a) => <ClientsCell ws={ws} account={a} />,
    },
    {
      key: "actions",
      label: "",
      className: "text-right",
      cell: (a) => (
        <Button
          variant="ghost"
          size="sm"
          className="text-muted-foreground hover:text-destructive"
          aria-label={`Delete ${a.name}`}
          onClick={() => setPendingDelete(a)}
        >
          Delete
        </Button>
      ),
    },
  ];

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Clients</h1>
        <Button onClick={() => setCreateOpen(true)}>+ Service account</Button>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : isError ? (
        <Alert variant="destructive">
          {/* Same cast as Endpoints.tsx: the generated hook types the error as
              the response body shape, but the mutator throws the real ApiError
              it parsed that body into. */}
          <AlertDescription>
            {(error as unknown as Error)?.message ?? "Could not load service accounts. Try again in a moment."}
          </AlertDescription>
        </Alert>
      ) : hasData ? (
        <>
          <div className="mb-3">
            <SummaryStrip items={[{ label: "service accounts", value: accounts.length }]} />
          </div>
          <FilterableTable
            rows={accounts}
            columns={columns}
            getId={(a) => a.id}
            searchPlaceholder="Search service accounts…"
            initialSort={{ key: "name", dir: "asc" }}
            // A search that narrows the account list to zero is not "no data" —
            // FilterableTable's own `noMatchText` default ("No matches.") covers
            // it. `emptyText` is passed a plain string, not the first-run CTA,
            // per the resolution: this branch can only be reached via an
            // external facet (there is none here beyond search today), and a
            // CTA offering "create your first" while data plainly exists would
            // be the same first-run/empty conflation the endpoints list fixed.
            emptyText="No service accounts match."
          />
        </>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No service accounts yet. Create one to issue OAuth2 clients for the Data API.</p>
          <Button className="mt-3" onClick={() => setCreateOpen(true)}>Create your first service account</Button>
        </div>
      )}

      <CreateServiceAccountDialog ws={ws} open={createOpen} onOpenChange={setCreateOpen} create={createMutation} />

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete service account"
        description={`Delete "${pendingDelete?.name ?? ""}"? This takes its grants and every client it owns with it — any consumer using one of its clients loses access immediately. The backend hard-deletes; this cannot be undone.`}
        onConfirm={async () => {
          if (!pendingDelete) return;
          try {
            await deleteMutation.mutateAsync({ workspace: ws, id: pendingDelete.id });
            toast.success("Service account deleted");
          } catch (err) {
            toast.error(String(err));
          }
          setPendingDelete(null);
        }}
      />
    </div>
  );
}

/**
 * One account's clients, rendered inline in that account's own table row.
 *
 * Owns the issue/revoke flow for this account: the "+ New client" button,
 * the issue dialog, and — the one rule this task exists to hold — the
 * one-time secret dialog. `newClient` (the `DataApiNewClientDto` the issue
 * call returned) lives ONLY in this component's local state, is shown in
 * exactly one dialog, and is discarded (`setNewClient(null)`) the moment that
 * dialog closes. It is never written into the client list below: that list
 * is driven by `useDataApiListClients`, whose `DataApiClientDto` shape has no
 * secret field at all — not omitted, absent — so there is no cell here that
 * could even reach for one.
 */
function ClientsCell({ ws, account }: { ws: string; account: DataApiServiceAccountDto }) {
  const { data: envelope, isLoading, isError } = useDataApiListClients(ws, account.id);
  const clients = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const issueMutation = useDataApiIssueClient();
  const revokeMutation = useDataApiRevokeClient();
  const qc = useQueryClient();

  const [issueOpen, setIssueOpen] = useState(false);
  const [newClient, setNewClient] = useState<DataApiNewClientDto | null>(null);
  const [pendingRevoke, setPendingRevoke] = useState<DataApiClientDto | null>(null);

  function invalidate() {
    void qc.invalidateQueries({ queryKey: getDataApiListClientsQueryKey(ws, account.id) });
  }

  return (
    <div className="min-w-[18rem] space-y-1.5">
      {isLoading ? (
        <span className="text-xs text-muted-foreground">Loading…</span>
      ) : isError ? (
        <span className="text-xs text-destructive">Could not load clients.</span>
      ) : clients.length === 0 ? (
        <p className="text-xs text-muted-foreground">No clients yet.</p>
      ) : (
        <ul className="space-y-1">
          {clients.map((c) => {
            const revoked = !!c.revoked_at;
            // `new Date(...)`, not `.getTime() < Date.now()` — same idiom
            // `SharedObjects.tsx` uses for its own "expired" badge, which
            // keeps this out of the `Date.now`-is-impure lint rule that a
            // direct `Date.now()` call in a render-time `.map()` trips.
            const expired = !revoked && !!c.expires_at && new Date(c.expires_at) < new Date();
            return (
              <li key={c.id} className="flex flex-wrap items-center gap-1.5 text-xs">
                <span className={revoked ? "text-muted-foreground line-through" : "font-medium"}>{c.name}</span>
                <span className="font-mono text-muted-foreground">{c.client_id}</span>
                {revoked ? (
                  <Badge variant="outline" className="text-muted-foreground">Revoked</Badge>
                ) : expired ? (
                  <Badge className="border-transparent bg-destructive/15 text-destructive">Expired</Badge>
                ) : (
                  <Badge className="border-transparent bg-success/15 text-success">Active</Badge>
                )}
                {c.last_used_at ? (
                  <span className="text-muted-foreground">
                    last used <RelativeTime iso={c.last_used_at} />
                  </span>
                ) : (
                  !revoked && <span className="text-muted-foreground">never used</span>
                )}
                {c.expires_at && (
                  <span className="text-muted-foreground">
                    expires <RelativeTime iso={c.expires_at} />
                  </span>
                )}
                {!revoked && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-1.5 text-muted-foreground hover:text-destructive"
                    aria-label={`Revoke ${c.name}`}
                    onClick={() => setPendingRevoke(c)}
                  >
                    Revoke
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <Button variant="outline" size="sm" onClick={() => setIssueOpen(true)}>+ New client</Button>

      <IssueClientDialog
        ws={ws}
        account={account}
        open={issueOpen}
        onOpenChange={setIssueOpen}
        issue={issueMutation}
        onIssued={(created) => {
          invalidate();
          setNewClient(created);
        }}
      />

      <NewClientSecretDialog client={newClient} onOpenChange={(o) => { if (!o) setNewClient(null); }} />

      <ConfirmDeleteDialog
        open={pendingRevoke !== null}
        onOpenChange={(o) => { if (!o) setPendingRevoke(null); }}
        title="Revoke client"
        description={`Revoke "${pendingRevoke?.name ?? ""}"? Any consumer using this client immediately loses the ability to mint new tokens. This cannot be undone.`}
        confirmText="revoke"
        confirmLabel="Revoke"
        onConfirm={async () => {
          if (!pendingRevoke) return;
          try {
            await revokeMutation.mutateAsync({ workspace: ws, id: pendingRevoke.id });
            invalidate();
            toast.success("Client revoked");
          } catch (err) {
            toast.error(String(err));
          }
          setPendingRevoke(null);
        }}
      />
    </div>
  );
}

/**
 * Per the Alerts lesson (#485, also followed by `CreateEndpointDialog`): this
 * dialog collects only what a client cannot exist without — its name, plus
 * the one field `IssueClientInput` allows to be set at mint time and never
 * again (`expires_at`) — and hands off to `NewClientSecretDialog` for the
 * result.
 */
function IssueClientDialog({ ws, account, open, onOpenChange, issue, onIssued }: {
  ws: string;
  account: DataApiServiceAccountDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  issue: ReturnType<typeof useDataApiIssueClient>;
  onIssued: (created: DataApiNewClientDto) => void;
}) {
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState("");

  // Reset on CLOSE, not via a `useEffect` keyed on `open` — see
  // `CreateServiceAccountDialog.reset` for why.
  function reset() {
    setName("");
    setExpiresAt("");
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit = name.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    try {
      const res = await issue.mutateAsync({
        workspace: ws,
        id: account.id,
        data: {
          name: name.trim(),
          expires_at: expiresAt.trim() ? new Date(expiresAt).toISOString() : undefined,
        },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — same pattern as
      // CreateEndpointDialog.
      if (res.status !== 201) { toast.error("Could not issue client."); return; }
      reset();
      onOpenChange(false);
      onIssued(res.data);
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New client for {account.name}</DialogTitle>
          <DialogDescription>
            Issues a fresh OAuth2 client_credentials pair. The secret is shown once, right after this.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="client-name">Name</Label>
            <Input id="client-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="etl" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="client-expiry">Expires (optional)</Label>
            <Input id="client-expiry" type="date" value={expiresAt} onChange={(e) => setExpiresAt(e.target.value)} />
            <p className="text-xs text-muted-foreground">Leave blank for a client that lives until revoked.</p>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => handleOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={!canSubmit || issue.isPending}>
            {issue.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The one place `client_secret` is ever rendered. `client` is
 * `DataApiNewClientDto` — "the only type in the crate carrying secret
 * material" per its own doc comment — held in `ClientsCell`'s local state
 * only until this dialog closes. There is exactly one copy affordance, for
 * the secret; the client id is shown as plain text so `getByRole("button",
 * { name: /copy/i })` (and a reader scanning the dialog) finds one unambiguous
 * button, not two.
 */
function NewClientSecretDialog({ client, onOpenChange }: {
  client: DataApiNewClientDto | null;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={client !== null} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Client created</DialogTitle>
          <DialogDescription>
            Copy the secret now — it cannot be shown again. If it&apos;s lost, issue a new client and revoke
            this one.
          </DialogDescription>
        </DialogHeader>
        {client && (
          <div className="space-y-3">
            <div className="space-y-1.5">
              <Label>Client ID</Label>
              <code className="block truncate rounded bg-muted px-2 py-1.5 text-xs">{client.client_id}</code>
            </div>
            <div className="space-y-1.5">
              <Label>Client secret</Label>
              <div className="flex items-center gap-2">
                <code className="flex-1 truncate rounded bg-muted px-2 py-1.5 text-xs">{client.client_secret}</code>
                <CopyButton value={client.client_secret} />
              </div>
            </div>
            <div className="space-y-1.5">
              <Label>Token endpoint</Label>
              <code className="block truncate rounded bg-muted px-2 py-1.5 text-xs">{client.token_endpoint}</code>
            </div>
          </div>
        )}
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Same "Copy"/"Copied" idiom as `TryIt.tsx`'s `CopyButton` / `WarehouseDetail.tsx`. */
function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

function CreateServiceAccountDialog({ ws, open, onOpenChange, create }: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  create: ReturnType<typeof useDataApiCreateServiceAccount>;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");

  // Reset on CLOSE (every path: Cancel, Esc, overlay click, successful
  // submit), not in a `useEffect` keyed on `open` — the effect version sets
  // state synchronously during a commit and trips `react-hooks/set-state-in-
  // effect`. Same idiom `DatabasesPage.tsx`'s `resetToConn`/`closeAndReset`
  // use.
  function reset() {
    setName("");
    setDescription("");
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit = name.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    try {
      const res = await create.mutateAsync({
        workspace: ws,
        data: { name: name.trim(), description: description.trim() || undefined },
      });
      if (res.status !== 201) { toast.error("Could not create service account."); return; }
      toast.success("Service account created");
      reset();
      onOpenChange(false);
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>New service account</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="sa-name">Name</Label>
            <Input id="sa-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="reporting-bot" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="sa-description">Description</Label>
            <Textarea
              id="sa-description"
              rows={2}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional"
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => handleOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={!canSubmit || create.isPending}>
            {create.isPending ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
