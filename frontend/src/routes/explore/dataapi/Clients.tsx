import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { KeyRound, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { EmptyState } from "@/components/explore/EmptyState";
import { RelativeTime } from "@/components/explore/RelativeTime";
import {
  getDataApiListClientsQueryKey,
  useDataApiIssueClient,
  useDataApiListClients,
  useDataApiListServiceAccounts,
  useDataApiRevokeClient,
} from "@/api/generated/data-api/data-api";
// Delete comes from the hand-written wrapper, which owns its invalidation
// (`src/api/dataApi.ts`). That key is shared by five surfaces — this table,
// Try-it's picker, the Access checklist, the OpenAPI download dialog and the
// Response preview — so a delete wired straight to the generated hook left a
// removed account in all five for 30 seconds. (Create used the same wrapper
// when this page owned the create dialog; it moved to `ServiceAccountCreate`
// with the form, still through the wrapper, which is why the row is already
// in this table when that page navigates back here.)
// Issue/revoke CLIENT stay generated + invalidated locally below: their key is
// per service account and `revoke`'s variables carry the client id, not the
// owning account's, so it cannot be derived the way the wrapper derives its.
import { useDeleteServiceAccount } from "@/api/dataApi";
import type { DataApiClientDto, DataApiNewClientDto, DataApiServiceAccountDto } from "@/api/generated/model";
import { ServiceAccountActionsMenu } from "./ServiceAccountActionsMenu";

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
  const qc = useQueryClient();

  const { data: envelope, isLoading, isError, error } = useDataApiListServiceAccounts(ws);
  const accounts = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const deleteMutation = useDeleteServiceAccount();
  const issueMutation = useDataApiIssueClient();

  const [query, setQuery] = useState("");
  const [pendingDelete, setPendingDelete] = useState<DataApiServiceAccountDto | null>(null);
  // Issue lives on the PAGE, not in the row's menu: a `DropdownMenuItem`
  // unmounts the whole menu on click, so a dialog rendered inside one closes
  // the instant it is opened. `pendingIssue` names which account the open form
  // belongs to, the same way `pendingDelete` does.
  const [pendingIssue, setPendingIssue] = useState<DataApiServiceAccountDto | null>(null);
  const [newClient, setNewClient] = useState<DataApiNewClientDto | null>(null);

  // `hasData` is the UNFILTERED count on purpose: the first-run CTA must not
  // appear just because a search missed. A search that narrows to zero is
  // `FilterableTable`'s empty body, below.
  const hasData = accounts.length > 0;

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return accounts;
    return accounts.filter((a) => `${a.name} ${a.email}`.toLowerCase().includes(q));
  }, [accounts, query]);

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
        <ServiceAccountActionsMenu
          account={a}
          onIssueClient={() => setPendingIssue(a)}
          onDelete={() => setPendingDelete(a)}
        />
      ),
    },
  ];

  return (
    // The canonical Explore list-page scaffold — `p-4` + a single `h1` — now
    // comes from `ListPageShell` rather than being hand-written here. Clients
    // is its own top-level sidebar item, not a section nested under a
    // `DataApiLayout` shell (that shell, which used to own the page's `h1` and
    // add `p-6`, was removed once Data API got its own sidebar group), so the
    // heading and the padding still belong to this route — they just come from
    // the shared shell now.
    <ListPageShell
      title="Clients"
      // Both entry points are real LINKS to `clients/new`, not buttons that
      // push the route imperatively: a button looks identical on screen but
      // cannot be middle-clicked, opened in a new tab, or copied.
      action={<Button asChild><Link to={`/explore/${ws}/data-api/clients/new`}>+ Service account</Link></Button>}
    >
      <ListStates
        isLoading={isLoading}
        isError={isError}
        // The generated hook types its error as the response BODY shape, but
        // the mutator (`mutator.ts`) throws the real `ApiError` it parsed that
        // body into — `ListStates` takes `unknown` and does the
        // `(error as Error)?.message || errorText` read itself.
        error={error}
        errorText="Could not load service accounts. Try again in a moment."
        hasData={hasData}
        skeletonCols={4}
        empty={
          <EmptyState
            variant="dashed"
            icon={KeyRound}
            title="No service accounts yet"
            description="Create one to issue OAuth2 clients for the Data API."
            action={<Button asChild><Link to={`/explore/${ws}/data-api/clients/new`}>Create your first service account</Link></Button>}
          />
        }
      >
        {/* Search sits in the page toolbar, above the summary strip, where all
            five sibling Explore pages put theirs — `FilterableTable`'s own box
            renders below the strip, inside the table block, which is the one
            place no other list page has it. `FilterableTable` has no
            controlled-filter prop (its `q` is private state), so this page
            filters `rows` itself and suppresses that box with `hideSearch`;
            two search inputs one row apart would be worse than either. */}
        <div data-testid="page-toolbar" className="mb-3 flex flex-wrap items-center gap-2">
          <div className="relative max-w-xs flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              className="pl-8"
              placeholder="Search service accounts…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
        </div>

        <div className="mb-3">
          <SummaryStrip items={[{ label: "service accounts", value: rows.length }]} />
        </div>

        <FilterableTable
          rows={rows}
          columns={columns}
          getId={(a) => a.id}
          hideSearch
          initialSort={{ key: "name", dir: "asc" }}
          // With `hideSearch` the component's own `q` never leaves `""`, so it
          // can no longer tell a first-run empty from a search miss and always
          // renders `emptyText`. That is correct here: the genuine first-run
          // case is `ListStates`' `empty` above and never reaches this table,
          // so the only way to see an empty body is a toolbar search that
          // matched nothing. `noMatchText` is therefore left unset.
          emptyText="No service accounts match."
          // Each row's `ClientsCell` fires its own `useDataApiListClients`
          // on mount — one request per VISIBLE service account, uncapped
          // without this. `FilterableTable` only mounts `pageRows` (the
          // current page's slice, see FilterableTable.tsx's `pageRows`),
          // not the full `shown` set, so paging here caps concurrent
          // requests at `pageSize` regardless of how many accounts exist.
          // 25 matches every other control-plane list page that sets this
          // for the same reason (Workspaces.tsx, Warehouses.tsx, etc.).
          pageSize={25}
        />
      </ListStates>

      {/* Keyed on the account id so switching rows remounts the form rather
          than carrying the previous account's half-typed name across. */}
      {pendingIssue && (
        <IssueClientDialog
          key={pendingIssue.id}
          ws={ws}
          account={pendingIssue}
          open
          onOpenChange={(o) => { if (!o) setPendingIssue(null); }}
          issue={issueMutation}
          onIssued={(created, accountId) => {
            void qc.invalidateQueries({ queryKey: getDataApiListClientsQueryKey(ws, accountId) });
            setNewClient(created);
          }}
        />
      )}

      <NewClientSecretDialog client={newClient} onOpenChange={(o) => { if (!o) setNewClient(null); }} />

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
    </ListPageShell>
  );
}

