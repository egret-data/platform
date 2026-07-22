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
import { EmptyState } from "@/components/explore/EmptyState";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
import { Textarea } from "@/components/ui/textarea";

/** Selectable k8s-style secret types. */
const SECRET_TYPES = [
  "Opaque",
  "kubernetes.io/tls",
  "kubernetes.io/basic-auth",
  "kubernetes.io/dockerconfigjson",
  "kubernetes.io/ssh-auth",
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

  const columns: Column<Secret>[] = [
    {
      key: "name",
      label: "Name",
      sortable: true,
      className: "font-medium",
      value: (s) => `${s.name} ${s.keys.join(" ")}`,
      cell: (s) => (
        <div>
          <div>{s.name}</div>
          {s.keys.length > 0 && (
            <div className="font-mono text-xs font-normal text-muted-foreground">
              {s.keys.join(", ")}
            </div>
          )}
        </div>
      ),
    },
    {
      key: "type",
      label: "Type",
      sortable: true,
      value: (s) => s.type,
      cell: (s) => (
        <Badge variant="outline" className="font-mono text-xs">
          {s.type}
        </Badge>
      ),
    },
    {
      key: "description",
      label: "Description",
      value: (s) => s.description ?? "",
      cell: (s) => <span className="text-muted-foreground">{s.description ?? "—"}</span>,
    },
    {
      key: "used_by",
      label: "Used by",
      sortable: true,
      value: (s) => s.used_by,
      cell: (s) => (
        <Badge variant={s.used_by > 0 ? "secondary" : "outline"}>{s.used_by}</Badge>
      ),
    },
    {
      key: "created",
      label: "Created",
      sortable: true,
      value: (s) => s.created_at,
      cell: (s) => (
        <span className="text-muted-foreground">
          {new Date(s.created_at).toLocaleDateString()}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      className: "w-10 text-right",
      cell: (s) => {
        const inUse = s.used_by > 0;
        return (
          <div className="flex justify-end">
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-destructive"
              onClick={() => setDeleteTarget(s)}
              disabled={inUse}
              title={
                inUse
                  ? `In use by ${s.used_by} connection${s.used_by === 1 ? "" : "s"}`
                  : undefined
              }
              aria-label={`Delete ${s.name}`}
            >
              <Trash2 className="h-4 w-4" />
              Delete
            </Button>
          </div>
        );
      },
    },
  ];

  return (
    <div className="space-y-4" data-testid="secrets-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <h1 className="text-2xl font-semibold leading-tight">Secrets</h1>
          <p className="text-sm text-muted-foreground">
            Global named secrets that storage and database connections across the platform can
            reference. Values are never displayed.
          </p>
        </div>
        <Button className="shrink-0" onClick={() => setAddOpen(true)}>
          <Plus className="h-4 w-4" />
          Add secret
        </Button>
      </div>

      <SummaryStrip items={[{ label: "secrets", value: rows.length }]} />

      {secrets.isError ? (
        <Alert variant="destructive">
          <AlertDescription>Failed to load secrets. Please try again.</AlertDescription>
        </Alert>
      ) : secrets.isLoading ? (
        <ListSkeleton rows={5} cols={6} />
      ) : (
        <FilterableTable
          rows={rows}
          columns={columns}
          getId={(s) => s.id}
          searchPlaceholder="Search secrets…"
          initialSort={{ key: "name", dir: "asc" }}
          pageSize={25}
          bordered
          emptyText={
            <EmptyState
              icon={KeyRound}
              title="No secrets yet"
              description="Add a global named secret that storage and database connections can reference."
              action={
                <Button size="sm" onClick={() => setAddOpen(true)}>
                  <Plus className="mr-1 h-4 w-4" />
                  Add secret
                </Button>
              }
            />
          }
        />
      )}

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
    </div>
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
  // kubernetes.io/dockerconfigjson
  const [dockerRegistry, setDockerRegistry] = useState("");
  const [dockerUsername, setDockerUsername] = useState("");
  const [dockerPassword, setDockerPassword] = useState("");
  const [dockerEmail, setDockerEmail] = useState("");
  // kubernetes.io/ssh-auth
  const [sshKey, setSshKey] = useState("");
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
    setDockerRegistry("");
    setDockerUsername("");
    setDockerPassword("");
    setDockerEmail("");
    setSshKey("");
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
    if (type === "kubernetes.io/dockerconfigjson") {
      const registry = dockerRegistry.trim();
      const auth = btoa(`${dockerUsername}:${dockerPassword}`);
      const dockercfg = JSON.stringify({
        auths: {
          [registry]: {
            username: dockerUsername,
            password: dockerPassword,
            email: dockerEmail.trim() || undefined,
            auth,
          },
        },
      });
      return { ".dockerconfigjson": dockercfg };
    }
    if (type === "kubernetes.io/ssh-auth") {
      return { "ssh-privatekey": sshKey };
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

            {type === "kubernetes.io/dockerconfigjson" && (
              <>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-docker-registry">Registry</Label>
                  <Input
                    id="cp-secret-docker-registry"
                    value={dockerRegistry}
                    onChange={(e) => setDockerRegistry(e.target.value)}
                    placeholder="registry.example.com"
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-docker-username">Username</Label>
                  <Input
                    id="cp-secret-docker-username"
                    value={dockerUsername}
                    onChange={(e) => setDockerUsername(e.target.value)}
                    placeholder="Username"
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-docker-password">Password</Label>
                  <Input
                    id="cp-secret-docker-password"
                    type="password"
                    value={dockerPassword}
                    onChange={(e) => setDockerPassword(e.target.value)}
                    placeholder="Password"
                    autoComplete="new-password"
                    required
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="cp-secret-docker-email">Email</Label>
                  <Input
                    id="cp-secret-docker-email"
                    type="email"
                    value={dockerEmail}
                    onChange={(e) => setDockerEmail(e.target.value)}
                    placeholder="Optional"
                  />
                </div>
              </>
            )}

            {type === "kubernetes.io/ssh-auth" && (
              <div className="space-y-1.5">
                <Label htmlFor="cp-secret-ssh-key">ssh-privatekey</Label>
                <Textarea
                  id="cp-secret-ssh-key"
                  value={sshKey}
                  onChange={(e) => setSshKey(e.target.value)}
                  placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                  className="font-mono text-xs"
                  rows={4}
                  required
                />
              </div>
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
