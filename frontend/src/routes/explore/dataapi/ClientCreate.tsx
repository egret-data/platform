import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ListPageShell } from "@/components/explore/ListPageShell";
// Issue stays on the GENERATED hook, with its invalidation done here rather than
// in `src/api/dataApi.ts`: the wrapper derives the service-account list key from
// the mutation variables, and the client list key is per service account —
// `revoke`'s variables carry the client id, not the owning account's, so the two
// halves of that key cannot both be derived there. Same split `Clients.tsx` has
// used since the dialog days.
import {
  getDataApiListClientsQueryKey,
  useDataApiIssueClient,
  useDataApiListServiceAccounts,
} from "@/api/generated/data-api/data-api";
import type { DataApiNewClientDto } from "@/api/generated/model";
import { BackLink } from "@/components/layout/DetailHeader";

/**
 * The one fact about an issued client that must outlive the mount — and it is
 * NOT the secret.
 *
 * `sessionStorage`, deliberately, and it holds only the `client_id` (public: it
 * is a column in the Clients table). The secret lives in component state and
 * nowhere else, so a reload cannot recover it; this marker exists purely so the
 * reloaded page can SAY that, instead of rendering an empty field or silently
 * offering a fresh form.
 *
 * A module-level variable would pass the same test — a fresh mount in one jsdom
 * document still sees it — and be wrong in a browser, where a reload
 * re-evaluates the module and the page would forget it had ever issued
 * anything. `sessionStorage` is the storage whose lifetime matches the claim
 * ("this tab issued a client here"), which is why the test's remount is a
 * proxy and the browser reload is the real check.
 *
 * Wrapped in try/catch: Safari in private mode throws on write, and failing to
 * remember is a worse first-run form, never a leaked secret.
 */
const markerKey = (ws: string, accountId: string) => `egret.dataapi.issued-client.${ws}.${accountId}`;

function readMarker(ws: string, accountId: string): string | null {
  try {
    return window.sessionStorage.getItem(markerKey(ws, accountId));
  } catch {
    return null;
  }
}

function writeMarker(ws: string, accountId: string, clientId: string) {
  try {
    window.sessionStorage.setItem(markerKey(ws, accountId), clientId);
  } catch {
    /* Storage unavailable (private mode, quota). See the comment above. */
  }
}

function clearMarker(ws: string, accountId: string) {
  try {
    window.sessionStorage.removeItem(markerKey(ws, accountId));
  } catch {
    /* As above. */
  }
}

/**
 * Issue an OAuth2 client credential to a service account, on a page with a URL
 * (`/explore/:ws/data-api/clients/:accountId/new-client`) rather than in a
 * dialog.
 *
 * This is the create page with a real design gain rather than just a URL.
 * Issuing returns a secret shown ONCE, and as dialogs that was a modal opening
 * on top of another modal — the previous phase's review found the issue dialog
 * being unmounted while still open as the secret dialog mounted, which is the
 * classic source of a stuck `pointer-events: none` on `document.body` that
 * jsdom cannot see.
 *
 * As a page the secret gets a better home: **the same route, in a created
 * state**. The form is replaced in place by the credential, one copy control
 * and a "Done" link. That cannot be dismissed by a click outside or by an
 * accidental Escape, and it does not need a second modal to appear on top of a
 * first.
 *
 * Three states, and the order of the checks below is the point:
 *
 * 1. **created** — this mount issued the client; the secret is in state.
 * 2. **spent** — a marker says this tab issued a client here but this mount does
 *    not hold the secret (a reload). It says so, and offers the way forward.
 * 3. **form** — nothing issued yet.
 *
 * **Issued is not granted, and both credential states say so.** A live walk of
 * the whole journey stopped here: the page hands over a credential, and the
 * credential cannot call anything — invoke is granted per ENDPOINT, to the
 * service ACCOUNT, on that endpoint's Access tab
 * (`/data-api/endpoints/:id/access`). Nothing between "Copy" and "Done" said
 * that, so every call the new client made was refused by a page that had just
 * told the reader it was finished. Both the created and the spent state now
 * carry the fact, because a reader who reloaded still holds an ungranted
 * client.
 *
 * **Only the spent state carries the LINK, and that asymmetry is the design.**
 * The created state's one invariant is that it has a single navigation
 * ("Done"), because leaving destroys a secret that cannot be shown again — a
 * link in the guidance would be exactly the second exit this file already
 * refuses in the header. It buys little, too: a grant is per endpoint and this
 * page cannot know which one, so the best any link could do is drop the reader
 * on the endpoints LIST to choose from, one hop closer at the cost of the
 * secret. Endpoints is a real sidebar item, so naming it in prose is a
 * destination a reader can reach. The spent state has no such cost — the secret
 * is already gone and the state already offers two ways out — so there the same
 * sentence ends in a link to the endpoints list.
 */
