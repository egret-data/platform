import { useState } from "react";
import { useParams } from "react-router-dom";
import { KeyRound, Loader2, Pencil, Plus, Trash2 } from "lucide-react";

import {
  type WorkspaceSecret,
  type SecretInput,
  type SecretUpdate,
  useCreateSecret,
  useDeleteSecret,
  useSecrets,
  useUpdateSecret,
} from "@/api/secrets";
import { ApiError } from "@/api/client";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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

export default function SecretsPage() {
  const ws = useParams().workspace!;
  const secrets = useSecrets(ws);
  const deleteMutation = useDeleteSecret(ws);

  const [addOpen, setAddOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<WorkspaceSecret | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<WorkspaceSecret | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  async function confirmDelete() {
    if (!deleteTarget) return;
    setDeleteError(null);
    try {
      await deleteMutation.mutateAsync(deleteTarget.id);
      setDeleteTarget(null);
    } catch (err) {
      // 409 → "in use by N connections"; surface the server message.
      setDeleteError((err as ApiError).message ?? "Failed to delete secret.");
    }
  }

  if (secrets.isLoading) {
    return (
      <section>
        <SectionHeader onAdd={() => setAddOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={3} cols={5} />
        </div>
      </section>
    );
  }

  if (secrets.isError) {
    const status = (secrets.error as ApiError | null)?.status;
    if (status === 403) {
      return (
        <Alert>
          <KeyRound className="h-4 w-4" />
          <AlertTitle>Insufficient permissions</AlertTitle>
          <AlertDescription>
            Requires workspace-admin to manage secrets.
          </AlertDescription>
        </Alert>
      );
    }
    return (
      <Alert variant="destructive">
        <AlertTitle>Failed to load secrets</AlertTitle>
        <AlertDescription>Something went wrong. Please try again.</AlertDescription>
      </Alert>
    );
  }

  const rows = secrets.data ?? [];

  return (
    <section>
      <SectionHeader onAdd={() => setAddOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No secrets yet.</p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr className="border-b border-border">
                <th className="py-2 pr-4 font-medium">Name</th>
                <th className="py-2 pr-4 font-medium">Description</th>
                <th className="py-2 pr-4 font-medium">Used by</th>
                <th className="py-2 pr-4 font-medium">Created</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((secret) => {
                const inUse = secret.used_by > 0;
                return (
                  <tr key={secret.id} className="border-b border-border last:border-b-0">
                    <td className="py-2 pr-4 font-medium">{secret.name}</td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      {secret.description ?? (
                        <span className="text-muted-foreground/50">—</span>
                      )}
                    </td>
                    <td className="py-2 pr-4">
                      <Badge variant={inUse ? "secondary" : "outline"}>
                        {secret.used_by}
                      </Badge>
                    </td>
                    <td className="py-2 pr-4 text-muted-foreground">
                      {new Date(secret.created_at).toLocaleDateString()}
                    </td>
                    <td className="py-2 text-right">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-muted-foreground"
                          onClick={() => setEditTarget(secret)}
                          aria-label={`Edit ${secret.name}`}
                        >
                          <Pencil className="h-4 w-4" />
                          Edit
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-muted-foreground hover:text-destructive"
                          onClick={() => {
                            setDeleteError(null);
                            setDeleteTarget(secret);
                          }}
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
      <SecretDialog
        ws={ws}
        open={addOpen}
        onOpenChange={setAddOpen}
        editSecret={null}
      />

      {/* Edit dialog — key forces remount when target changes so useState initializers re-run */}
      <SecretDialog
        key={editTarget?.id ?? "__edit-closed"}
        ws={ws}
        open={editTarget !== null}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
        editSecret={editTarget}
      />

      {/* Delete confirm — surfaces a 409 "in use" error from the server. */}
      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(o) => {
          if (!o) {
            setDeleteTarget(null);
            setDeleteError(null);
          }
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {deleteTarget ? `Delete «${deleteTarget.name}»?` : "Delete secret?"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              This secret will be permanently removed. Connections that reference it
              will lose their credentials.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {deleteError && (
            <Alert variant="destructive">
              <AlertDescription>{deleteError}</AlertDescription>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                // Keep the dialog open so a 409 error can be shown.
                e.preventDefault();
                confirmDelete();
              }}
              disabled={deleteMutation.isPending}
            >
              {deleteMutation.isPending && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

function SectionHeader({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Secrets</h2>
        <p className="text-sm text-muted-foreground">
          Named secrets this workspace can reference from storage and database
          connections.
        </p>
      </div>
      <Button onClick={onAdd}>
        <Plus className="h-4 w-4" />
        Add secret
      </Button>
    </div>
  );
}

function SecretDialog({
  ws,
  open,
  onOpenChange,
  editSecret,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editSecret: WorkspaceSecret | null;
}) {
  const isEdit = editSecret !== null;
  const createMutation = useCreateSecret(ws);
  const updateMutation = useUpdateSecret(ws);

  const [name, setName] = useState(editSecret?.name ?? "");
  const [description, setDescription] = useState(editSecret?.description ?? "");
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  function resetToSecret(secret: WorkspaceSecret | null) {
    setName(secret?.name ?? "");
    setDescription(secret?.description ?? "");
    setValue("");
    setError(null);
    createMutation.reset();
    updateMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) resetToSecret(null);
    onOpenChange(next);
  }

  const isPending = createMutation.isPending || updateMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    try {
      if (isEdit && editSecret) {
        const update: SecretUpdate = {
          name: name.trim(),
          description: description.trim().length > 0 ? description.trim() : null,
          // Blank value = keep existing; a filled value rotates it.
          value: value.length > 0 ? value : undefined,
        };
        await updateMutation.mutateAsync({ id: editSecret.id, input: update });
      } else {
        const input: SecretInput = {
          name: name.trim(),
          description: description.trim().length > 0 ? description.trim() : null,
          value,
        };
        await createMutation.mutateAsync(input);
      }
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
            <DialogTitle>{isEdit ? "Edit secret" : "Add secret"}</DialogTitle>
            <DialogDescription>
              {isEdit
                ? "Rename, update the description, or rotate the value. Leave the value blank to keep the current one."
                : "Create a named secret that storage and database connections can reference."}
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="secret-name">Name</Label>
              <Input
                id="secret-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. prod-s3-secret"
                required
                autoFocus={!isEdit}
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="secret-description">Description</Label>
              <Input
                id="secret-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Optional"
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="secret-value">
                {isEdit ? "New value (leave blank to keep)" : "Value"}
              </Label>
              <Input
                id="secret-value"
                type="password"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="Enter a value"
                required={!isEdit}
                autoComplete="new-password"
              />
            </div>

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
              {isEdit ? "Save changes" : "Add secret"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
