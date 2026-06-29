import { useState } from "react";
import { Check, Copy, KeyRound, Loader2, Pencil, Plus, RotateCw, Trash2 } from "lucide-react";

import {
  type CatalogTokenDto,
  type MintTokenRequest,
  type UpdateTokenRequest,
  useCatalogTokens,
  useMintToken,
  useRevokeToken,
  useRotateToken,
  useUpdateToken,
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

/** Split free text on newlines/commas, trim, drop empties → string[]. */
function parseScope(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** A table scope entry must be a dotted `namespace.table` with ≥2 non-empty segments. */
function invalidTableScope(entries: string[]): string | null {
  for (const t of entries) {
    const segs = t.split(".");
    if (segs.length < 2 || segs.some((s) => s.length === 0)) {
      return `Invalid table scope «${t}»: use a dotted namespace.table (e.g. sales.orders).`;
    }
  }
  return null;
}

export default function CatalogAccess({ ws, wid }: { ws: string; wid: string }) {
  const tokens = useCatalogTokens(ws, wid);
  const revokeMutation = useRevokeToken(ws, wid);

  const [mintOpen, setMintOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<CatalogTokenDto | null>(null);
  const [rotateTarget, setRotateTarget] = useState<CatalogTokenDto | null>(null);
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
                <th className="py-2 pr-4 font-medium">Tables</th>
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
                  <td className="py-2 pr-4 font-mono text-muted-foreground">
                    {token.table_scope.length === 0
                      ? "All tables"
                      : token.table_scope.join(", ")}
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground">
                    <RelativeTime iso={token.created_at} />
                  </td>
                  <td className="py-2 text-right">
                    <div className="flex justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-foreground"
                        onClick={() => setEditTarget(token)}
                      >
                        <Pencil className="h-4 w-4" />
                        Edit
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-foreground"
                        onClick={() => setRotateTarget(token)}
                      >
                        <RotateCw className="h-4 w-4" />
                        Rotate
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() => setRevokeTarget(token)}
                      >
                        <Trash2 className="h-4 w-4" />
                        Revoke
                      </Button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <MintTokenDialog ws={ws} wid={wid} open={mintOpen} onOpenChange={setMintOpen} />

      <EditTokenDialog
        ws={ws}
        wid={wid}
        token={editTarget}
        onOpenChange={(o) => {
          if (!o) setEditTarget(null);
        }}
      />

      <RotateTokenDialog
        ws={ws}
        wid={wid}
        token={rotateTarget}
        onOpenChange={(o) => {
          if (!o) setRotateTarget(null);
        }}
      />

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

/** Shared editable fields for the mint + edit dialogs. */
interface TokenFormState {
  label: string;
  read: boolean;
  write: boolean;
  namespaces: string;
  tables: string;
}

function TokenFormFields({
  state,
  onChange,
}: {
  state: TokenFormState;
  onChange: (next: TokenFormState) => void;
}) {
  return (
    <>
      <div className="space-y-1.5">
        <span className="text-sm font-medium">Label</span>
        <Input
          value={state.label}
          onChange={(e) => onChange({ ...state, label: e.target.value })}
          placeholder="e.g. dbt-ci"
          autoFocus
        />
      </div>

      <div className="space-y-1.5">
        <span className="text-sm font-medium">Permissions</span>
        <div className="flex gap-2">
          <PermissionToggle
            label="Read"
            checked={state.read}
            onChange={(read) => onChange({ ...state, read })}
          />
          <PermissionToggle
            label="Write"
            checked={state.write}
            onChange={(write) => onChange({ ...state, write })}
          />
        </div>
        {!state.read && !state.write && (
          <p className="text-xs text-destructive">Select at least one permission.</p>
        )}
      </div>

      <div className="space-y-1.5">
        <span className="text-sm font-medium">Namespaces</span>
        <Input
          value={state.namespaces}
          onChange={(e) => onChange({ ...state, namespaces: e.target.value })}
          placeholder="marts, staging — leave blank for all"
        />
      </div>

      <div className="space-y-1.5">
        <span className="text-sm font-medium">Tables</span>
        <textarea
          value={state.tables}
          onChange={(e) => onChange({ ...state, tables: e.target.value })}
          placeholder="sales.orders, marts.daily — leave blank for all tables"
          rows={3}
          className="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        />
        <p className="text-xs text-muted-foreground">
          Dotted namespace.table per line or comma-separated. Leave blank for all tables.
        </p>
      </div>
    </>
  );
}

const emptyForm: TokenFormState = {
  label: "",
  read: true,
  write: false,
  namespaces: "",
  tables: "",
};

/** Reusable one-time secret reveal (mint + rotate share this). */
function RevealSecret({ raw, onDone }: { raw: string; onDone: () => void }) {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(raw);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard may be unavailable; the token is still selectable manually.
    }
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>Token secret</DialogTitle>
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
        <Button type="button" onClick={onDone}>
          Done
        </Button>
      </DialogFooter>
    </>
  );
}

type MintPhase = "form" | "reveal";

function MintTokenDialog({
  ws,
  wid,
  open,
  onOpenChange,
}: {
  ws: string;
  wid: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const mintMutation = useMintToken(ws, wid);

  const [phase, setPhase] = useState<MintPhase>("form");
  const [form, setForm] = useState<TokenFormState>(emptyForm);
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setPhase("form");
    setForm(emptyForm);
    setRaw(null);
    setError(null);
    mintMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit =
    form.label.trim().length > 0 && (form.read || form.write) && !mintMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);

    const table_scope = parseScope(form.tables);
    const tableErr = invalidTableScope(table_scope);
    if (tableErr) {
      setError(tableErr);
      return;
    }

    const permissions = [
      ...(form.read ? ["read"] : []),
      ...(form.write ? ["write"] : []),
    ];

    const req: MintTokenRequest = {
      label: form.label.trim(),
      permissions,
      namespace_scope: parseScope(form.namespaces),
      table_scope,
    };

    try {
      const res = await mintMutation.mutateAsync(req);
      setRaw(res.raw);
      setPhase("reveal");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to mint token.");
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
              <TokenFormFields state={form} onChange={setForm} />

              {error && (
                <Alert variant="destructive">
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
            </div>

            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!canSubmit}>
                {mintMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
                Mint token
              </Button>
            </DialogFooter>
          </form>
        ) : (
          raw && <RevealSecret raw={raw} onDone={() => handleOpenChange(false)} />
        )}
      </DialogContent>
    </Dialog>
  );
}

function EditTokenDialog({
  ws,
  wid,
  token,
  onOpenChange,
}: {
  ws: string;
  wid: string;
  token: CatalogTokenDto | null;
  onOpenChange: (open: boolean) => void;
}) {
  const updateMutation = useUpdateToken(ws, wid);

  const [form, setForm] = useState<TokenFormState>(emptyForm);
  const [error, setError] = useState<string | null>(null);
  // Re-seed the form whenever a new token target is opened.
  const [seededId, setSeededId] = useState<string | null>(null);
  if (token && token.id !== seededId) {
    setSeededId(token.id);
    setForm({
      label: token.label,
      read: token.permissions.includes("catalog:read"),
      write: token.permissions.includes("catalog:write"),
      namespaces: token.namespace_scope.join("\n"),
      tables: token.table_scope.join("\n"),
    });
    setError(null);
    updateMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) {
      setSeededId(null);
      setError(null);
      updateMutation.reset();
    }
    onOpenChange(next);
  }

  const canSubmit =
    form.label.trim().length > 0 && (form.read || form.write) && !updateMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!token || !canSubmit) return;
    setError(null);

    const table_scope = parseScope(form.tables);
    const tableErr = invalidTableScope(table_scope);
    if (tableErr) {
      setError(tableErr);
      return;
    }

    const permissions = [
      ...(form.read ? ["read"] : []),
      ...(form.write ? ["write"] : []),
    ];

    const body: UpdateTokenRequest = {
      label: form.label.trim(),
      permissions,
      namespace_scope: parseScope(form.namespaces),
      table_scope,
    };

    try {
      await updateMutation.mutateAsync({ id: token.id, body });
      handleOpenChange(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to update token.");
    }
  }

  return (
    <Dialog open={token !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Edit catalog token</DialogTitle>
            <DialogDescription>
              Update the token&apos;s label, permissions, and scope. The existing secret keeps
              working.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <TokenFormFields state={form} onChange={setForm} />

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {updateMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
              Save changes
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

type RotatePhase = "confirm" | "reveal";

function RotateTokenDialog({
  ws,
  wid,
  token,
  onOpenChange,
}: {
  ws: string;
  wid: string;
  token: CatalogTokenDto | null;
  onOpenChange: (open: boolean) => void;
}) {
  const rotateMutation = useRotateToken(ws, wid);

  const [phase, setPhase] = useState<RotatePhase>("confirm");
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  function handleOpenChange(next: boolean) {
    if (!next) {
      setPhase("confirm");
      setRaw(null);
      setError(null);
      rotateMutation.reset();
    }
    onOpenChange(next);
  }

  async function handleRotate() {
    if (!token) return;
    setError(null);
    try {
      const res = await rotateMutation.mutateAsync(token.id);
      setRaw(res.raw);
      setPhase("reveal");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to rotate token.");
    }
  }

  return (
    <Dialog open={token !== null} onOpenChange={handleOpenChange}>
      <DialogContent>
        {phase === "confirm" ? (
          <>
            <DialogHeader>
              <DialogTitle>
                {token ? `Rotate token «${token.label}»?` : "Rotate token?"}
              </DialogTitle>
              <DialogDescription>
                A new secret is generated and the current one stops working immediately. The new
                secret is shown only once.
              </DialogDescription>
            </DialogHeader>

            {error && (
              <Alert variant="destructive" className="my-2">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
                Cancel
              </Button>
              <Button type="button" onClick={handleRotate} disabled={rotateMutation.isPending}>
                {rotateMutation.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
                Rotate token
              </Button>
            </DialogFooter>
          </>
        ) : (
          raw && <RevealSecret raw={raw} onDone={() => handleOpenChange(false)} />
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
