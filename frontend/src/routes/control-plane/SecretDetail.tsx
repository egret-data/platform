import { useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, HardDrive, KeyRound, Loader2, RotateCw, Trash2 } from "lucide-react";

import {
  type Secret,
  type SecretUpdate,
  deleteSecret,
  listSecrets,
  listStorage,
  updateSecret,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SYSTEM_SECRET_REASON } from "@/lib/systemSecrets";
import { secretsQueryKey } from "@/routes/control-plane/ControlPlaneSecrets";
import { SecretDataFields, type SecretDataHandle } from "@/routes/control-plane/SecretDataFields";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["overview", "usage"] as const;

/**
 * Control Plane → Secrets → detail.
 *
 * A per-secret page: its metadata (name/description are editable in place),
 * which storage connections reference it, and a Rotate-value action. Values
 * are never displayed — rotating writes new ones. There is no single-secret
 * GET, so the secret is read out of the cached list.
 */
export default function SecretDetail() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [sp, setSp] = useSearchParams();
  const tabParam = sp.get("tab") ?? "";
  const tab = (TABS as readonly string[]).includes(tabParam) ? tabParam : "overview";
  const setTab = (t: string) =>
    setSp(
      (prev) => {
        const n = new URLSearchParams(prev);
        n.set("tab", t);
        return n;
      },
      { replace: true },
    );

  const secretsQ = useQuery<Secret[], ApiError>({
    queryKey: secretsQueryKey,
    queryFn: listSecrets,
    retry: false,
  });
  const secret = secretsQ.data?.find((s) => s.id === id);

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [rotateOpen, setRotateOpen] = useState(false);

  const deleteMut = useMutation<void, ApiError, void>({
    mutationFn: () => deleteSecret(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: secretsQueryKey });
      navigate("/control-plane/secrets");
    },
  });

  const backLink = (
    <Link
      to="/control-plane/secrets"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Secrets
    </Link>
  );

  if (secretsQ.isLoading) {
    return <p className="p-6 text-sm text-muted-foreground">Loading secret…</p>;
  }
  if (!secret) {
    return (
      <div className="space-y-5">
        {backLink}
        <Alert variant="destructive">
          <AlertDescription>
            {secretsQ.error?.message ?? "This secret wasn't found."}
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  const inUse = secret.used_by > 0;
  const system = secret.system_managed;

  return (
    <div className="space-y-5">
      {backLink}

      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0 space-y-3">
          <div className="flex items-center gap-3">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <KeyRound className="h-6 w-6" />
            </div>
            <div className="min-w-0">
              <h1 className="truncate text-2xl font-semibold leading-tight">{secret.name}</h1>
              <div className="mt-1 flex items-center gap-2">
                <Badge variant="outline" className="font-mono text-xs">
                  {secret.type}
                </Badge>
                {system && (
                  <Badge variant="secondary" className="text-xs">
                    System
                  </Badge>
                )}
              </div>
            </div>
          </div>
          {system && <p className="text-sm text-muted-foreground">{SYSTEM_SECRET_REASON}</p>}
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm text-muted-foreground">
            <span>
              <span className="text-xs uppercase tracking-wide">Used by</span>{" "}
              <span className="text-foreground">
                {secret.used_by} connection{secret.used_by === 1 ? "" : "s"}
              </span>
            </span>
            <span>
              <span className="text-xs uppercase tracking-wide">Created</span>{" "}
              <span className="text-foreground">
                {new Date(secret.created_at).toLocaleDateString()}
              </span>
            </span>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={system}
            title={system ? SYSTEM_SECRET_REASON : undefined}
            onClick={() => setRotateOpen(true)}
          >
            <RotateCw className="mr-1 h-4 w-4" /> Rotate value
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="text-destructive hover:text-destructive"
            disabled={system || inUse}
            title={
              system
                ? SYSTEM_SECRET_REASON
                : inUse
                  ? `In use by ${secret.used_by} connection${secret.used_by === 1 ? "" : "s"}`
                  : undefined
            }
            onClick={() => setDeleteOpen(true)}
          >
            <Trash2 className="mr-1 h-4 w-4" /> Delete
          </Button>
        </div>
      </div>

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="overview">
            <KeyRound className="h-4 w-4" />
            Overview
          </TabsTrigger>
          <TabsTrigger value="usage">
            <HardDrive className="h-4 w-4" />
            Used by
          </TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="mt-4">
          <OverviewTab secret={secret} />
        </TabsContent>
        <TabsContent value="usage" className="mt-4">
          <UsageTab secretId={id} />
        </TabsContent>
      </Tabs>

      <RotateSecretDialog open={rotateOpen} onOpenChange={setRotateOpen} secret={secret} />
      <ConfirmDeleteDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={`Delete «${secret.name}»?`}
        description="This secret will be permanently removed. Connections that reference it will lose their credentials."
        confirmText="delete"
        onConfirm={() => deleteMut.mutate()}
      />
    </div>
  );
}

function OverviewTab({ secret }: { secret: Secret }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(secret.name);
  const [description, setDescription] = useState(secret.description ?? "");
  const [error, setError] = useState<string | null>(null);

  const updateMut = useMutation<Secret, ApiError, SecretUpdate>({
    mutationFn: (u) => updateSecret(secret.id, u),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: secretsQueryKey });
      setEditing(false);
    },
    onError: (e) => setError(e.message ?? "Failed to save the secret."),
  });

  const startEdit = () => {
    setName(secret.name);
    setDescription(secret.description ?? "");
    setError(null);
    setEditing(true);
  };
  const save = () => {
    setError(null);
    updateMut.mutate({ name: name.trim(), description: description.trim() || null });
  };

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="text-base font-semibold">Details</h2>
          <p className="text-sm text-muted-foreground">
            Metadata for this secret. Values are never shown — use Rotate value to replace them.
          </p>
        </div>
        {editing ? (
          <div className="flex shrink-0 gap-2">
            <Button size="sm" variant="outline" disabled={updateMut.isPending} onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button size="sm" disabled={updateMut.isPending} onClick={save}>
              Save
            </Button>
          </div>
        ) : (
          <Button
            size="sm"
            variant="outline"
            className="shrink-0"
            disabled={secret.system_managed}
            title={secret.system_managed ? SYSTEM_SECRET_REASON : undefined}
            onClick={startEdit}
          >
            Edit
          </Button>
        )}
      </div>

      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="w-44">Setting</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow>
              <TableCell className="align-middle font-medium">Name</TableCell>
              <TableCell className="align-middle">
                {editing ? (
                  <Input
                    aria-label="Name"
                    className="h-8 w-72"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                ) : (
                  <span className="text-sm">{secret.name}</span>
                )}
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell className="align-middle font-medium">Description</TableCell>
              <TableCell className="align-middle">
                {editing ? (
                  <Input
                    aria-label="Description"
                    className="h-8 w-72"
                    placeholder="Optional"
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                  />
                ) : (
                  <span className="text-sm text-muted-foreground">{secret.description || "—"}</span>
                )}
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell className="align-middle font-medium">Type</TableCell>
              <TableCell className="align-middle">
                <Badge variant="outline" className="font-mono text-xs">
                  {secret.type}
                </Badge>
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell className="align-middle font-medium">Keys</TableCell>
              <TableCell className="align-middle font-mono text-xs text-muted-foreground">
                {secret.keys.length > 0 ? secret.keys.join(", ") : "—"}
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell className="align-middle font-medium">Used by</TableCell>
              <TableCell className="align-middle">
                <Badge variant={secret.used_by > 0 ? "secondary" : "outline"}>
                  {secret.used_by}
                </Badge>
              </TableCell>
            </TableRow>
            <TableRow>
              <TableCell className="align-middle font-medium">Created</TableCell>
              <TableCell className="align-middle text-sm text-muted-foreground">
                {new Date(secret.created_at).toLocaleString()}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </section>
  );
}

