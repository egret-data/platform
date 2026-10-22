import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { ListPageShell } from "@/components/explore/ListPageShell";
// The mutation comes from the hand-written wrapper, never the generated hook:
// the wrapper is what invalidates the service-account list, and that key is
// shared by five surfaces (the Clients table, Try-it's "Run as" picker, the
// Access checklist, the OpenAPI download dialog and the Response preview) —
// wired straight to the generated hook, a new account is missing from all five
// for `staleTime`'s thirty seconds.
import { useCreateServiceAccount } from "@/api/dataApi";
import { BackLink } from "@/components/layout/DetailHeader";

/**
 * Create a service account on a page with a URL
 * (`/explore/:ws/data-api/clients/new`) rather than in a modal.
 *
 * It collects exactly what the dialog it replaces collected — name, and an
 * optional description — because that is what a row cannot exist without. The
 * page has room for more; more is not owed. Clients, grants and expiry are all
 * things an account acquires AFTER it exists, each with its own affordance
 * ELSEWHERE, and hoisting any of them here would rebuild those flows in a
 * worse place.
 *
 * Two affordances, and they are on two different screens — the sentence that
 * put both "on the Clients row" is what the subtitle below was copied from,
 * and it is why the subtitle shipped false:
 *
 * - **Issue a client** — the Clients page, in the account's group-header
 *   actions menu (`ServiceAccountActionsMenu`).
 * - **Grant invoke** — an ENDPOINT's Access tab
 *   (`/data-api/endpoints/:id/access`, `detail/AccessSection.tsx`, the only
 *   caller of `useDataApiGrantEndpointInvoke`).
 *
 * **A grant is a fact about a (SERVICE ACCOUNT, endpoint) pair — not about a
 * client.** The mutation body is `{ service_account_id }` (`AccessSection.tsx`
 * `onGrant`, pinned at `__tests__/AccessSection.test.tsx:346`), `/grants`
 * keys on the account, and `ClientCreate` says the same thing to the reader:
 * "invoke is granted per endpoint, to <account>". An earlier revision of this
 * comment said "(client, endpoint)", and getting that pair wrong does not just
 * misdescribe the model — it INVERTS the conclusion below it. If grants were
 * about clients, the Clients page would be their natural home and the argument
 * for keeping them off it would collapse.
 *
 * The reason a grant cannot live here is not that this surface holds the wrong
 * object; it holds exactly the right one. It is that this surface holds no
 * ENDPOINT, and the other half of the pair is not optional. A control here
 * would have to make the reader pick an endpoint first — and then answer, per
 * pair, whether that account can already invoke, which `/grants` cannot tell
 * it: `list_endpoint_grants` filters `scope_type = 'object'`
 * (`crates/egret-dataapi/src/routes.rs`), so a principal reaching invoke
 * through the Cedar workspace→object cascade is invisible to it. That is why
 * `AccessSection` probes `can_invoke` per pair and renders a four-valued
 * `InvokeStatus` rather than a checkbox.
 */
export default function ServiceAccountCreate() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const create = useCreateServiceAccount();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);

  const canSubmit = name.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    setError(null);
    try {
      const res = await create.mutateAsync({
        workspace: ws,
        data: {
          name: name.trim(),
          // Optional, so it is only sent when the author wrote one — an empty
          // string is a value, and would overwrite nothing with something.
          ...(description.trim() !== "" ? { description: description.trim() } : {}),
        },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — the check just keeps this page
      // honest against the response union the generated client declares.
      if (res.status !== 201) {
        setError("Could not create service account.");
        return;
      }
      // Back to the LIST, where `EndpointCreate` hands off into the created
      // object's own editor. That difference is not an inconsistency between
      // the two create pages: an endpoint has a detail route to open, and a
      // service account does not — it is a row in a list with an actions menu,
      // so the list IS its detail view. The row is already there because the
      // wrapper above invalidated the query before this line runs.
      //
      // No success toast, deliberately. The hand-off is the confirmation —
      // the author lands on the list holding the account they just named — and
      // a toast on top of that reports what is already on screen. Same as
      // `EndpointCreate` and `EngineCreate`: invalidate, navigate, no toast.
      nav(`/explore/${ws}/data-api/clients`);
    } catch (e) {
      // NOTHING is cleared here. A dialog could be dismissed and reopened; a
      // page that resets itself on failure loses work the author cannot get
      // back — so the server's own message goes inline above the form the
      // author already filled in, and every field keeps its value.
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <ListPageShell
      width="narrow"
      title="New service account"
      /* Every clause of this names a place, and each place has to exist.
         "grant it endpoints from the Clients list" did not: the Clients list
         has no grant affordance on either of its menus, and a reader who
         followed it went hunting for a control that is not there. Invoke is
         granted per ENDPOINT, on that endpoint's Access tab — the same words
         `ClientCreate` uses one screen downstream, deliberately, so the one
         fact has one phrasing and a future drift is greppable.

         Prose, no link, and that is the same asymmetry `ClientCreate` records:
         a link here is a third navigation off a form with unsaved input and no
         draft persistence (§3.8), so it would silently discard what the author
         typed. `Endpoints` is a real sidebar item, so naming it is a
         destination a reader can reach. */
      description="A machine identity for the Data API. Once it exists, issue it OAuth2 clients from the Clients list — then grant it invoke per endpoint, on that endpoint's Access tab under Endpoints."
      back={
        /* Top-left, above the title, the same slot `EndpointCreate` moved to
           and the same shape every detail screen uses. It was still in
           `toolbar`, which `ListPageShell` renders into the SAME right-hand
           group as `action` — so on this page "leave without creating
           anything" sat one control away from "create it" while its sibling
           create page put it in the opposite corner. Seen side by side in the
           browser, the two pages disagreed about where the way out is. */
        <BackLink to={`/explore/${ws}/data-api/clients`}>Clients</BackLink>
      }
      action={
        <Button onClick={() => void submit()} disabled={!canSubmit || create.isPending}>
          {create.isPending ? "Creating…" : "Create service account"}
        </Button>
      }
    >
      <div className="space-y-4">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <div className="space-y-4 rounded-lg border p-4">
          <div className="space-y-1.5">
            <Label htmlFor="new-sa-name">Name</Label>
            <Input
              id="new-sa-name"
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="reporting-bot"
            />
            <p className="text-xs text-muted-foreground">
              The account&apos;s email address is derived from this by the backend, and is shown on its row.
            </p>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-sa-description">Description (optional)</Label>
            {/* The page has room the dialog did not: 4 rows, not 2. */}
            <Textarea
              id="new-sa-description"
              rows={4}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What this account is for, and who owns it"
            />
          </div>
        </div>
      </div>
    </ListPageShell>
  );
}
