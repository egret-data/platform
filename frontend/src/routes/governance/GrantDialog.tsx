import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  getPermListGrantsQueryKey,
  usePermCreateGrant,
  usePermPatchGrant,
  usePermPreview,
} from "@/api/generated/perm/perm";
import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import type { Action, Effect, Grant, NewGrant, PreviewResponse, PrincipalSet, Scope } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { formatActions, formatScope } from "./permFormat";

const EMPTY_SCOPE: Scope = { catalog_id: "" };
const EMPTY_PRINCIPALS: PrincipalSet = { include: [], exclude: [] };

function bodyOf(effect: Effect, actions: Action[], scope: Scope, principals: PrincipalSet): NewGrant {
  return { effect, actions, scope, principals };
}

/**
 * Create or edit a `Grant`. Thin wrapper: the actual form (`GrantForm`) is
 * mounted only while `open`, so every field it holds is freshly seeded from
 * `existing` on each open — including reopening the SAME grant after a
 * cancelled edit — with no effect needed to "reset" stale state, because
 * there is none to reset.
 *
 * Only ever opened on an `explicit` grant (or none, for create): a
 * `workspace_assignment` grant is not editable through this surface — the
 * list page enforces that by never handing one to `existing`.
 */
export function GrantDialog({
  open,
  onOpenChange,
  existing,
  initialScope,
  initialPrincipals,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing?: Grant | null;
  /** Where a NEW grant starts (the object page's Grant access); ignored when `existing` is set. */
  initialScope?: Scope;
  /** Who a NEW grant starts with (a People page's Grant access); ignored when `existing` is set. */
  initialPrincipals?: PrincipalSet;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        {open && (
          <GrantForm
            existing={existing}
            initialScope={initialScope}
            initialPrincipals={initialPrincipals}
            onOpenChange={onOpenChange}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function GrantForm({
  existing,
  initialScope,
  initialPrincipals,
  onOpenChange,
}: {
  existing?: Grant | null;
  initialScope?: Scope;
  initialPrincipals?: PrincipalSet;
  onOpenChange: (open: boolean) => void;
}) {
  // The catalog list is read HERE, not passed down from the list page
  // (review M-6): only this dialog needs it — the list's sentence comes off
  // `GrantView.catalog_name`. It is the Governance list (`policy:read` or
  // `tag:read`), not the registry's (`catalog_registry:read`), so a steward
  // can name a catalog. `ScopePicker` below issues the same query, which
  // react-query de-duplicates by key, so this costs no extra request.
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const catalogName = (id: string) => catalogs.find((c) => c.id === id)?.name ?? id;

  const [effect, setEffect] = useState<Effect>(existing?.effect ?? "allow");
  const [actions, setActions] = useState<Action[]>(existing?.actions ?? ["read"]);
  const [scope, setScope] = useState<Scope>(existing ? existing.scope : (initialScope ?? EMPTY_SCOPE));
  const [principals, setPrincipals] = useState<PrincipalSet>(
    existing ? existing.principals : (initialPrincipals ?? EMPTY_PRINCIPALS),
  );

  const [preview, setPreview] = useState<PreviewResponse | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewedFor, setPreviewedFor] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  const queryClient = useQueryClient();
  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: getPermListGrantsQueryKey() });

  const previewMutation = usePermPreview<ApiError>();
  const createGrant = usePermCreateGrant<ApiError>({ mutation: { onSuccess: invalidate } });
  const patchGrant = usePermPatchGrant<ApiError>({ mutation: { onSuccess: invalidate } });

  const body = bodyOf(effect, actions, scope, principals);
  const bodySnapshot = JSON.stringify({ id: existing?.id ?? null, body });

  // D1's own guard restated client-side, per the brief: the server's 400 on
  // an empty `principals.include` exists to stop a PATCH from silently
  // revoking a grant, but that 400 must not be the ONLY thing standing
  // between an operator and that outcome — the form refuses to offer
  // Preview/Save at all rather than let someone reach the 400 by surprise.
  const includeEmpty = principals.include.length === 0;
  const noActions = actions.length === 0;
  const noCatalog = !scope.catalog_id;
  const blockedReason = includeEmpty
    ? "Include at least one principal — a grant that includes nobody would compile to nothing."
    : noActions
      ? "Choose at least one action."
      : noCatalog
        ? "Choose a catalog."
        : null;

  const previewStale = previewedFor !== bodySnapshot;

  // KI-132: the dialog is taller than a 900 px viewport, so its body scrolls and
  // the preview panel sits below the fold. Once an answer (result or error)
  // arrives, bring the panel into view: the reader pressed Preview to read it.
  // `start`, not `nearest`: the footer (Create) follows the panel, and the body
  // scrolls as far as it can, so the result and the button both end up on screen.
  const previewPanelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (preview || previewError) previewPanelRef.current?.scrollIntoView({ block: "start" });
  }, [preview, previewError]);

  function toggleAction(a: Action, checked: boolean) {
    setActions((prev) => (checked ? [...prev, a] : prev.filter((x) => x !== a)));
  }

  async function runPreview() {
    setPreviewError(null);
    setPreview(null);
    try {
      const result = await previewMutation.mutateAsync({
        data: { id: existing?.id ?? null, grant: body },
      });
      const data = result.status === 200 ? result.data : undefined;
      if (!data) {
        // A non-200 that did not throw (the mutator resolves whatever the
        // envelope carries) left `preview` null and no error, so pressing
        // Preview looked like nothing happened — it failed CLOSED, since Save
        // stays blocked without a preview, but silence is the failure mode this
        // branch keeps fixing elsewhere (review M-7). Say the status.
        setPreviewError(`Preview failed (HTTP ${result.status}).`);
      }
      setPreview(data ?? null);
      setPreviewedFor(bodySnapshot);
    } catch (e) {
      setPreviewError((e as ApiError).message || "Preview failed.");
      setPreviewedFor(bodySnapshot);
    }
  }

  async function save() {
    setSaveError(null);
    try {
      if (existing) {
        await patchGrant.mutateAsync({ id: existing.id, data: body });
      } else {
        await createGrant.mutateAsync({ data: body });
      }
      toast.success(existing ? "Grant updated." : "Grant created.");
      onOpenChange(false);
    } catch (e) {
      // Surfaced verbatim (D8): the backend's message already names the
      // rule/profile/ceiling a generic "failed to save" would throw away.
      setSaveError((e as ApiError).message || "Failed to save the grant.");
    }
  }

  const saving = createGrant.isPending || patchGrant.isPending;
  const canSave = !blockedReason && !previewStale && !!preview && !previewError;

  return (
    <>
      <DialogHeader>
        <DialogTitle>{existing ? "Edit grant" : "New grant"}</DialogTitle>
        <DialogDescription>
          Who can do what with data — the effect, the actions, the scope and, if needed, who is
          excluded.
        </DialogDescription>
      </DialogHeader>

        <div className="space-y-5">
          {saveError && (
            <Alert variant="destructive">
              <AlertDescription>{saveError}</AlertDescription>
            </Alert>
          )}

          <div className="flex gap-4">
            <div className="flex-1 space-y-1.5">
              <Label htmlFor="grant-effect">Effect</Label>
              <Select value={effect} onValueChange={(v) => setEffect(v as Effect)}>
                <SelectTrigger id="grant-effect">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="allow">Allow</SelectItem>
                  <SelectItem value="deny">Deny</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex-1 space-y-1.5">
              <Label>Actions</Label>
              <div className="flex h-9 items-center gap-4">
                <label className="flex items-center gap-1.5 text-sm">
                  <Checkbox
                    checked={actions.includes("read")}
                    onCheckedChange={(v) => toggleAction("read", v === true)}
                  />
                  Read
                </label>
                <label className="flex items-center gap-1.5 text-sm">
                  <Checkbox
                    checked={actions.includes("write")}
                    onCheckedChange={(v) => toggleAction("write", v === true)}
                  />
                  Write
                </label>
              </div>
            </div>
          </div>

          <div className="space-y-1.5">
            <Label>Scope</Label>
            <ScopePicker value={scope} onChange={setScope} />
          </div>

          <div className="space-y-1.5">
            <Label>Principals</Label>
            <PrincipalPicker value={principals} onChange={setPrincipals} />
          </div>

          {blockedReason && (
            <Alert>
              <AlertDescription>{blockedReason}</AlertDescription>
            </Alert>
          )}

          <div ref={previewPanelRef} className="space-y-2 rounded-md border border-border bg-muted/30 p-3">
            <div className="flex items-center justify-between gap-2">
              <p className="text-sm font-medium">
                Preview: {formatActions(actions)} on {noCatalog ? "…" : formatScope(scope, catalogName(scope.catalog_id))}
              </p>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => void runPreview()}
                disabled={!!blockedReason || previewMutation.isPending}
              >
                {previewMutation.isPending ? "Checking…" : "Preview"}
              </Button>
            </div>

            {previewError && (
              <Alert variant="destructive">
                <AlertDescription>{previewError}</AlertDescription>
              </Alert>
            )}

            {preview && !previewStale && (
              <div className="space-y-1 text-sm">
                <p>
                  Would recompile{" "}
                  {preview.affected.length === 0
                    ? "no profiles"
                    : preview.affected.map((p) => p.name).join(", ")}
                  .
                </p>
                <p className="text-muted-foreground">
                  created {preview.compile.created} · updated {preview.compile.updated} · deleted{" "}
                  {preview.compile.deleted} · unchanged {preview.compile.unchanged}
                </p>
                {preview.compile.skipped.length > 0 && (
                  <ul className="list-disc space-y-0.5 pl-5 text-muted-foreground">
                    {/* The index is part of the key on purpose (SP4b-2
                        finding 4): `SkipReport` is `{source_kind, id, reason}`
                        with no profile field, so the SAME rule skipped in two
                        profiles produces two byte-identical entries — the
                        normal case for a tag rule, which is catalog-unscoped
                        and therefore the exact fan-out this panel exists to
                        show. Keying on `source_kind`+`id` collided there and
                        React dropped one of the two lines it was asked to
                        render. */}
                    {preview.compile.skipped.map((s, i) => (
                      <li key={`${s.source_kind}-${s.id}-${i}`}>
                        {s.source_kind} {s.id}: {s.reason}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}

            {!preview && !previewError && (
              <p className="text-xs text-muted-foreground">
                Run a preview before saving — a rule or grant's blast radius is also a failure
                radius.
              </p>
            )}
            {previewStale && (preview || previewError) && (
              <p className="text-xs text-muted-foreground">
                The form changed since that preview — run it again before saving.
              </p>
            )}
          </div>
        </div>

      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="button" onClick={() => void save()} disabled={!canSave || saving}>
          {saving ? "Saving…" : existing ? "Save changes" : "Create"}
        </Button>
      </DialogFooter>
    </>
  );
}

/** Small badge naming a grant's assignment source, for the list page. */
export function AssignmentBadge({ workspaceLabel }: { workspaceLabel: string }) {
  return (
    <Badge variant="secondary" className="whitespace-normal text-left font-normal">
      Set by workspace assignment · {workspaceLabel}
    </Badge>
  );
}
