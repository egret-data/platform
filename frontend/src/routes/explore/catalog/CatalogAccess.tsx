import { useState } from "react";
import { Check, Copy, KeyRound, Loader2, Pencil, Plus, RotateCw, Trash2, X } from "lucide-react";

import {
  type CatalogTokenDto,
  type Grant,
  type MintTokenRequest,
  type UpdateTokenRequest,
  nsDotted,
  useCatalogNamespaces,
  useCatalogTokens,
  useMintToken,
  useNamespaceContents,
  useRevokeToken,
  useRotateToken,
  useUpdateToken,
} from "@/api/catalog";
import { ApiError } from "@/api/client";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/** All-namespaces sentinel. */
const ALL_NS = "*";
/** Select value used for the "All tables" option (Radix can't carry null). */
const ALL_TABLES = "__all_tables__";

/** A grant whose perms are the short ("read"/"write") API form. */
type GrantState = Grant;

/** Human summary of a single grant, e.g. `sale (read, write)` or `marketing.campaigns (read)`. */
function grantSummary(g: Grant): string {
  const target = g.table ? `${g.namespace}.${g.table}` : g.namespace;
  return `${target} (${g.permissions.join(", ")})`;
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
                <th className="py-2 pr-4 font-medium">Scope</th>
                <th className="py-2 pr-4 font-medium">Created</th>
                <th className="py-2 font-medium sr-only">Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((token) => (
                <tr key={token.id} className="border-b border-border last:border-b-0">
                  <td className="py-2 pr-4 font-medium align-top">{token.label}</td>
                  <td className="py-2 pr-4 align-top">
                    <div className="flex flex-col gap-0.5 font-mono text-xs text-muted-foreground">
                      {token.scope.map((g, i) => (
                        <span key={i}>{grantSummary(g)}</span>
                      ))}
                    </div>
                  </td>
                  <td className="py-2 pr-4 text-muted-foreground align-top">
                    <RelativeTime iso={token.created_at} />
                  </td>
                  <td className="py-2 text-right align-top">
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

// ── Grant builder ────────────────────────────────────────────────────────────

function defaultGrant(firstNamespace: string | undefined): GrantState {
  return { namespace: firstNamespace ?? ALL_NS, table: null, permissions: ["read"] };
}

/** Validate the builder rows; returns an error string or null. */
function validateGrants(grants: GrantState[]): string | null {
  if (grants.length === 0) {
    return "Add at least one grant (resource) to the scope.";
  }
  for (const g of grants) {
    if (g.permissions.length === 0) {
      return "Each resource needs at least one permission (read or write).";
    }
  }
  return null;
}

function GrantRow({
  ws,
  wid,
  namespaces,
  grant,
  onChange,
  onRemove,
}: {
  ws: string;
  wid: string;
  namespaces: string[];
  grant: GrantState;
  onChange: (next: GrantState) => void;
  onRemove: () => void;
}) {
  const isAllNs = grant.namespace === ALL_NS;
  // Only fetch tables for a concrete namespace.
  const contents = useNamespaceContents(ws, wid, isAllNs ? null : grant.namespace);
  const tables = contents.data?.tables ?? [];

  function handleNamespace(next: string) {
    if (next === ALL_NS) {
      // "*" forces all-tables (table grants need a concrete namespace).
      onChange({ ...grant, namespace: ALL_NS, table: null });
    } else {
      onChange({ ...grant, namespace: next, table: null });
    }
  }

  function handleTable(next: string) {
    onChange({ ...grant, table: next === ALL_TABLES ? null : next });
  }

  function togglePerm(perm: "read" | "write", on: boolean) {
    const set = new Set(grant.permissions);
    if (on) set.add(perm);
    else set.delete(perm);
    // Keep a stable read-before-write order.
    const ordered = (["read", "write"] as const).filter((p) => set.has(p));
    onChange({ ...grant, permissions: ordered });
  }

  return (
    <div
      data-testid="grant-row"
      className="flex flex-wrap items-end gap-2 rounded-md border border-border p-2"
    >
      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">Namespace</span>
        <Select value={grant.namespace} onValueChange={handleNamespace}>
          <SelectTrigger aria-label="Namespace" className="w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL_NS}>All namespaces (*)</SelectItem>
            {namespaces.map((ns) => (
              <SelectItem key={ns} value={ns}>
                {ns}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">Table</span>
        <Select
          value={grant.table ?? ALL_TABLES}
          onValueChange={handleTable}
          disabled={isAllNs}
        >
          <SelectTrigger aria-label="Table" className="w-40" disabled={isAllNs}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL_TABLES}>All tables</SelectItem>
            {tables.map((t) => (
              <SelectItem key={t} value={t}>
                {t}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="space-y-1">
        <span className="text-xs text-muted-foreground">Permissions</span>
        <div className="flex gap-2">
          <PermissionToggle
            label="Read"
            checked={grant.permissions.includes("read")}
            onChange={(on) => togglePerm("read", on)}
          />
          <PermissionToggle
            label="Write"
            checked={grant.permissions.includes("write")}
            onChange={(on) => togglePerm("write", on)}
          />
        </div>
      </div>

      <Button
        type="button"
        variant="ghost"
        size="sm"
        aria-label="Remove resource"
        className="ml-auto text-muted-foreground hover:text-destructive"
        onClick={onRemove}
      >
        <X className="h-4 w-4" />
      </Button>
    </div>
  );
}

function GrantBuilder({
  ws,
  wid,
  namespaces,
  grants,
  onChange,
}: {
  ws: string;
  wid: string;
  namespaces: string[];
  grants: GrantState[];
  onChange: (next: GrantState[]) => void;
}) {
  function update(idx: number, next: GrantState) {
    onChange(grants.map((g, i) => (i === idx ? next : g)));
  }
  function remove(idx: number) {
    onChange(grants.filter((_, i) => i !== idx));
  }
  function add() {
    onChange([...grants, defaultGrant(namespaces[0])]);
  }

  return (
    <div className="space-y-2">
      <span className="text-sm font-medium">Scope</span>
      <div className="space-y-2">
        {grants.map((g, i) => (
          <GrantRow
            key={i}
            ws={ws}
            wid={wid}
            namespaces={namespaces}
            grant={g}
            onChange={(next) => update(i, next)}
            onRemove={() => remove(i)}
          />
        ))}
      </div>
      <Button type="button" variant="outline" size="sm" onClick={add}>
        <Plus className="h-4 w-4" />
        Add resource
      </Button>
    </div>
  );
}

// ── Shared dialog form ───────────────────────────────────────────────────────

interface TokenFormState {
  label: string;
  grants: GrantState[];
}

function TokenFormFields({
  ws,
  wid,
  namespaces,
  state,
  onChange,
}: {
  ws: string;
  wid: string;
  namespaces: string[];
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

      <GrantBuilder
        ws={ws}
        wid={wid}
        namespaces={namespaces}
        grants={state.grants}
        onChange={(grants) => onChange({ ...state, grants })}
      />
    </>
  );
}

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
  const nsQuery = useCatalogNamespaces(ws, wid);
  const namespaces = (nsQuery.data ?? []).map((n) => nsDotted(n.levels));

  const [phase, setPhase] = useState<MintPhase>("form");
  const [form, setForm] = useState<TokenFormState>(() => ({ label: "", grants: [defaultGrant(undefined)] }));
  const [seeded, setSeeded] = useState(false);
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Seed the default grant's namespace from the first real namespace once loaded.
  if (open && !seeded && namespaces.length > 0) {
    setSeeded(true);
    setForm((f) =>
      f.grants.length === 1 && f.grants[0].namespace === ALL_NS && f.grants[0].table === null
        ? { ...f, grants: [defaultGrant(namespaces[0])] }
        : f,
    );
  }

  function reset() {
    setPhase("form");
    setForm({ label: "", grants: [defaultGrant(undefined)] });
    setSeeded(false);
    setRaw(null);
    setError(null);
    mintMutation.reset();
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  const canSubmit = form.label.trim().length > 0 && !mintMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);

    const grantErr = validateGrants(form.grants);
    if (grantErr) {
      setError(grantErr);
      return;
    }

    const req: MintTokenRequest = { label: form.label.trim(), scope: form.grants };

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
              <TokenFormFields
                ws={ws}
                wid={wid}
                namespaces={namespaces}
                state={form}
                onChange={setForm}
              />

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
  const nsQuery = useCatalogNamespaces(ws, wid);
  const namespaces = (nsQuery.data ?? []).map((n) => nsDotted(n.levels));

  const [form, setForm] = useState<TokenFormState>({ label: "", grants: [] });
  const [error, setError] = useState<string | null>(null);
  // Re-seed the form whenever a new token target is opened.
  const [seededId, setSeededId] = useState<string | null>(null);
  if (token && token.id !== seededId) {
    setSeededId(token.id);
    setForm({
      label: token.label,
      // Deep-copy the grants so edits don't mutate the cached token.
      grants: token.scope.map((g) => ({
        namespace: g.namespace,
        table: g.table,
        permissions: [...g.permissions],
      })),
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

  const canSubmit = form.label.trim().length > 0 && !updateMutation.isPending;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!token || !canSubmit) return;
    setError(null);

    const grantErr = validateGrants(form.grants);
    if (grantErr) {
      setError(grantErr);
      return;
    }

    const body: UpdateTokenRequest = { label: form.label.trim(), scope: form.grants };

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
              Update the token&apos;s label and resource scope. The existing secret keeps working.
            </DialogDescription>
          </DialogHeader>

          <div className="my-4 space-y-4">
            <TokenFormFields
              ws={ws}
              wid={wid}
              namespaces={namespaces}
              state={form}
              onChange={setForm}
            />

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
