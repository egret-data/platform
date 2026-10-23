import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Plus, Trash2 } from "lucide-react";

import {
  type Secret,
  type SecretInput,
  createSecret,
  deleteSecret,
  listSecrets,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { SYSTEM_SECRET_REASON } from "@/lib/systemSecrets";
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
import {
  SECRET_TYPES,
  SecretDataFields,
  type SecretDataHandle,
} from "@/routes/control-plane/SecretDataFields";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";

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
          <Link to={`/control-plane/secrets/${s.id}`} className={ENTITY_LINK_CLASS}>
            {s.name}
          </Link>
          {s.system_managed && (
            <Badge variant="secondary" className="ml-2 text-xs" title={SYSTEM_SECRET_REASON}>
              System
            </Badge>
          )}
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
        const reason = s.system_managed
          ? SYSTEM_SECRET_REASON
          : inUse
            ? `In use by ${s.used_by} connection${s.used_by === 1 ? "" : "s"}`
            : undefined;
        return (
          <div className="flex justify-end">
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-destructive"
              onClick={() => setDeleteTarget(s)}
              disabled={s.system_managed || inUse}
              title={reason}
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
    <PageContainer>
      <div className="space-y-4" data-testid="secrets-frame">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="max-w-3xl">
          <PageTitle>Secrets</PageTitle>
          <p className="text-sm text-muted-foreground">
            Global named secrets that storage and database connections across the platform can
            reference. Values are never displayed. Secrets marked System are created by egret for
            engines and catalogs and are managed with them.
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
    </PageContainer>
  );
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
  const [error, setError] = useState<string | null>(null);
  const dataRef = useRef<SecretDataHandle>(null);
  // Bump on each open to remount SecretDataFields with cleared, write-only inputs.
  const [formSeq, setFormSeq] = useState(0);
  useEffect(() => {
    if (open) setFormSeq((s) => s + 1);
  }, [open]);

  function reset() {
    setName("");
    setDescription("");
    setType("Opaque");
    setError(null);
    createMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    const data = dataRef.current?.getData() ?? {};
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

            <SecretDataFields key={`${type}-${formSeq}`} ref={dataRef} type={type} />

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
