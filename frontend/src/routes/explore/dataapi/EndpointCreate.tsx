import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { isFullConnection, useDatabases } from "@/api/databases";
// The mutation comes from the hand-written wrapper, never the generated hook:
// the wrapper is what invalidates the endpoint list, so the row this page
// creates is already in the list behind it when the user navigates back.
import { useCreateEndpoint } from "@/api/dataApi";
import { GOVERNED_BLOCKED_MESSAGE, governedWithoutPerPrincipal } from "./governance";
import { BackLink } from "@/components/layout/DetailHeader";

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
 * database, one statement's SQL — plus the optional description the dialog had
 * no room for.
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
          // A one-element list named `main` — the shape migration 0129 gave
          // every endpoint that pre-dated the statement list, so a brand-new
          // endpoint and a migrated one are indistinguishable from here on.
          //
          // The page stays single-SQL deliberately. Naming, reordering and
          // adding statements is the editor's job, exactly like params, cache
          // and limits: this page collects only what a row cannot exist
          // without, and a list UI here would rebuild that editor in a worse
          // place. `main` is never shown to the author for the same reason it
          // is not shown in the editor — a one-statement endpoint answers with
          // an ARRAY, so the name is not a key any caller reads.
          statements: [{ name: "main", sql: sql.trim() }],
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
      // No success toast, deliberately. The dialog needed one because it
      // closed over a list that looked unchanged; this page hands the author
      // straight to the new endpoint's own editor, with the name they typed in
      // the header — the hand-off IS the confirmation, and a toast on top of it
      // would report what is already on screen. `EngineCreate`, the precedent
      // for this page, does the same: invalidate, navigate, no toast.
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
      width="narrow"
      title="New endpoint"
      description="Publish a query as a REST endpoint. Everything else — parameters, output shape, cache, limits — is configured on the endpoint itself once it exists."
      back={
        /* Top-left, above the title, exactly as every endpoint detail screen
           renders it (`EndpointDetail.tsx`). It used to sit in `toolbar`, which
           `ListPageShell` renders into the same right-hand group as `action` —
           so "leave without creating anything" was one control away from
           "create it", the two outcomes furthest apart on the page. */
        <BackLink to={`/explore/${ws}/data-api/endpoints`}>Endpoints</BackLink>
      }
      action={
        <Button onClick={() => void submit()} disabled={!canSubmit || create.isPending}>
          {create.isPending ? "Creating…" : "Create endpoint"}
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
                  {/* The option is still OFFERED when it is governed and the
                      connection is not per-user. Omitting it would be the
                      obvious move and it is the wrong one: the same database is
                      publishable the moment an admin turns delegation on, so a
                      picker that hides it teaches the author the connection does
                      not exist rather than that it needs one setting. The
                      warning below says which. */}
                  {(databases ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>
                      {d.name}
                      {isFullConnection(d) && d.governed ? (
                        <span className="ml-1.5 text-xs text-muted-foreground">Governed</span>
                      ) : null}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          {/* Shown here, at the picker, rather than only at Publish. A refusal
              an author meets after writing the SQL is a refusal that cost them
              the work; this one is visible the moment the choice is made, and
              it names the setting and the person who can change it. */}
          {governedWithoutPerPrincipal(
            (databases ?? []).find((d) => d.id === databaseId),
          ) ? (
            <Alert>
              <AlertDescription>{GOVERNED_BLOCKED_MESSAGE}</AlertDescription>
            </Alert>
          ) : null}
          <div className="space-y-1.5">
            {/* A `<p>`, not a `<Label>`: Monaco is not a form control, so
                `htmlFor` would point at nothing. The editor names itself
                through `ariaLabel` below, which is where `Query`'s SQL editor
                gets its accessible name too. */}
            <p className="text-sm font-medium">SQL</p>
            {/* The same editor this SQL becomes the moment the endpoint exists
                (`detail/Query.tsx`). It was a bare `<Textarea>` here — the
                weakest tool at the moment of most need, since the first
                statement is the one written from nothing and every statement
                after it gets syntax colouring, bracket matching and
                completion.

                Fixed height, like the response transform's editor and unlike
                `Query`'s: this page is an ordinary scrolling form, not a
                bounded split pane, so an editor that grew with its content
                would push the Create button down the page as the author types.

                **No `onRun`, deliberately.** Preview is
                `POST …/endpoints/{id}/preview` and there is no id yet, so there
                is nothing ⌘↵ could run. `SqlEditor` registers that command
                unconditionally, so the guard is that the callback is absent
                rather than that the command is.

                **Schema-aware completion is deliberately absent**, exactly as
                in `Query`: SQL Lab's `words` come from its schema explorer's
                own state (a `SchemaSink` the `DataPanel` writes into), there is
                no reusable hook to call and no explorer on this page, so `[]`
                is passed and Monaco offers its built-in SQL keywords. Wiring a
                catalog fetch here is its own task. */}
            <div className="h-64 overflow-hidden rounded-md border border-border">
              <SqlEditor value={sql} onChange={setSql} schemaWords={[]} ariaLabel="SQL" />
            </div>
          </div>
        </div>
      </div>
    </ListPageShell>
  );
}
