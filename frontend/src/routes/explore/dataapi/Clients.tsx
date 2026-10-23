import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Check, Copy, KeyRound, Search } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";
import { Badge } from "@/components/ui/badge";
import { CHIP_TONE } from "./chipTone";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListStates } from "@/components/explore/ListStates";
import { EmptyState } from "@/components/explore/EmptyState";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { cn } from "@/lib/utils";
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
// account's, so it cannot be derived the way the wrapper derives its. That is
// exactly why `pendingRevoke` carries the ACCOUNT alongside the client. (Issue
// was the same case and left with the form: it is `ClientCreate`, the page that
// owns the secret it returns.)
import { useDeleteServiceAccount } from "@/api/dataApi";
import type { DataApiClientDto, DataApiServiceAccountDto } from "@/api/generated/model";
import { ServiceAccountActionsMenu } from "./ServiceAccountActionsMenu";
import { Forbidden, isForbidden, refusalReason } from "@/routes/Forbidden";

/**
 * How many service-account groups render before the "Show more" button.
 *
 * Each group fires its own `useDataApiListClients` on mount — one request per
 * RENDERED account. `FilterableTable`'s `pageSize={25}` used to cap that
 * implicitly, by only ever mounting one page of rows; this table is
 * hand-rolled (see the component doc) and has no pagination, so the cap has to
 * be kept explicitly or a workspace with two hundred accounts fires two
 * hundred requests on first paint. 25 is the number every control-plane list
 * page uses for the same reason (`Workspaces.tsx`).
 *
 * **The "Show N more service accounts" button is load-bearing, not decorative.**
 * The summary strip counts every account the search matched, while the table
 * renders only the first `GROUP_PAGE` of them, so 30 accounts read as "30
 * service accounts" over 25 visible groups. Those two numbers reconcile only
 * because the button names the remainder — drop it (or replace it with an
 * infinite scroll that has no count) and the strip becomes a number the page
 * contradicts.
 */
const GROUP_PAGE = 25;

/**
 * Shorten a client id for display, keeping both ends.
 *
 * A client id is a 36-character UUID. Nobody reads one; what a reader does with
 * it is paste it into a consumer's config, which is what the copy button is
 * for. Both ends are kept because that is what makes two ids distinguishable at
 * a glance. Ids at or under the threshold are returned unchanged rather than
 * "shortened" into something longer.
 *
 * Done in JS, not by CSS `truncate`: a CSS ellipsis depends on the column's
 * measured width, which cannot be asserted in jsdom at all, and would put the
 * full 36 characters in the accessibility tree anyway.
 */
