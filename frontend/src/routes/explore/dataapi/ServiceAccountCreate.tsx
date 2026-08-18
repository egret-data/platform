import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";

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

/**
 * Create a service account on a page with a URL
 * (`/explore/:ws/data-api/clients/new`) rather than in a modal.
 *
 * It collects exactly what the dialog it replaces collected — name, and an
 * optional description — because that is what a row cannot exist without. The
 * page has room for more; more is not owed. Clients, grants and expiry are all
 * things an account acquires AFTER it exists, each with its own affordance on
 * the Clients row (the actions menu issues clients; Access grants endpoints),
 * and hoisting any of them here would rebuild those flows in a worse place.
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
      title="New service account"
      description="A machine identity for the Data API. Once it exists, issue it OAuth2 clients and grant it endpoints from the Clients list."
      toolbar={
        <Button asChild variant="ghost">
          <Link to={`/explore/${ws}/data-api/clients`}>
            <ArrowLeft className="h-4 w-4" />
            Clients
          </Link>
        </Button>
      }
      action={
        <Button onClick={() => void submit()} disabled={!canSubmit || create.isPending}>
          {create.isPending ? "Creating…" : "Create service account"}
        </Button>
      }
    >
      <div className="max-w-3xl space-y-4">
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