function UsageTab({ secretId }: { secretId: string }) {
  const storageQ = useQuery({
    queryKey: ["control-plane", "storage"],
    queryFn: listStorage,
    retry: false,
  });
  const rows = (storageQ.data ?? []).filter((s) => s.secret_ref === secretId);

  return (
    <section className="space-y-3">
      <div className="space-y-1">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          <HardDrive className="h-4 w-4 text-muted-foreground" />
          Storage connections
        </h2>
        <p className="text-sm text-muted-foreground">
          Storage connections that reference this secret for their credentials.
        </p>
      </div>
      {storageQ.isLoading ? (
        <ListSkeleton rows={3} cols={3} />
      ) : rows.length === 0 ? (
        <p className="rounded-md border bg-muted/40 px-4 py-6 text-sm text-muted-foreground">
          No storage connections reference this secret.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-md border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Connection</TableHead>
                <TableHead>Provider</TableHead>
                <TableHead>Bucket</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((s) => (
                <TableRow key={s.id}>
                  <TableCell>
                    <div className="flex min-w-0 items-center gap-2.5">
                      <HardDrive className="h-4 w-4 shrink-0 text-muted-foreground" />
                      <Link
                        to={`/control-plane/storage/${s.id}`}
                        className="truncate font-medium text-primary hover:underline"
                      >
                        {s.name}
                      </Link>
                    </div>
                  </TableCell>
                  <TableCell>
                    <Badge variant="secondary">{s.provider}</Badge>
                  </TableCell>
                  <TableCell className="font-mono text-muted-foreground">{s.bucket}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </section>
  );
}

function RotateSecretDialog({
  open,
  onOpenChange,
  secret,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  secret: Secret;
}) {
  const qc = useQueryClient();
  const dataRef = useRef<SecretDataHandle>(null);
  const [error, setError] = useState<string | null>(null);
  const [formSeq, setFormSeq] = useState(0);
  useEffect(() => {
    if (open) {
      setFormSeq((s) => s + 1);
      setError(null);
    }
  }, [open]);

  const rotateMut = useMutation<Secret, ApiError, Record<string, string>>({
    mutationFn: (data) => updateSecret(secret.id, { data }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: secretsQueryKey });
      onOpenChange(false);
    },
    onError: (e) => setError(e.message ?? "Failed to rotate the secret."),
  });

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    const data = dataRef.current?.getData() ?? {};
    if (Object.keys(data).length === 0) {
      setError("Enter the new value(s).");
      return;
    }
    rotateMut.mutate(data);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Rotate «{secret.name}»</DialogTitle>
            <DialogDescription>
              Write new values for this {secret.type} secret. Connections referencing it pick up the
              new credentials immediately.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <SecretDataFields key={formSeq} ref={dataRef} type={secret.type} />
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={rotateMut.isPending}>
              {rotateMut.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Rotate value
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