export function shortClientId(id: string): string {
  return id.length > 14 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

/** The client whose revocation is being confirmed, and the account that owns it. */
interface PendingRevoke {
  client: DataApiClientDto;
  account: DataApiServiceAccountDto;
}

/**
 * Clients — the OAuth2 clients that call the Data API, grouped by the service
 * account that holds them.
 *
 * **A client is a ROW; a service account is a GROUP HEADER.** It used to be the
 * other way round: each row *was* a service account, and its clients rendered
 * inline in that row's "Clients" cell as a variable-length list of running text
 * — name, a 36-character UUID, a badge, a last-used phrase and a Revoke, per
 * entry, with no columns. Nothing aligned down the page, and an irreversible
 * action sat as body text between two timestamps.
 *
 * The table is hand-rolled (`<table className="w-full text-sm">`, `thead
 * text-muted-foreground`, `tr border-t hover:bg-accent`) rather than built on
 * `FilterableTable`, for two reasons. `FilterableTable` cannot model grouping —
 * it sorts and paginates one flat entity — and its header/row styling differs
 * visibly from the sibling BI lists (`Charts.tsx`, `Datasets.tsx`,
 * `Dashboards.tsx`, `Notebooks.tsx`, `Endpoints.tsx`), which are what this page
 * sits beside in the sidebar. Grouping by `<tbody>` is the same structure
 * `Charts.tsx` and `Endpoints.tsx` use for folders.
 *
 * `AccountGroup` is its own component (not an inline function called per
 * account) specifically so it can call `useDataApiListClients` — one query per
 * account, same shape as `AccessSection.tsx`'s per-row `useDataApiGetOpenapi`
 * probe; hooks cannot be called from inside a `.map()` callback that isn't its
 * own component.
 *
 * **One noun, said once.** The sidebar and the `h1` say *Clients*; the search
 * box and the summary strip say *service accounts*. They are different things
 * — an account holds clients — so the page description says that outright,
 * rather than leaving two labels that read as synonyms which disagree.
 */
export default function Clients() {
  const ws = useParams().workspace!;

  const { data: envelope, isLoading, isError, error } = useDataApiListServiceAccounts(ws);
  const accounts = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);

  const deleteMutation = useDeleteServiceAccount();
  const revokeMutation = useDataApiRevokeClient();
  const qc = useQueryClient();

  const [query, setQuery] = useState("");
  const [pendingDelete, setPendingDelete] = useState<DataApiServiceAccountDto | null>(null);
  // Both confirmations belong to the PAGE, not to the group that raised them —
  // the same rule `ServiceAccountActionsMenu` records for Delete. A dialog
  // rendered inside a `<tbody>` would also be a dialog inside a table.
  const [pendingRevoke, setPendingRevoke] = useState<PendingRevoke | null>(null);
  const [visible, setVisible] = useState(GROUP_PAGE);

  // Issuing a client is no longer state on this page. It was two dialogs — a
  // form, then the one-time secret — and the second opened as the first was
  // unmounted while still open, which is how a stuck `pointer-events: none`
  // lands on `document.body`. Both are now `ClientCreate`, a route, and the
  // group menu's "Issue client…" is a link to it.

  // `hasData` is the UNFILTERED count on purpose: the first-run CTA must not
  // appear just because a search missed. A search that narrows to zero is the
  // no-match line below, which is a different sentence for a different
  // situation.
  const hasData = accounts.length > 0;

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matched = q
      ? accounts.filter((a) => `${a.name} ${a.email}`.toLowerCase().includes(q))
      : accounts.slice();
    // Sorted here rather than left in server order, and tie-broken on `id`, so
    // two accounts sharing a name cannot swap places between renders.
    return matched.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  }, [accounts, query]);

  const shown = rows.slice(0, visible);
  const hidden = rows.length - shown.length;

  // The server refused the page's own read: the shared refusal, not a red
  // box with "forbidden" under a title bar that offers actions (KI-231).
  if (isForbidden(error)) {
    return <Forbidden reason={refusalReason(error, "Viewing clients needs the service_account:read permission in this workspace.")} />;
  }

  return (
    // The canonical Explore list-page scaffold — `p-4` + a single `h1` — comes
    // from `ListPageShell`. Clients is its own top-level sidebar item, not a
    // section nested under a `DataApiLayout` shell (that shell, which used to
    // own the page's `h1` and add `p-6`, was removed once Data API got its own
    // sidebar group), so the heading and the padding still belong to this route
    // — they just come from the shared shell now.
    <ListPageShell
      title="Clients"
      description="A service account holds the OAuth2 clients that call your endpoints. Each client below is one credential: revoke it and the consumer using it stops minting tokens."
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
        skeletonCols={5}
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
            five sibling Explore pages put theirs. It filters SERVICE ACCOUNTS,
            which is what this page holds a list of — a client's own name lives
            in a per-account query this component never sees. */}
        <div data-testid="page-toolbar" className="mb-3 flex flex-wrap items-center gap-2">
          <div className="relative max-w-xs flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              className="pl-8"
              placeholder="Search service accounts…"
              value={query}
              // The reveal resets with the query. Searching after a "Show more"
              // otherwise carries the raised limit into a result set it was
              // never about, and — worse in the other direction — clearing a
              // search leaves the page showing everything, so the cap this
              // button exists to enforce is silently off. Done in the handler
              // rather than an effect: a `setState` in an effect body is what
              // `react-hooks/set-state-in-effect` is for.
              onChange={(e) => { setQuery(e.target.value); setVisible(GROUP_PAGE); }}
            />
          </div>
        </div>

        <div className="mb-3">
          <SummaryStrip
            items={[{ label: "service accounts", singular: "service account", value: rows.length }]}
          />
        </div>

        {rows.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <th className="px-2 font-medium">Client</th>
                  <th className="px-2 font-medium">ID</th>
                  <th className="px-2 font-medium">Status</th>
                  <th className="px-2 font-medium">Last used</th>
                  <th className="w-8" />
                </tr>
              </thead>
              {shown.map((a) => (
                <AccountGroup
                  key={a.id}
                  ws={ws}
                  account={a}
                  onDelete={() => setPendingDelete(a)}
                  onRevoke={(client) => setPendingRevoke({ client, account: a })}
                />
              ))}
            </table>
          </div>
        ) : (
          <p className="text-muted-foreground">No service accounts match.</p>
        )}

        {hidden > 0 && (
          <div className="mt-3">
            <Button variant="outline" size="sm" onClick={() => setVisible((v) => v + GROUP_PAGE)}>
              Show {hidden} more service {hidden === 1 ? "account" : "accounts"}
            </Button>
          </div>
        )}
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

      <ConfirmDeleteDialog
        open={pendingRevoke !== null}
        onOpenChange={(o) => { if (!o) setPendingRevoke(null); }}
        title="Revoke client"
        description={`Revoke "${pendingRevoke?.client.name ?? ""}"? Any consumer using this client immediately loses the ability to mint new tokens. This cannot be undone.`}
        confirmText="revoke"
        confirmLabel="Revoke"
        onConfirm={async () => {
          if (!pendingRevoke) return;
          try {
            await revokeMutation.mutateAsync({ workspace: ws, id: pendingRevoke.client.id });
            // The list key is per SERVICE ACCOUNT while the mutation's
            // variables carry the CLIENT's id — which is why the account rides
            // along in `pendingRevoke`. Keying this off the client id would
            // leave the revoked row on screen wearing an Active badge.
            void qc.invalidateQueries({
              queryKey: getDataApiListClientsQueryKey(ws, pendingRevoke.account.id),
            });
            toast.success("Client revoked");
          } catch (err) {
            toast.error(String(err));
          }
          setPendingRevoke(null);
        }}
      />
    </ListPageShell>
  );
}