/**
 * One account's clients, rendered inline in that account's own table row.
 *
 * Lists this account's clients and owns the REVOKE flow only. Revoke acts on
 * one client credential, so it belongs on that client's own line — not in the
 * account's row menu, which acts on the account.
 *
 * ISSUE moved up to the page (`pendingIssue`), because it acts on the account:
 * it is the row menu's "Issue client…" item. That took the one-time secret
 * dialog with it — `newClient` is page state now, still shown in exactly one
 * dialog and still discarded the moment that dialog closes. Nothing changed
 * about the rule this file exists to hold: the client list below is driven by
 * `useDataApiListClients`, whose `DataApiClientDto` shape has no secret field
 * at all — not omitted, absent — so there is no cell here that could even
 * reach for one.
 */
function ClientsCell({ ws, account }: { ws: string; account: DataApiServiceAccountDto }) {
  const { data: envelope, isLoading, isError } = useDataApiListClients(ws, account.id);
  const clients = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const revokeMutation = useDataApiRevokeClient();
  const qc = useQueryClient();

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
                {/* Gated on `revoked`, not `revoked || expired`: expiry and
                    revocation are different lifecycle events on the backend
                    (`expires_at` vs `revoked_at`, independently settable), and
                    an expired-but-not-yet-revoked client is still a real row
                    an operator may want to revoke explicitly (e.g. to record
                    why, or before its expiry is trusted). */}
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
  /** `accountId` is echoed back rather than re-read from the page's state: the
   *  caller invalidates a per-account query key, and the account whose form
   *  this was must be the account that key names. */
  onIssued: (created: DataApiNewClientDto, accountId: string) => void;
}) {
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState("");

  // Reset on CLOSE (every path: Cancel, Esc, overlay click, successful
  // submit), not in a `useEffect` keyed on `open` — the effect version sets
  // state synchronously during a commit and trips `react-hooks/set-state-in-
  // effect`. Same idiom `DatabasesPage.tsx`'s `resetToConn`/`closeAndReset`
  // use.
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
      onIssued(res.data, account.id);
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
 * material" per its own doc comment — held in the page's local state
 * only until this dialog closes. There is exactly one copy affordance, for
 * the secret; the client id is shown as plain text so `getByRole("button",
 * { name: /copy/i })` (and a reader scanning the dialog) finds one unambiguous
 * button, not two.
 *
 * Known cosmetic gap: `open` is driven by `client !== null`, and `Done`
 * calls `onOpenChange(false)` which nulls `client` synchronously in the same
 * tick — so Radix's close animation plays over an already-empty body rather
 * than the last-shown content. No security implication (the secret is exactly
 * as gone either way); not fixed here because the natural fix — caching the
 * last-shown value in its own state and updating it inside a `useEffect` — is
 * exactly the `react-hooks/set-state-in-effect` shape this file went out of
 * its way to avoid elsewhere (see `IssueClientDialog`'s `reset()` comment).
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
