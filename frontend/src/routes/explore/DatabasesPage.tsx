import { useState } from "react";
import { useParams } from "react-router-dom";
import { toast } from "sonner";
import {
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Loader2,
  Lock,
  Pencil,
  Plus,
  Trash2,
  XCircle,
  Zap,
} from "lucide-react";

import {
  type DatabaseConn,
  type DatabaseConnInput,
  type Engine,
  isFullConnection,
  useCreateDatabase,
  useDatabases,
  useDeleteDatabase,
  useTestDatabase,
  useUpdateDatabase,
  useWsEngineView,
} from "@/api/databases";
import { ApiError } from "@/api/client";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SecretPicker, type SecretSelection } from "@/components/SecretPicker";
import { Switch } from "@/components/ui/switch";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { StatusBadge, type StatusTone } from "@/components/ui/status-badge";
import { Forbidden } from "@/routes/Forbidden";
import { ScrollX } from "@/components/ui/scroll-cue";

function statusTone(status: string): StatusTone {
  if (status === "ok") return "success";
  if (status === "error") return "destructive";
  return "neutral";
}

/** Turn the raw status enum into a human-readable label (tone mapping unchanged). */
function humanizeStatus(status: string): string {
  if (status === "ok") return "Healthy";
  if (status === "error") return "Error";
  return status.charAt(0).toUpperCase() + status.slice(1);
}

/** The shared refusal (KI-231): a 403, or the redacted listing a member who
 *  may only query gets, both mean this section is not theirs to manage. */
function AdminOnlyNotice() {
  return <Forbidden level="section" reason="Only a workspace admin can manage connections." />;
}