/**
 * One service account: a group header row, then one row per client it holds.
 *
 * Renders a `<tbody>` — the same grouping structure `Charts.tsx` and
 * `Endpoints.tsx` use for folders — so a reader can tell which account a
 * client belongs to without clicking anything (no expand, no row select).
 *
 * It owns the client QUERY and nothing destructive: both confirmations belong
 * to the page (see `Clients`). ISSUE left this file entirely, because it acts
 * on the account and returns a secret: it is now the `ClientCreate` route,
 * reached by the group menu's "Issue client…" link, and the one-time secret
 * went with it. Nothing changed about the rule that made that safe: the client
 * list is driven by `useDataApiListClients`, whose `DataApiClientDto` shape has
 * no secret field at all — not omitted, absent — so there is no cell here that
 * could even reach for one.
 */
function AccountGroup({ ws, account, onDelete, onRevoke }: {
  ws: string;
  account: DataApiServiceAccountDto;
  onDelete: () => void;
  onRevoke: (client: DataApiClientDto) => void;
}) {
  const { data: envelope, isLoading, isError } = useDataApiListClients(ws, account.id);
  const clients = useMemo(() => (envelope?.status === 200 ? envelope.data : []), [envelope]);
  const sorted = useMemo(
    () => clients.slice().sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id)),
    [clients],
  );

  return (
    <tbody>
      <tr className="border-t bg-muted/40">
        {/* `role="rowheader"`, the same idiom `ConfigTableGroup` uses for a
            group row, so assistive tech reads the account as the heading of
            the rows beneath it rather than as another data cell. `scope` is
            deliberately absent: it is a `<th>`-only attribute and inert here. */}
        <td colSpan={4} role="rowheader" className="px-2 py-1">
          <div className="flex flex-wrap items-baseline gap-x-2">
            <span className="font-medium">{account.name}</span>
            <span className="font-mono text-xs text-muted-foreground">{account.email}</span>
            {/* Only once the list has settled: a count printed while the query
                is in flight says "0 clients" about an account that has four. */}
            {!isLoading && !isError && (
              <span className="text-xs text-muted-foreground">
                {sorted.length === 1 ? "1 client" : `${sorted.length} clients`}
              </span>
            )}
            <span className="text-xs text-muted-foreground">
              created <RelativeTime iso={account.created_at} />
            </span>
          </div>
        </td>
        <td className="px-2 py-1 text-right">
          <ServiceAccountActionsMenu ws={ws} account={account} onDelete={onDelete} />
        </td>
      </tr>

      {/* The three states are rows INSIDE the table (ui-conventions §4.3), not
          a branch that replaces it: a branch takes the column headers with it,
          and then an account whose clients failed to load sits under nothing
          that names what a client id or a last-used even is. `isError` is
          checked before the empty case (§2.1/§2.2) — folding them together is
          how a failed fetch comes to claim there is nothing there. */}
      {isLoading ? (
        <StateRow>Loading clients…</StateRow>
      ) : isError ? (
        <StateRow className="text-destructive">Could not load clients.</StateRow>
      ) : sorted.length === 0 ? (
        <StateRow>
          No clients yet.{" "}
          <Link className="underline" to={`/explore/${ws}/data-api/clients/${account.id}/new-client`}>
            Issue one…
          </Link>
        </StateRow>
      ) : (
        sorted.map((c) => <ClientRow key={c.id} client={c} onRevoke={() => onRevoke(c)} />)
      )}
    </tbody>
  );
}

