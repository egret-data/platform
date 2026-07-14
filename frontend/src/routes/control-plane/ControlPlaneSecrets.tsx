import { useState } from "react";
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const COLUMNS = ["Name", "Description", "Used by", "Created", ""];

export const secretsQueryKey = ["control-plane", "secrets"] as const;

/**
 * Control Plane → Secrets
 *
 * Platform-admin registry of GLOBAL named secrets. Lists, adds, and deletes
 * secrets. Plaintext values are never displayed — write-only on create.
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
                      <TableCell className="font-medium">{secret.name}</TableCell>
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
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setDescription("");
    setValue("");
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

    const input: SecretInput = {
      name: name.trim(),
      description: description.trim().length > 0 ? description.trim() : null,
      value,
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
              <Label htmlFor="cp-secret-value">Value</Label>
              <Input
                id="cp-secret-value"
                type="password"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="Enter a value"
                required
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
