import { useState } from "react";
import { useParams } from "react-router-dom";
import {
  CheckCircle2,
  Database as DatabaseIcon,
  Loader2,
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
  useCreateDatabase,
  useDatabases,
  useDeleteDatabase,
  useTestDatabase,
  useUpdateDatabase,
} from "@/api/databases";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
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

function statusVariant(status: string): "default" | "destructive" | "secondary" {
  if (status === "ok") return "default";
  if (status === "error") return "destructive";
  return "secondary";
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
      setTestResult((prev) => ({ ...prev, [conn.id]: outcome }));
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
    if (status === 403) {
      return (
        <Alert>
          <DatabaseIcon className="h-4 w-4" />
          <AlertTitle>Insufficient permissions</AlertTitle>
          <AlertDescription>
            Requires workspace-admin to manage engines.
          </AlertDescription>
        </Alert>
      );
    }
    return (
      <Alert variant="destructive">
        <AlertTitle>Failed to load engines</AlertTitle>
        <AlertDescription>Something went wrong. Please try again.</AlertDescription>
      </Alert>
    );
  }

  const rows = dbs.data ?? [];

  return (
    <section>
      <SectionHeader onAdd={() => setAddOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No engines connected yet.</p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr className="border-b border-border">
                <th className="py-2 pr-4 font-medium">Name</th>
                <th className="py-2 pr-4 font-medium">Engine</th>
                <th className="py-2 pr-4 font-medium">Host:Port</th>
                <th className="py-2 pr-4 font-medium">Status</th>
                <th className="py-2 pr-4 font-medium">Secret</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((conn) => {
                const result = testResult[conn.id];
                return (
                  <tr key={conn.id} className="border-b border-border last:border-b-0">
                    <td className="py-2 pr-4 font-medium">{conn.name}</td>
                    <td className="py-2 pr-4">
                      <Badge variant="secondary">{conn.engine}</Badge>
                    </td>
                    <td className="py-2 pr-4 font-mono text-muted-foreground">
                      {conn.host}:{conn.port}
                    </td>
                    <td className="py-2 pr-4">
                      <Badge variant={statusVariant(conn.status)}>{conn.status}</Badge>
                    </td>
                    <td className="py-2 pr-4">
                      <Badge variant={conn.has_secret ? "secondary" : "outline"}>
                        {conn.has_secret ? "Configured" : "Not set"}
                      </Badge>
                    </td>
                    <td className="py-2 text-right">
                      <div className="flex items-center justify-end gap-1">
                        {result && (
                          <span
                            className={`mr-1 inline-flex items-center gap-1 text-xs ${
                              result.ok ? "text-emerald-600" : "text-destructive"
                            }`}
                            role="status"
                          >
                            {result.ok ? (
                              <CheckCircle2 className="h-3.5 w-3.5" />
                            ) : (
                              <XCircle className="h-3.5 w-3.5" />
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
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      {/* Add dialog */}
      <DatabaseConnDialog
        ws={ws}
        open={addOpen}
        onOpenChange={setAddOpen}
        editConn={null}
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
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete engine connection?"}
        description="This engine connection will be removed. Any SQL Lab tabs or charts that reference it may break."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </section>
  );
}

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Engines</h2>
        <p className="text-sm text-muted-foreground">
          Query engine connections this workspace can use in SQL Lab and charts.
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        Add connection
      </Button>
    </div>
  );
}

function DatabaseConnDialog({
  ws,
  open,
  onOpenChange,
  editConn,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editConn: DatabaseConn | null;
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
    setSecretSel({});
    setError(null);
    createMutation.reset();
    updateMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) resetToConn(null);
    onOpenChange(next);
  }

  const isPending = createMutation.isPending || updateMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const options = {
      user: user.trim().length > 0 ? user.trim() : undefined,
      http_scheme: httpScheme.trim().length > 0 ? httpScheme.trim() : undefined,
      catalog: catalog.trim().length > 0 ? catalog.trim() : undefined,
      database: schema.trim().length > 0 ? schema.trim() : undefined,
    };

    const input: DatabaseConnInput = {
      name: name.trim(),
      engine,
      host: host.trim(),
      port: Number(port),
      options,
      // Reference an existing named secret or create a new one. On edit with no
      // change, secretSel is `{}` so the backend keeps the current secret.
      ...secretSel,
    };

    try {
      if (isEdit && editConn) {
        await updateMutation.mutateAsync({ id: editConn.id, input });
      } else {
        await createMutation.mutateAsync(input);
      }
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save engine connection.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>
              {isEdit ? "Edit engine connection" : "Add engine connection"}
            </DialogTitle>
            <DialogDescription>
              {isEdit
                ? "Update the connection details. Leave password blank to keep the current secret."
                : "Connect a Trino or StarRocks query engine. Egret validates reachability on save."}
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
            <Button type="submit" disabled={isPending}>
              {isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              {isEdit ? "Save changes" : "Add connection"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
