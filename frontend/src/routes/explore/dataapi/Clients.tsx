import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { KeyRound, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { EmptyState } from "@/components/explore/EmptyState";
import { RelativeTime } from "@/components/explore/RelativeTime";
import {
  getDataApiListClientsQueryKey,
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
// Revoke CLIENT stays generated + invalidated locally below: its key is per
// service account and `revoke`'s variables carry the client id, not the owning
// account's, so it cannot be derived the way the wrapper derives its. (Issue was
// the same case and left with the form: it is `ClientCreate`, the page that owns
// the secret it returns.)
import { useDeleteServiceAccount } from "@/api/dataApi";
import type { DataApiClientDto, DataApiServiceAccountDto } from "@/api/generated/model";
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

  const { data: envelope, isLoading, isError, error } = useDataApiListServiceAccounts(ws);
  const accounts = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const deleteMutation = useDeleteServiceAccount();

  const [query, setQuery] = useState("");
  const [pendingDelete, setPendingDelete] = useState<DataApiServiceAccountDto | null>(null);
  // Issuing a client is no longer state on this page. It was two dialogs — a
  // form, then the one-time secret — and the second opened as the first was
  // unmounted while still open, which is how a stuck `pointer-events: none`
  // lands on `document.body`. Both are now `ClientCreate`, a route, and the row
  // menu's "Issue client…" is a link to it.

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
        <ServiceAccountActionsMenu ws={ws} account={a} onDelete={() => setPendingDelete(a)} />
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
 * ISSUE left this file entirely, because it acts on the account and returns a
 * secret: it is now the `ClientCreate` route, reached by the row menu's "Issue
 * client…" link, and the one-time secret went with it — rendered in that page's
 * created state, held in its component state, and never in this table's data.
 * Nothing changed about the rule this file exists to hold: the client list is
 * driven by
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
