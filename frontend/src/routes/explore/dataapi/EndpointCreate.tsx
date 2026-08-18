import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { useDatabases } from "@/api/databases";
// The mutation comes from the hand-written wrapper, never the generated hook:
// the wrapper is what invalidates the endpoint list, so the row this page
// creates is already in the list behind it when the user navigates back.
import { useCreateEndpoint } from "@/api/dataApi";

/**
 * Create an endpoint on a page with a URL (`/explore/:ws/data-api/endpoints/new`)
 * rather than in a modal.
 *
 * This is a DEDICATED page, not the detail route rendered without an id — the
 * precedent that fits is `engines/new` → `EngineCreate`, not `charts/new`. Six
 * of `EndpointDetail`'s seven sections read the endpoint id (Preview POSTs to
 * `…/endpoints/{id}/preview`; Response, Access, Try it, Versions and Config all
 * load the saved row), so reusing that shell would ship a page whose Preview
 * cannot run and whose right-hand tabs are meaningless until the first save.
 *
 * It collects exactly what a row cannot exist without — name, path, method,
 * database, SQL — plus the optional description the dialog had no room for.
 * Cache, limits, rate limit and params are deliberately ABSENT: they belong to
 * the editor the author lands on a moment later, and putting them here would
 * rebuild that editor in a worse place.
 */
export default function EndpointCreate() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const { data: databases } = useDatabases(ws);
  const create = useCreateEndpoint();

  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [description, setDescription] = useState("");
  const [method, setMethod] = useState("GET");
  const [databaseId, setDatabaseId] = useState("");
  const [sql, setSql] = useState("");
  const [error, setError] = useState<string | null>(null);

  const canSubmit = name.trim() !== "" && path.trim() !== "" && databaseId !== "" && sql.trim() !== "";

  async function submit() {
    if (!canSubmit) return;
    setError(null);
    try {
      const res = await create.mutateAsync({
        workspace: ws,
        data: {
          name: name.trim(),
          path: path.trim(),
          method,
          database_id: databaseId,
          sql: sql.trim(),
          // Optional, so it is only sent when the author wrote one — an empty
          // string is a value, and would overwrite nothing with something.
          ...(description.trim() !== "" ? { description: description.trim() } : {}),
        },
      });
      // The mutator throws on any non-2xx (see mutator.ts), so this branch is
      // the only one ever reached at runtime — the check just gives TS a
      // narrower type than the response union declares.
      if (res.status !== 201) {
        setError("Could not create endpoint.");
        return;
      }
      nav(`/explore/${ws}/data-api/endpoints/${res.data.id}`);
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
      title="New endpoint"
      description="Publish a query as a REST endpoint. Everything else — parameters, output shape, cache, limits — is configured on the endpoint itself once it exists."
      toolbar={
        <Button asChild variant="ghost">
          <Link to={`/explore/${ws}/data-api/endpoints`}>
            <ArrowLeft className="h-4 w-4" />
            Endpoints
          </Link>
        </Button>
      }
      action={
        <Button onClick={() => void submit()} disabled={!canSubmit || create.isPending}>
          {create.isPending ? "Creating…" : "Create endpoint"}
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
            <Label htmlFor="new-ep-name">Name</Label>
            <Input id="new-ep-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Daily report" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-ep-path">Path</Label>
            <Input id="new-ep-path" value={path} onChange={(e) => setPath(e.target.value)} placeholder="reports/daily" />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-ep-description">Description (optional)</Label>
            <Input
              id="new-ep-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="What this endpoint returns, and who it is for"
            />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="new-ep-method">Method</Label>
              <Select value={method} onValueChange={setMethod}>
                <SelectTrigger id="new-ep-method" aria-label="Method"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="GET">GET</SelectItem>
                  <SelectItem value="POST">POST</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-ep-db">Database</Label>
              <Select value={databaseId} onValueChange={setDatabaseId}>
                <SelectTrigger id="new-ep-db" aria-label="Database">
                  <SelectValue placeholder="Choose a database" />
                </SelectTrigger>
                <SelectContent>
                  {(databases ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>{d.name}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-ep-sql">SQL</Label>
            {/* The page has room the dialog did not: 12 rows, not 4. */}
            <Textarea id="new-ep-sql" className="font-mono text-xs" rows={12} value={sql} onChange={(e) => setSql(e.target.value)} placeholder="SELECT …" />
          </div>
        </div>
      </div>
    </ListPageShell>
  );
}