export default function DatabasesPage() {
  const ws = useParams().workspace!;
  const dbs = useDatabases(ws);
  const deleteMutation = useDeleteDatabase(ws);
  const testMutation = useTestDatabase(ws);

  const [addOpen, setAddOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<DatabaseConn | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DatabaseConn | null>(null);

  // Inline per-row test outcome keyed by connection id.
  const [testResult, setTestResult] = useState<
    Record<string, { ok: boolean; message?: string }>
  >({});
  const [testingId, setTestingId] = useState<string | null>(null);

  async function handleTest(conn: DatabaseConn) {
    setTestingId(conn.id);
    try {
      const outcome = await testMutation.mutateAsync(conn.id);
      // A failed test says why in `detail` (KI-001: e.g. the Trino setting to change).
      setTestResult((prev) => ({ ...prev, [conn.id]: { ok: outcome.ok, message: outcome.ok ? undefined : outcome.detail } }));
    } catch (err) {
      setTestResult((prev) => ({
        ...prev,
        [conn.id]: { ok: false, message: (err as ApiError).message },
      }));
    } finally {
      setTestingId(null);
    }
  }

  if (dbs.isLoading) {
    return (
      <section>
        <SectionHeader onAdd={() => setAddOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={3} cols={6} />
        </div>
      </section>
    );
  }

  if (dbs.isError) {
    const status = (dbs.error as ApiError | null)?.status;
    if (status === 403) return <AdminOnlyNotice />;
    return (
      <Alert variant="destructive">
        <AlertTitle>Failed to load connections</AlertTitle>
        <AlertDescription>Something went wrong. Please try again.</AlertDescription>
      </Alert>
    );
  }

  const listed = dbs.data ?? [];
  // A member who may run queries or read datasets gets the redacted listing
  // (no host, options or secret). This page manages connections, so it stays
  // admin-only rather than rendering rows it cannot fill.
  if (!listed.every(isFullConnection)) return <AdminOnlyNotice />;
  const rows = listed.filter(isFullConnection);

  const columns: Column<DatabaseConn>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      value: (conn) => conn.name,
      cell: (conn) => <span className="font-medium">{conn.name}</span>,
    },
    {
      key: "engine",
      label: "Engine",
      sortable: true,
      value: (conn) => conn.engine,
      cell: (conn) => (
        <span className="flex items-center gap-1.5">
          <Badge variant="secondary">{conn.engine}</Badge>
          {conn.delegated ? (
            <Badge
              className="border-transparent bg-highlight text-highlight-foreground"
              title="Queries run as the signed-in user (per-user delegation)"
            >
              Per-user
            </Badge>
          ) : null}
          {conn.governed ? (
            <Badge
              variant="outline"
              title="Row filters and column masks are applied per user at the engine"
            >
              Governed
            </Badge>
          ) : null}
        </span>
      ),
    },
    {
      key: "host",
      label: "Host:Port",
      sortable: true,
      value: (conn) => `${conn.host}:${conn.port}`,
      cell: (conn) => (
        <span className="font-mono text-muted-foreground">
          {conn.host}:{conn.port}
        </span>
      ),
    },
    {
      key: "status",
      label: "Status",
      sortable: true,
      value: (conn) => conn.status,
      cell: (conn) => (
        <StatusBadge tone={statusTone(conn.status)}>{humanizeStatus(conn.status)}</StatusBadge>
      ),
    },
    {
      key: "secret",
      label: "Secret",
      cell: (conn) => (
        <Badge variant={conn.has_secret ? "secondary" : "outline"}>
          {conn.has_secret ? "Configured" : "Not set"}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "Actions",
      className: "text-right",
      cell: (conn) => {
        const result = testResult[conn.id];
        return (
          <div className="flex items-center justify-end gap-1">
            {result && (
              <span
                // A failure's detail can be a sentence (KI-001): it wraps in a
                // bounded column instead of pushing the row's buttons off-screen.
                className={`mr-1 inline-flex max-w-xs items-start gap-1 whitespace-normal text-left text-xs ${
                  result.ok ? "text-success" : "text-destructive"
                }`}
                role="status"
              >
                {result.ok ? (
                  <CheckCircle2 className="mt-px h-3.5 w-3.5 shrink-0" />
                ) : (
                  <XCircle className="mt-px h-3.5 w-3.5 shrink-0" />
                )}
                {result.ok ? "OK" : (result.message ?? "Failed")}
              </span>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => handleTest(conn)}
              disabled={testingId === conn.id}
              aria-label={`Test connection for ${conn.name}`}
            >
              {testingId === conn.id ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Zap className="h-4 w-4" />
              )}
              Test
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              onClick={() => setEditTarget(conn)}
              aria-label={`Edit ${conn.name}`}
            >
              <Pencil className="h-4 w-4" />
              Edit
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-destructive"
              onClick={() => setDeleteTarget(conn)}
              aria-label={`Delete ${conn.name}`}
            >
              <Trash2 className="h-4 w-4" />
              Delete
            </Button>
          </div>
        );
      },
    },
  ];

  // Provisioned rows (materialized by an engine→workspace assignment) are
  // read-only — no Add/Edit/Delete/Test. Self-service rows keep the full CRUD.
  const provisionedRows = rows.filter((c) => c.engine_id);
  const selfServiceRows = rows.filter((c) => !c.engine_id);

  const okCount = selfServiceRows.filter((c) => c.status === "ok").length;
  const errorCount = selfServiceRows.filter((c) => c.status === "error").length;

  return (
    <section>
      <SectionHeader onAdd={() => setAddOpen(true)} />

      <div className="mt-4 space-y-8">
        {provisionedRows.length > 0 && (
          <div className="space-y-3">
            <div>
              <h3 className="text-sm font-semibold">Provisioned by egret</h3>
              <p className="text-sm text-muted-foreground">
                Read-only engines assigned to this workspace by a platform admin.
              </p>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              {provisionedRows.map((conn) => (
                <ProvisionedEngineCard key={conn.id} ws={ws} conn={conn} />
              ))}
            </div>
          </div>
        )}

        {selfServiceRows.length > 0 ? (
          <div className="space-y-3">
            {provisionedRows.length > 0 && (
              <div>
                <h3 className="text-sm font-semibold">Your connections</h3>
                <p className="text-sm text-muted-foreground">
                  Self-service connections managed by this workspace.
                </p>
              </div>
            )}
            <SummaryStrip
              items={[
                { label: "connections", singular: "connection", value: selfServiceRows.length },
                { label: "ok", value: okCount },
                { label: "error", value: errorCount },
              ]}
            />
            <FilterableTable
              rows={selfServiceRows}
              columns={columns}
              getId={(conn) => conn.id}
              searchPlaceholder="Search connections…"
              initialSort={{ key: "name", dir: "asc" }}
            />
          </div>
        ) : provisionedRows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No connections yet.</p>
          </div>
        ) : null}
      </div>

      {/* Add dialog. A new connection is tested at once, so its status is known (KI-011). */}
      <DatabaseConnDialog
        ws={ws}
        open={addOpen}
        onOpenChange={setAddOpen}
        editConn={null}
        onCreated={(conn) => void handleTest(conn)}
      />

      {/* Edit dialog — key forces remount when target changes so useState initializers re-run */}
      <DatabaseConnDialog
        key={editTarget?.id ?? "__edit-closed"}
        ws={ws}
        open={editTarget !== null}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
        editConn={editTarget}
      />

      {/* Delete confirm */}
      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete connection?"}
        description="This connection will be removed. Any SQL Lab tabs or charts that reference it may break."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget)
            deleteMutation.mutate(deleteTarget.id, {
              onSuccess: () => toast.success("Connection deleted"),
              onError: (e) => toast.error(String((e as Error)?.message ?? e)),
            });
        }}
      />
    </section>
  );
}

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Connections</h2>
        <p className="text-sm text-muted-foreground">
          Connections to query engines that this workspace can use in SQL Lab and charts.
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        Add connection
      </Button>
    </div>
  );
}

