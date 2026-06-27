import { useState } from "react";
import { useParams } from "react-router-dom";
import { Check, Copy, KeyRound, Loader2, Plus, Trash2 } from "lucide-react";

import {
  type CatalogTokenDto,
  type MintTokenRequest,
  useCatalogTokens,
  useMintToken,
  useRevokeToken,
} from "@/api/catalog";
import { ApiError } from "@/api/client";
import { RelativeTime } from "@/components/explore/RelativeTime";
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

export default function CatalogAccess() {
  const ws = useParams().workspace!;
  const tokens = useCatalogTokens(ws);
  const revokeMutation = useRevokeToken(ws);

  const [mintOpen, setMintOpen] = useState(false);
  const [revokeTarget, setRevokeTarget] = useState<CatalogTokenDto | null>(null);

  if (tokens.isLoading) {
    return (
      <section>
        <SectionHeader onMint={() => setMintOpen(true)} />
        <div className="mt-4">
          <ListSkeleton rows={4} cols={4} />
        </div>
      </section>
    );
  }

  if (tokens.isError) {
    const status = (tokens.error as ApiError | null)?.status;
    if (status === 403) {
      return (
        <Alert>
          <KeyRound className="h-4 w-4" />
          <AlertTitle>Insufficient permissions</AlertTitle>
          <AlertDescription>
            Requires workspace-admin to manage catalog access.
          </AlertDescription>
        </Alert>
      );
    }
    return (
      <Alert variant="destructive">
        <AlertTitle>Failed to load catalog tokens</AlertTitle>
        <AlertDescription>Something went wrong. Please try again.</AlertDescription>
      </Alert>
    );
  }

  const rows = tokens.data ?? [];

  return (
    <section>
      <SectionHeader onMint={() => setMintOpen(true)} />

      <div className="mt-4">
        {rows.length === 0 ? (
          <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
            <p>No catalog tokens yet.</p>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr className="border-b border-border">
                <th className="py-2 pr-4 font-medium">Label</th>
                <th className="py-2 pr-4 font-medium">Permissions</th>
                <th className="py-2 pr-4 font-medium">Namespaces</th>
                <th className="py-2 pr-4 font-medium">Created</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((token) => (
                <tr key={token.id} className="border-b border-border last:border-b-0">
                  <td className="py-2 pr-4 font-medium">{token.label}</td>
                  <td className="py-2 pr-4">
                    <div className="flex flex-wrap gap-1">
                      {token.permissions.includes("catalog:read") && (
                        <Badge variant="secondary">read</Badge>
                      )}
                      {token.permissions.includes("catalog:write") && (
                        <Badge variant="secondary">write</Badge>
                      )}
                    </div>
                  </td>
                  <td className="py-2 pr-4 font-mono text-muted-foreground">
                    {token.namespace_scope.length === 0
                      ? "*"
                      : token.namespace_scope.join(", ")}
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    <RelativeTime iso={token.created_at} />
                  </td>
                  <td className="py-2 text-right">
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-muted-foreground hover:text-destructive"
                      onClick={() => setRevokeTarget(token)}
                    >
                      <Trash2 className="h-4 w-4" />
                      Revoke
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <MintTokenDialog ws={ws} open={mintOpen} onOpenChange={setMintOpen} />

      <ConfirmDeleteDialog
        open={revokeTarget !== null}
        onOpenChange={(o) => {
          if (!o) setRevokeTarget(null);
        }}
        title={revokeTarget ? `Revoke token «${revokeTarget.label}»?` : "Revoke token?"}
        description="Any client using this token will immediately lose catalog access. This cannot be undone."
        confirmText="revoke"
        onConfirm={() => {
          if (revokeTarget) revokeMutation.mutate(revokeTarget.id);
        }}
      />
    </section>
  );
}

function SectionHeader({ onMint }: { onMint: () => void }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <div>
        <h2 className="text-base font-semibold">Catalog access</h2>
        <p className="text-sm text-muted-foreground">
          Mint and manage tokens that grant programmatic access to this workspace&apos;s catalog.
        </p>
      </div>
      <Button onClick={onMint}>
        <Plus className="h-4 w-4" />
        Mint token
      </Button>
    </div>
  );
}

type MintPhase = "form" | "reveal";

function MintTokenDialog({
  ws,
  open,
  onOpenChange,
}: {
  ws: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const mintMutation = useMintToken(ws);

  const [phase, setPhase] = useState<MintPhase>("form");
  const [label, setLabel] = useState("");
  const [read, setRead] = useState(true);
  const [write, setWrite] = useState(false);
  const [namespaces, setNamespaces] = useState("");
  const [raw, setRaw] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setPhase("form");
    setLabel("");
    setRead(true);
    setWrite(false);
    setNamespaces("");
    setRaw(null);
    setCopied(false);
    setError(null);
    mintMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit =
    label.trim().length > 0 && (read || write) && !mintMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);

    const permissions = [...(read ? ["read"] : []), ...(write ? ["write"] : [])];
    const namespace_scope = namespaces
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    const req: MintTokenRequest = {
      label: label.trim(),
      permissions,
      namespace_scope,
    };

    try {
      const res = await mintMutation.mutateAsync(req);
      setRaw(res.raw);
      setPhase("reveal");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to mint token.");
    }
  }

  async function handleCopy() {
    if (!raw) return;
    try {
      await navigator.clipboard.writeText(raw);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard may be unavailable; the token is still selectable manually.
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        {phase === "form" ? (
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>Mint catalog token</DialogTitle>
              <DialogDescription>
                Create a token for programmatic catalog access. The raw secret is shown only once.
              </DialogDescription>
            </DialogHeader>

            <div className="my-4 space-y-4">
              <div className="space-y-1.5">
                <span className="text-sm font-medium">Label</span>
                <Input
                  value={label}
                  onChange={(e) => setLabel(e.target.value)}
                  placeholder="e.g. dbt-ci"
                  autoFocus
                />
              </div>

              <div className="space-y-1.5">
                <span className="text-sm font-medium">Permissions</span>
                <div className="flex gap-2">
                  <PermissionToggle
                    label="Read"
                    checked={read}
                    onChange={setRead}
                  />
                  <PermissionToggle
                    label="Write"
                    checked={write}
                    onChange={setWrite}
                  />
                </div>
                {!read && !write && (
                  <p className="text-xs text-destructive">
                    Select at least one permission.
                  </p>
                )}
              </div>

              <div className="space-y-1.5">
                <span className="text-sm font-medium">Namespaces</span>
                <Input
                  value={namespaces}
                  onChange={(e) => setNamespaces(e.target.value)}
                  placeholder="marts, staging — leave blank for all"
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
              <Button type="submit" disabled={!canSubmit}>
                {mintMutation.isPending && (
                  <Loader2 className="h-4 w-4 animate-spin" />
                )}
                Mint token
              </Button>
            </DialogFooter>
          </form>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>Token created</DialogTitle>
              <DialogDescription>
                Use this secret as the bearer token for catalog API requests.
              </DialogDescription>
            </DialogHeader>

            <div className="my-4 space-y-3">
              <div className="flex items-center gap-2">
                <code className="block flex-1 select-all overflow-x-auto rounded border border-border bg-muted px-3 py-2 font-mono text-sm">
                  {raw}
                </code>
                <Button type="button" variant="outline" size="sm" onClick={handleCopy}>
                  {copied ? (
                    <>
                      <Check className="h-4 w-4" />
                      Copied!
                    </>
                  ) : (
                    <>
                      <Copy className="h-4 w-4" />
                      Copy
                    </>
                  )}
                </Button>
              </div>
              <p className="text-xs text-destructive">
                Copy this token now — you won&apos;t be able to see it again.
              </p>
            </div>

            <DialogFooter>
              <Button type="button" onClick={() => handleOpenChange(false)}>
                Done
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function PermissionToggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={checked}
      onClick={() => onChange(!checked)}
      className={`inline-flex items-center gap-2 rounded-md border px-3 py-1.5 text-sm transition-colors ${
        checked
          ? "border-primary bg-primary/10 text-foreground"
          : "border-border text-muted-foreground hover:bg-accent/50"
      }`}
    >
      <span
        className={`flex h-4 w-4 items-center justify-center rounded-sm border ${
          checked ? "border-primary bg-primary text-primary-foreground" : "border-border"
        }`}
      >
        {checked && <Check className="h-3 w-3" />}
      </span>
      {label}
    </button>
  );
}
