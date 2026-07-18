import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Plus, Trash2, X } from "lucide-react";

import {
  type Secret,
  type SecretInput,
  createSecret,
  deleteSecret,
  listSecrets,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";

const COLUMNS = ["Name", "Type", "Description", "Used by", "Created", ""];

/** Selectable k8s-style secret types. */
const SECRET_TYPES = [
  "Opaque",
  "kubernetes.io/tls",
  "kubernetes.io/basic-auth",
] as const;

export const secretsQueryKey = ["control-plane", "secrets"] as const;

/**
 * Control Plane → Secrets
 *
 * Platform-admin registry of GLOBAL named secrets. Lists, adds, and deletes
 * typed secrets. Plaintext values are never displayed — write-only on create.
 */
export default function ControlPlaneSecrets() {
  const secrets = useQuery<Secret[], ApiError>({
    queryKey: secretsQueryKey,
    queryFn: listSecrets,
    retry: false,
  });
  const queryClient = useQueryClient();
  const deleteMutation = useMutation<void, ApiError, string>({
    mutationFn: deleteSecret,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: secretsQueryKey }),
  });

  const [addOpen, setAddOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Secret | null>(null);

  const rows = secrets.data ?? [];

  return (
    <Card data-testid="secrets-frame">
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle className="flex items-center gap-2 text-lg">
              <KeyRound className="h-5 w-5 text-muted-foreground" />
              Secrets
            </CardTitle>
            <CardDescription>
              Global named secrets that storage and database connections across
              the platform can reference. Values are never displayed.
            </CardDescription>
          </div>
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="h-4 w-4" />
            Add secret
          </Button>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <SummaryStrip items={[{ label: "secrets", value: rows.length }]} />
        {secrets.isError ? (
          <Alert variant="destructive">
            <AlertDescription>
              Failed to load secrets. Please try again.
            </AlertDescription>
          </Alert>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                {COLUMNS.map((col) => (
                  <TableHead
                    key={col || "actions"}
                    className={col === "" ? "text-right" : undefined}
                  >
                    {col || <span className="sr-only">Actions</span>}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow className="hover:bg-transparent">
                  <TableCell
                    colSpan={COLUMNS.length}
                    className="py-10 text-center text-muted-foreground"
                  >
                    {secrets.isLoading
                      ? "Loading…"
                      : "No secrets yet — add one to get started."}
                  </TableCell>
                </TableRow>
              ) : (
                rows.map((secret) => {
                  const inUse = secret.used_by > 0;
                  return (
                    <TableRow key={secret.id}>
                      <TableCell className="font-medium">
                        <div>{secret.name}</div>
                        {secret.keys.length > 0 && (
                          <div className="text-xs font-normal text-muted-foreground">
                            {secret.keys.join(", ")}
                          </div>
                        )}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline" className="font-mono text-xs">
                          {secret.type}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        {secret.description ?? "—"}
                      </TableCell>
                      <TableCell>
                        <Badge variant={inUse ? "secondary" : "outline"}>
                          {secret.used_by}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        {new Date(secret.created_at).toLocaleDateString()}
                      </TableCell>
                      <TableCell className="text-right">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-muted-foreground hover:text-destructive"
                          onClick={() => setDeleteTarget(secret)}
                          disabled={inUse}
                          title={
                            inUse
                              ? `In use by ${secret.used_by} connection${
                                  secret.used_by === 1 ? "" : "s"
                                }`
                              : undefined
                          }
                          aria-label={`Delete ${secret.name}`}
                        >
                          <Trash2 className="h-4 w-4" />
                          Delete
                        </Button>
                      </TableCell>
                    </TableRow>
                  );
                })
              )}
            </TableBody>
          </Table>
        )}
      </CardContent>

      <AddSecretDialog open={addOpen} onOpenChange={setAddOpen} />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) setDeleteTarget(null);
        }}
        title={deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete secret?"}
        description="This secret will be permanently removed. Connections that reference it will lose their credentials."
        confirmText="delete"
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
        }}
      />
    </Card>
  );
}

interface OpaqueRow {
  key: string;
  value: string;
}

function AddSecretDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const createMutation = useMutation<Secret, ApiError, SecretInput>({
    mutationFn: createSecret,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: secretsQueryKey }),
  });

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [type, setType] = useState<string>("Opaque");
  // Opaque: key→value rows (never render an existing value; write-only).
  const [rows, setRows] = useState<OpaqueRow[]>([{ key: "value", value: "" }]);
  // kubernetes.io/tls
  const [tlsCrt, setTlsCrt] = useState("");
  const [tlsKey, setTlsKey] = useState("");
  // kubernetes.io/basic-auth
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setDescription("");
    setType("Opaque");
    setRows([{ key: "value", value: "" }]);
    setTlsCrt("");
    setTlsKey("");
    setUsername("");
    setPassword("");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function buildData(): Record<string, string> {
    if (type === "kubernetes.io/tls") {
      return { "tls.crt": tlsCrt, "tls.key": tlsKey };
    }
    if (type === "kubernetes.io/basic-auth") {
      return { username, password };
    }
    // Opaque
    return Object.fromEntries(
      rows.filter((r) => r.key.trim().length > 0).map((r) => [r.key.trim(), r.value]),
    );
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const data = buildData();
    if (Object.keys(data).length === 0) {
      setError("Add at least one data key.");
      return;
    }

    const input: SecretInput = {
      name: name.trim(),
      description: description.trim().length > 0 ? description.trim() : null,
      type,
      data,
    };

    try {
      await createMutation.mutateAsync(input);
      handleOpenChange(false);
    } catch (err) {
      setError((err as ApiError).message ?? "Failed to save secret.");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Add secret</DialogTitle>
            <DialogDescription>
              Create a global named secret that storage and database connections
              can reference.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="cp-secret-name">Name</Label>
              <Input
                id="cp-secret-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. prod-s3-secret"
                required
                autoFocus
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cp-secret-description">Description</Label>
              <Input
                id="cp-secret-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Optional"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="cp-secret-type">Type</Label>
              <Select value={type} onValueChange={setType}>
                <SelectTrigger id="cp-secret-type">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SECRET_TYPES.map((t) => (
                    <SelectItem key={t} value={t}>
                      {t}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {type === "Opaque" && (
              <div className="space-y-2">
                <Label>Data</Label>
                {rows.map((row, i) => (
                  <div key={i} className="flex items-center gap-2">
                    <Input
                      aria-label={`Key ${i + 1}`}
                      value={row.key}
                      onChange={(e) =>
                        setRows((rs) =>
                          rs.map((r, j) =>
                            j === i ? { ...r, key: e.target.value } : r,
                          ),
                        )
                      }
                      placeholder="key"
                      className="flex-1"
                    />
                    <Input
                      aria-label={`Value ${i + 1}`}
                      type="password"
                      value={row.value}
                      onChange={(e) =>
                        setRows((rs) =>
                          rs.map((r, j) =>
                            j === i ? { ...r, value: e.target.value } : r,
                          ),
                        )
                      }
                      placeholder="value"
                      autoComplete="new-password"
                      className="flex-1"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="text-muted-foreground hover:text-destructive"
                      onClick={() =>
                        setRows((rs) =>
                          rs.length > 1 ? rs.filter((_, j) => j !== i) : rs,
                        )
                      }
                      disabled={rows.length <= 1}
                      aria-label={`Remove row ${i + 1}`}
                    >
                      <X className="h-4 w-4" />
                    </Button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setRows((rs) => [...rs, { key: "", value: "" }])}
                >
                  <Plus className="h-4 w-4" />
                  Add key
                </Button>
              </div>
            )}

            {type === "kubernetes.io/tls" && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-tls-crt">tls.crt</Label>
                  <Textarea
                    id="cp-secret-tls-crt"
                    value={tlsCrt}
                    onChange={(e) => setTlsCrt(e.target.value)}
                    placeholder="-----BEGIN CERTIFICATE-----"
                    className="font-mono text-xs"
                    rows={4}
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-tls-key">tls.key</Label>
                  <Textarea
                    id="cp-secret-tls-key"
                    value={tlsKey}
                    onChange={(e) => setTlsKey(e.target.value)}
                    placeholder="-----BEGIN PRIVATE KEY-----"
                    className="font-mono text-xs"
                    rows={4}
                    required
                  />
                </div>
              </>
            )}

            {type === "kubernetes.io/basic-auth" && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-username">Username</Label>
                  <Input
                    id="cp-secret-username"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                    placeholder="Username"
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-password">Password</Label>
                  <Input
                    id="cp-secret-password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="Password"
                    autoComplete="new-password"
                    required
                  />
                </div>
              </>
            )}

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
            <Button type="submit" disabled={createMutation.isPending}>
              {createMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Add secret
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