/**
 * Read-only card for an egret-provisioned engine (materialized via an
 * engine→workspace assignment). No Add/Edit/Delete/Test affordances. Expands to
 * show the engine's catalogs, each resolved to the storage/secret name it maps
 * to (from the ws-member engine view, `GET /ws/{ws}/engines/{engine_id}`).
 */
function ProvisionedEngineCard({ ws, conn }: { ws: string; conn: DatabaseConn }) {
  const [expanded, setExpanded] = useState(false);
  const view = useWsEngineView(ws, conn.engine_id!);
  const catalogs = view.data?.catalogs ?? [];

  return (
    <div className="rounded-lg border p-4">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate font-medium">{conn.name}</span>
            <Badge variant="secondary">{conn.engine}</Badge>
            {conn.delegated ? (
              <Badge
                className="border-transparent bg-highlight text-highlight-foreground"
                title="Queries run as the signed-in user (per-user delegation)"
              >
                Per-user
              </Badge>
            ) : null}
            {conn.governed ? (
              <Badge
                variant="outline"
                title="Row filters and column masks are applied per user at the engine"
              >
                Governed
              </Badge>
            ) : null}
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            <StatusBadge tone={statusTone(conn.status)}>{humanizeStatus(conn.status)}</StatusBadge>
            <Badge
              variant="outline"
              className="gap-1 text-muted-foreground"
              title="Provisioned by egret — read-only"
            >
              <Lock className="h-3 w-3" />
              Provisioned by egret · read-only
            </Badge>
          </div>
        </div>
      </div>

      <button
        type="button"
        className="mt-3 inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        onClick={() => setExpanded((e) => !e)}
        aria-expanded={expanded}
        aria-label={`${expanded ? "Hide" : "Show"} catalogs for ${conn.name}`}
      >
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5" />
        )}
        {expanded ? "Hide catalogs" : "Show catalogs"}
      </button>

      {expanded && (
        <div className="mt-2 rounded border">
          {view.isLoading ? (
            <div className="flex items-center gap-2 p-3 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              Loading catalogs…
            </div>
          ) : catalogs.length === 0 ? (
            <p className="p-3 text-xs text-muted-foreground">No catalogs.</p>
          ) : (
            <ScrollX>
              <table className="w-full text-left text-xs">
                <thead className="text-muted-foreground">
                  <tr className="border-b">
                    <th className="px-3 py-1.5 font-medium">Catalog</th>
                    <th className="px-3 py-1.5 font-medium">Kind</th>
                    <th className="px-3 py-1.5 font-medium">Storage</th>
                    <th className="px-3 py-1.5 font-medium">Secret</th>
                  </tr>
                </thead>
                <tbody>
                  {catalogs.map((c) => (
                    <tr key={c.name} className="border-b last:border-b-0">
                      <td className="px-3 py-1.5 font-medium">{c.name}</td>
                      <td className="px-3 py-1.5">
                        <Badge variant="secondary">{c.kind}</Badge>
                      </td>
                      <td className="whitespace-nowrap px-3 py-1.5 font-mono text-muted-foreground">
                        {c.storage_name ?? "—"}
                      </td>
                      <td className="whitespace-nowrap px-3 py-1.5 font-mono text-muted-foreground">
                        {c.secret_name ?? "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </ScrollX>
          )}
        </div>
      )}
    </div>
  );
}

function DatabaseConnDialog({
  ws,
  open,
  onOpenChange,
  editConn,
  onCreated,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editConn: DatabaseConn | null;
  /** Called with a connection this dialog has just created. */
  onCreated?: (conn: DatabaseConn) => void;
}) {
  const isEdit = editConn !== null;
  const createMutation = useCreateDatabase(ws);
  const updateMutation = useUpdateDatabase(ws);

  const [name, setName] = useState(editConn?.name ?? "");
  const [engine, setEngine] = useState<Engine>(editConn?.engine ?? "trino");
  const [host, setHost] = useState(editConn?.host ?? "");
  const [port, setPort] = useState(editConn?.port?.toString() ?? "");
  const [user, setUser] = useState(editConn?.options.user ?? "");
  const [catalog, setCatalog] = useState(editConn?.options.catalog ?? "");
  const [schema, setSchema] = useState(editConn?.options.database ?? "");
  const [httpScheme, setHttpScheme] = useState(editConn?.options.http_scheme ?? "");
  const [impersonate, setImpersonate] = useState(editConn?.options.impersonate ?? false);
  const [delegated, setDelegated] = useState(editConn?.delegated ?? false);
  // Held in form state even though nothing on this page reads it back, because
  // `PUT …/databases/{id}` replaces the whole row: a form that did not carry
  // `governed` would send the serde default (`false`) and CLEAR the flag every
  // time someone edited a port. That is the quiet direction — the connection
  // keeps working, and the endpoints on it silently stop being gated.
  const [governed, setGoverned] = useState(editConn?.governed ?? false);
  const [secretSel, setSecretSel] = useState<SecretSelection>({});
  const [error, setError] = useState<string | null>(null);

  function resetToConn(conn: DatabaseConn | null) {
    setName(conn?.name ?? "");
    setEngine(conn?.engine ?? "trino");
    setHost(conn?.host ?? "");
    setPort(conn?.port?.toString() ?? "");
    setUser(conn?.options.user ?? "");
    setCatalog(conn?.options.catalog ?? "");
    setSchema(conn?.options.database ?? "");
    setHttpScheme(conn?.options.http_scheme ?? "");
    setImpersonate(conn?.options.impersonate ?? false);
    setDelegated(conn?.delegated ?? false);
    setGoverned(conn?.governed ?? false);
    setSecretSel({});
    setError(null);
    createMutation.reset();
    updateMutation.reset();
  }

  // The form is dirty when any field diverges from its initial (edit-target or
  // add-blank) value — including having staged a secret change.
  const dirty =
    name !== (editConn?.name ?? "") ||
    engine !== (editConn?.engine ?? "trino") ||
    host !== (editConn?.host ?? "") ||
    port !== (editConn?.port?.toString() ?? "") ||
    user !== (editConn?.options.user ?? "") ||
    catalog !== (editConn?.options.catalog ?? "") ||
    schema !== (editConn?.options.database ?? "") ||
    httpScheme !== (editConn?.options.http_scheme ?? "") ||
    impersonate !== (editConn?.options.impersonate ?? false) ||
    delegated !== (editConn?.delegated ?? false) ||
    governed !== (editConn?.governed ?? false) ||
    Object.keys(secretSel).length > 0;

  // beforeunload guard (refresh / tab-close) while there are unsaved edits.
  useUnsavedChangesWarning(open && dirty);

  /** Reset + close without prompting — used after a successful save. */
  function closeAndReset() {
    resetToConn(null);
    onOpenChange(false);
  }

  function handleOpenChange(next: boolean) {
    if (!next) {
      // Guard in-app closes (Cancel / Esc / overlay) against silent data loss.
      if (dirty && !window.confirm("Discard unsaved changes?")) return;
      closeAndReset();
      return;
    }
    onOpenChange(next);
  }

  const isPending = createMutation.isPending || updateMutation.isPending;

  // Required fields (mirror the native `required` on name/host/port). Submit is
  // gated on these being filled, with a muted hint naming what's still missing.
  const missing: string[] = [];
  if (!name.trim()) missing.push("name");
  if (!host.trim()) missing.push("host");
  if (!port.trim()) missing.push("port");
  const missingHint = missing.length > 0 ? `Fill in ${missing.join(", ")}` : null;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const options = {
      user: user.trim().length > 0 ? user.trim() : undefined,
      http_scheme: httpScheme.trim().length > 0 ? httpScheme.trim() : undefined,
      catalog: catalog.trim().length > 0 ? catalog.trim() : undefined,
      database: schema.trim().length > 0 ? schema.trim() : undefined,
      impersonate:
        (engine === "trino" || engine === "starrocks") && impersonate
          ? true
          : undefined,
    };

    const input: DatabaseConnInput = {
      name: name.trim(),
      engine,
      host: host.trim(),
      port: Number(port),
      options,
      // B3a: run BI queries as the caller's per-user engine JWT (delegation).
      delegated,
      // Package 1: the data behind this connection is filtered per user at the
      // engine, so a Data API endpoint may only be run or published onto it when
      // the query runs as the caller.
      //
      // Sent EXPLICITLY, always. The field is optional on the wire and an
      // omitted one now means "leave it as it is" rather than `false`, so this
      // page could no longer clear the flag by forgetting it — but a form whose
      // switch is off must be able to turn it off, and "the value the user is
      // looking at" is the only thing this dialog may send.
      governed,
      // Reference an existing named secret or create a new one. On edit with no
      // change, secretSel is `{}` so the backend keeps the current secret.
      ...secretSel,
    };

    try {
      if (isEdit && editConn) {
        await updateMutation.mutateAsync({ id: editConn.id, input });
      } else {
        const created = await createMutation.mutateAsync(input);
        onCreated?.(created);
      }
      closeAndReset();
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save the connection.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>
              {isEdit ? "Edit connection" : "Add connection"}
            </DialogTitle>
            <DialogDescription>
              {isEdit
                ? "Update the connection details. Leave password blank to keep the current secret."
                : "Connect a Trino or StarRocks query engine. Egret tests the connection once it is saved."}
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="db-name">Name</Label>
              <Input
                id="db-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. prod-trino"
                required
                autoFocus={!isEdit}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="db-engine">Engine</Label>
              <Select value={engine} onValueChange={(v) => setEngine(v as Engine)}>
                <SelectTrigger id="db-engine" className="w-full">
                  <SelectValue placeholder="Select an engine" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="trino">Trino</SelectItem>
                  <SelectItem value="starrocks">StarRocks</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="db-host">Host</Label>
                <Input
                  id="db-host"
                  value={host}
                  onChange={(e) => setHost(e.target.value)}
                  placeholder="e.g. trino.corp.local"
                  required
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="db-port">Port</Label>
                <Input
                  id="db-port"
                  type="number"
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  placeholder="e.g. 8080"
                  required
                />
              </div>
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="db-user">User</Label>
                <Input
                  id="db-user"
                  value={user}
                  onChange={(e) => setUser(e.target.value)}
                  placeholder="Optional"
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="db-http-scheme">HTTP scheme</Label>
                <Input
                  id="db-http-scheme"
                  value={httpScheme}
                  onChange={(e) => setHttpScheme(e.target.value)}
                  placeholder="http / https"
                />
              </div>
            </div>

            <div className="grid grid-cols-2 gap-4">
              <div className="space-y-1.5">
                <Label htmlFor="db-catalog">Catalog</Label>
                <Input
                  id="db-catalog"
                  value={catalog}
                  onChange={(e) => setCatalog(e.target.value)}
                  placeholder="Optional"
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="db-schema">Schema</Label>
                <Input
                  id="db-schema"
                  value={schema}
                  onChange={(e) => setSchema(e.target.value)}
                  placeholder="Optional"
                />
              </div>
            </div>

            {(engine === "trino" || engine === "starrocks") && (
              <div className="space-y-1.5">
                <div className="flex items-center justify-between">
                  <Label htmlFor="db-impersonate" className="font-normal">
                    Run queries as the logged-in user
                  </Label>
                  <Switch
                    id="db-impersonate"
                    checked={impersonate}
                    onCheckedChange={setImpersonate}
                    aria-label="Run queries as the logged-in user"
                  />
                </div>
                <p className="text-xs text-muted-foreground">
                  {engine === "trino"
                    ? "Requires Trino impersonation + Ranger"
                    : "Requires a StarRocks service user configured for EXECUTE AS + Ranger"}
                </p>
              </div>
            )}

            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="db-delegated" className="font-normal">
                  Per-user delegation
                </Label>
                <Switch
                  id="db-delegated"
                  checked={delegated}
                  onCheckedChange={setDelegated}
                  aria-label="Per-user delegation"
                />
              </div>
              <p className="text-xs text-muted-foreground">
                Run queries as the signed-in user (requires SSO)
              </p>
            </div>

            {/* Directly under the two switches it depends on, because the
                sentence below is only true in relation to them. */}
            <div className="space-y-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="db-governed" className="font-normal">
                  Governed data
                </Label>
                <Switch
                  id="db-governed"
                  checked={governed}
                  onCheckedChange={setGoverned}
                  aria-label="Governed data"
                />
              </div>
              <p className="text-xs text-muted-foreground">
                {governed && !delegated && !impersonate
                  ? "Row filters and column masks are applied per user at the engine. Data API endpoints on this connection cannot be run or published until per-user delegation or impersonation is on, and any already published stop serving."
                  : "Row filters and column masks are applied per user at the engine. Data API endpoints on this connection must run queries as the caller."}
              </p>
            </div>

            <SecretPicker
              ws={ws}
              value={secretSel}
              hasExisting={!!editConn?.has_secret}
              onChange={setSecretSel}
            />

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => handleOpenChange(false)}
            >
              Cancel
            </Button>
            {!isPending && missingHint && (
              <span className="self-center text-xs text-muted-foreground">{missingHint}</span>
            )}
            <Button type="submit" disabled={isPending || missing.length > 0}>
              {isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              {isEdit ? "Save changes" : "Add connection"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