/** A loading / error / empty line that keeps the table's columns on screen. */
function StateRow({ className, children }: { className?: string; children: React.ReactNode }) {
  return (
    <tr className="border-t">
      <td colSpan={5} className={cn("px-2 py-1 pl-6 text-muted-foreground", className)}>
        {children}
      </td>
    </tr>
  );
}

/**
 * One OAuth2 client: Client · ID · Status · Last used · actions.
 *
 * **Revoked is not encoded by a strikethrough, and not by colour alone.** The
 * old line struck through the client's NAME and left the UUID beside it
 * unstruck, so it read half-cancelled. Status is a column now: the badge
 * carries the word "Revoked" (the part a screen reader and a monochrome
 * display both get), it differs in FORM from the other two (outline, not
 * filled), the row is muted, and the actions cell is empty because there is
 * nothing left to do to it.
 */
function ClientRow({ client: c, onRevoke }: { client: DataApiClientDto; onRevoke: () => void }) {
  const revoked = !!c.revoked_at;
  // `new Date(...) < new Date()`, not `.getTime() < Date.now()` — the same
  // idiom `SharedObjects.tsx` uses for its own "expired" badge, which keeps
  // this out of the `Date.now`-is-impure lint rule that a direct `Date.now()`
  // call in a render-time `.map()` trips.
  const expired = !revoked && !!c.expires_at && new Date(c.expires_at) < new Date();

  return (
    <tr className={cn("border-t hover:bg-accent", revoked && "text-muted-foreground")}>
      <td className="px-2 py-1 pl-6 font-medium">{c.name}</td>
      <td className="px-2 py-1">
        <span className="inline-flex items-center gap-1">
          <span className="font-mono text-xs" title={c.client_id}>{shortClientId(c.client_id)}</span>
          <CopyIdButton id={c.client_id} clientName={c.name} />
        </span>
      </td>
      <td className="px-2 py-1">
        <div className="flex flex-wrap items-center gap-x-2">
          {revoked ? (
            <Badge variant="outline" className="text-muted-foreground">Revoked</Badge>
          ) : expired ? (
            /* Ink is `--foreground` on both washes, not the tint's own hue —
               `chipTone.ts` carries the measurements. The three states are told
               apart by their WORDS, which is why flattening the ink costs
               nothing a reader was using. */
            <Badge className={CHIP_TONE.destructive}>Expired</Badge>
          ) : (
            <Badge className={CHIP_TONE.success}>Active</Badge>
          )}
          {revoked ? (
            <span className="text-xs text-muted-foreground">
              revoked <RelativeTime iso={c.revoked_at!} />
            </span>
          ) : c.expires_at ? (
            <span className="text-xs text-muted-foreground">
              expires <RelativeTime iso={c.expires_at} />
            </span>
          ) : null}
        </div>
      </td>
      <td className="px-2 py-1 text-muted-foreground">
        {c.last_used_at ? <RelativeTime iso={c.last_used_at} /> : "Never"}
      </td>
      <td className="px-2 py-1 text-right">
        {/* Gated on `revoked`, not `revoked || expired`: expiry and revocation
            are different lifecycle events on the backend (`expires_at` vs
            `revoked_at`, independently settable), and an expired-but-not-yet-
            revoked client is still a real row an operator may want to revoke
            explicitly. A revoked one has nothing left to act on, so it gets no
            menu rather than a menu that is empty. */}
        {!revoked && (
          <RowActionsMenu label={`Actions for ${c.name}`}>
            <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onRevoke}>
              Revoke…
            </DropdownMenuItem>
          </RowActionsMenu>
        )}
      </td>
    </tr>
  );
}

/** Copies the FULL client id — the cell shows only the short form. Same
 *  "Copy"/"Copied" idiom as `TryIt.tsx` and `ClientCreate.tsx`, shrunk to an
 *  icon because it sits in a table cell. */
function CopyIdButton({ id, clientName }: { id: string; clientName: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="h-6 w-6 p-0 text-muted-foreground"
      // The label names the client, not just "Copy": a page with eight of these
      // otherwise offers eight controls with the same accessible name.
      aria-label={`Copy client id for ${clientName}`}
      onClick={() => {
        void navigator.clipboard?.writeText(id);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </Button>
  );
}