export default function ClientCreate() {
  const { workspace: ws, accountId } = useParams() as { workspace: string; accountId: string };
  const qc = useQueryClient();
  const issue = useDataApiIssueClient();

  // The account is read from the list the Clients page already loads (same
  // query key, so this is normally a cache hit): the URL carries an id, and an
  // id is not something a reader recognises — a credential is issued FOR
  // something, and the page has to name it.
  const { data: envelope } = useDataApiListServiceAccounts(ws);
  const accounts = envelope?.status === 200 ? envelope.data : [];
  const account = accounts.find((a) => a.id === accountId);

  const [issued, setIssued] = useState<DataApiNewClientDto | null>(null);
  const [spentClientId, setSpentClientId] = useState<string | null>(() => readMarker(ws, accountId));
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [error, setError] = useState<string | null>(null);

  const clientsHref = `/explore/${ws}/data-api/clients`;
  // Where a grant is given. Not a destination this page can be precise about
  // — see the "issued is not granted" note in the doc comment above.
  const endpointsHref = `/explore/${ws}/data-api/endpoints`;
  const canSubmit = name.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    setError(null);
    try {
      const res = await issue.mutateAsync({
        workspace: ws,
        id: accountId,
        data: {
          name: name.trim(),
          // Only sent when the author picked one — absent means "lives until
          // revoked", and an empty string is a value the backend would have to
          // interpret.
          ...(expiresAt.trim() !== "" ? { expires_at: new Date(expiresAt).toISOString() } : {}),
        },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — the check just keeps this page
      // honest against the response union the generated client declares.
      if (res.status !== 201) {
        setError("Could not issue client.");
        return;
      }
      // The row is in the table behind this page before the author walks back
      // to it. Per service account, so this is the only key that matters here.
      void qc.invalidateQueries({ queryKey: getDataApiListClientsQueryKey(ws, accountId) });
      // AFTER success, never before: marking the route spent on a failed issue
      // would lock the author out of the only page that can issue this account
      // a client, with no credential to show for it.
      writeMarker(ws, accountId, res.data.client_id);
      setSpentClientId(res.data.client_id);
      setIssued(res.data);
    } catch (e) {
      // NOTHING is cleared here. A dialog could be dismissed and reopened; a
      // page that resets itself on failure loses work the author cannot get
      // back — so the server's own message goes inline above the form the
      // author already filled in, and every field keeps its value.
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  // ── 1. Created: the secret, in place, exactly once ────────────────────────
  if (issued) {
    return (
      <ListPageShell
        width="narrow"
        title="Client created"
        description={`Copy the secret now — this is the only time it is shown.${
          account ? ` It belongs to ${account.name}.` : ""
        } If it is lost, revoke this client and issue another.`}
        // No back-arrow toolbar here: "Done" IS the way back, and a second exit
        // beside it invites leaving before the secret is copied. There is no
        // success toast either — the credential on screen is the confirmation.
        action={
          <Button asChild>
            <Link to={clientsHref}>Done</Link>
          </Button>
        }
      >
        <div className="space-y-4">
          {/* One copy affordance, for the secret. The id and the token endpoint
              are plain text so that "Copy" is unambiguous — for a reader
              scanning the page and for a test querying by accessible name. */}
          <div className="space-y-4 rounded-lg border p-4">
            <Field label="Client ID" testId="client-id" value={issued.client_id} />
            <div className="space-y-1.5">
              <Label>Client secret</Label>
              <div className="flex items-center gap-2">
                <code data-testid="client-secret" className="flex-1 truncate rounded bg-muted px-2 py-1.5 text-xs">
                  {issued.client_secret}
                </code>
                <CopyButton value={issued.client_secret} />
              </div>
            </div>
            <Field label="Token endpoint" testId="token-endpoint" value={issued.token_endpoint} />
          </div>

          {/* Issued is not granted. Prose, and deliberately NOT a link: see the
              doc comment above for why this state still has exactly one
              navigation. It sits BELOW the card so it is read after the three
              values, and it leads with the ordering ("Copy the secret first")
              because the fact it delivers is the one thing on this page that
              can wait — the secret is not. */}
          <p className="text-sm text-muted-foreground">
            <span className="font-medium text-foreground">This client cannot call anything yet.</span> Copy the secret
            first, then grant it an endpoint: invoke is granted per endpoint, to{" "}
            {account ? account.name : "the service account"}, on that endpoint&apos;s Access tab under Endpoints. Until
            one is granted, every call it makes is refused.
          </p>
        </div>
      </ListPageShell>
    );
  }

  // ── 2. Spent: this tab issued here, and the secret is gone for good ───────
  if (spentClientId) {
    return (
      <ListPageShell
        width="narrow"
        title="Client secret no longer available"
        description={`A client was already issued${account ? ` to ${account.name}` : ""} from this page. Its secret was shown once and cannot be shown again — reloading cannot bring it back.`}
        // Top-left, above the title — the `back` slot, not `toolbar`, which
        // `ListPageShell` renders into the same right-hand group as `action`.
        // See the form below for the measurement.
        back={
          <BackLink to={clientsHref}>Clients</BackLink>
        }
        // Explicit, and it mints nothing on its own: it clears the marker and
        // returns the empty form. A URL that cannot be left forward is worse
        // than the dialog this page replaced.
        action={
          <Button
            onClick={() => {
              clearMarker(ws, accountId);
              setSpentClientId(null);
              setError(null);
            }}
          >
            Issue another client
          </Button>
        }
      >
        <div className="space-y-4">
          <div className="space-y-4 rounded-lg border p-4">
            <Field label="Client ID" testId="client-id" value={spentClientId} />
            <p className="text-xs text-muted-foreground">
              The client itself is live and listed on the Clients page. If its secret was lost, revoke it there and
              issue another — there is no way to re-read a secret, by design.
            </p>
            {/* The same fact the created state carries — a reader who reloaded
                still holds an ungranted client — and here it gets the link the
                created state cannot have: there is no secret left on screen for
                a stray click to destroy, and this state already offers two ways
                out of its own. */}
            <p className="text-xs text-muted-foreground">
              Being issued is not being allowed: invoke is granted per endpoint, to{" "}
              {account ? account.name : "the service account"}, on that endpoint&apos;s Access tab. Until one is
              granted, every call it makes is refused —{" "}
              <Link className="font-medium text-foreground underline underline-offset-2" to={endpointsHref}>
                pick the endpoint from Endpoints
              </Link>
              .
            </p>
          </div>
        </div>
      </ListPageShell>
    );
  }

  // ── 3. The form ───────────────────────────────────────────────────────────
  return (
    <ListPageShell
      width="narrow"
      title={account ? `New client for ${account.name}` : "New client"}
      description="Issues a fresh OAuth2 client_credentials pair. The secret appears on this page the moment it is created, and is never shown again."
      back={
        /* Top-left, above the title, the slot `EndpointCreate` moved to. It was
           in `toolbar`, which `ListPageShell` renders into the SAME right-hand
           group as `action`: seen in the browser at 1440px, `← Clients` sat
           directly beside the "Create client" primary, so the way out and the
           commit were adjacent — while the sibling `New endpoint` page put the
           way out in the opposite corner. That inconsistency is the reason the
           `back` slot exists; this page and `ServiceAccountCreate` were simply
           not converted with it. */
        <BackLink to={clientsHref}>Clients</BackLink>
      }
      action={
        <Button onClick={() => void submit()} disabled={!canSubmit || issue.isPending}>
          {issue.isPending ? "Creating…" : "Create client"}
        </Button>
      }
    >
      <div className="space-y-4">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {/* Exactly what the dialog it replaces collected: the name, and the one
            field `IssueClientInput` allows to be set at mint time and never
            again. A grant is not one of them, and the reason is not that it
            lives elsewhere ON THIS PAGE'S SURFACE — an earlier version of this
            comment said "already have their own affordance on the Clients
            page", which is the same false claim `ServiceAccountCreate`'s
            subtitle shipped. There is no grant on the Clients page. A grant is
            a fact about a (service account, endpoint) pair and is given on the
            ENDPOINT's Access tab, per endpoint; this form holds neither an
            endpoint nor a way to pick one. */}
        <div className="space-y-4 rounded-lg border p-4">
          <div className="space-y-1.5">
            <Label htmlFor="new-client-name">Name</Label>
            <Input
              id="new-client-name"
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="nightly-etl"
            />
            <p className="text-xs text-muted-foreground">
              How this credential is identified on the account&apos;s row — name it after the consumer that will use
              it, so revoking the right one later is obvious.
            </p>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-client-expiry">Expires (optional)</Label>
            <Input
              id="new-client-expiry"
              type="date"
              value={expiresAt}
              onChange={(e) => setExpiresAt(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">Leave blank for a client that lives until revoked.</p>
          </div>
        </div>
      </div>
    </ListPageShell>
  );
}

/** A read-only credential field: a label over one line of monospace. */
function Field({ label, value, testId }: { label: string; value: string; testId: string }) {
  return (
    <div className="space-y-1.5">
      <Label>{label}</Label>
      <code data-testid={testId} className="block truncate rounded bg-muted px-2 py-1.5 text-xs">
        {value}
      </code>
    </div>
  );
}

/** Same "Copy"/"Copied" idiom as `TryIt.tsx`'s `CopyButton`;
 *  moved here verbatim from the secret dialog this page replaces. */
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
