import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  getPermListRulesQueryKey,
  usePermCreateRule,
  usePermPatchRule,
  usePermPreview,
} from "@/api/generated/perm/perm";
import type { NewRule, PreviewResponse, PrincipalSet, Rule, RuleKind, Scope, Selector } from "@/api/generated/model";
import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import { ApiError } from "@/api/client";
import { type Tag, useTags } from "@/api/tags";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { formatScope } from "./permFormat";

/**
 * D7: `mask_type` is a fixed list in the UI even though the API takes a free
 * string — Ranger's own vocabulary (measured against
 * `crates/egret-policy/src/service-defs/trino.json`'s `maskTypes`), with
 * `CUSTOM` revealing the expression field. Hand-maintained mirror: nothing
 * derives this from the API, because the API exposes no enumeration.
 */
const MASK_TYPES: { value: string; label: string }[] = [
  { value: "MASK", label: "Redact" },
  { value: "MASK_SHOW_LAST_4", label: "Partial mask: show last 4" },
  { value: "MASK_SHOW_FIRST_4", label: "Partial mask: show first 4" },
  { value: "MASK_HASH", label: "Hash" },
  { value: "MASK_NULL", label: "Nullify" },
  { value: "MASK_NONE", label: "Unmasked (retain original value)" },
  { value: "MASK_DATE_SHOW_YEAR", label: "Date: show only year" },
  { value: "CUSTOM", label: "Custom" },
];

const EMPTY_SCOPE: Scope = { catalog_id: "" };
const EMPTY_EXEMPT: PrincipalSet = { include: [], exclude: [] };

type KindType = RuleKind["type"];
type SelectorType = Selector["type"];

/**
 * A tag selector for a rule. **The model selects by `key`, while
 * Classifications browses tags by `name` and `id`** — this bridges the two:
 * the dropdown shows `name`, the click stores `key`. Storing a name would
 * compile and then match nothing, since `Selector`'s `tag` variant is
 * `{key, type: "tag"}`, not a name.
 */
function TagKeySelector({
  value,
  onChange,
  disabled,
}: {
  value: string;
  onChange: (key: string) => void;
  disabled?: boolean;
}) {
  const tags = useTags();
  const rows: Tag[] = tags.data ?? [];
  const selected = rows.find((t) => t.key === value);
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const filtered = rows.filter((t) => t.name.toLowerCase().includes(q.toLowerCase()));

  return (
    <div className="space-y-1.5">
      <Label htmlFor="rule-tag-search">Tag</Label>
      {selected ? (
        <div className="flex items-center gap-2">
          <Badge variant="secondary">{selected.name}</Badge>
          <span className="font-mono text-xs text-muted-foreground">stored as {selected.key}</span>
          {!disabled && (
            <Button type="button" variant="ghost" size="sm" onClick={() => onChange("")}>
              Change
            </Button>
          )}
        </div>
      ) : (
        !disabled && (
          <div className="relative">
            <Input
              id="rule-tag-search"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onFocus={() => setOpen(true)}
              onBlur={() => setTimeout(() => setOpen(false), 150)}
              placeholder="Search tags by name…"
            />
            {open && (
              <div className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1 shadow-md">
                {filtered.map((t) => (
                  <button
                    key={t.id}
                    type="button"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      onChange(t.key);
                      setQ("");
                      setOpen(false);
                    }}
                    className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                  >
                    <span>{t.name}</span>
                    <span className="shrink-0 text-xs text-muted-foreground">stored as {t.key}</span>
                  </button>
                ))}
                {filtered.length === 0 && (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">
                    {tags.isFetching ? "Loading…" : "No matches."}
                  </div>
                )}
              </div>
            )}
          </div>
        )
      )}
    </div>
  );
}

function kindOf(kindType: KindType, maskType: string, valueExpr: string, filterExpr: string): RuleKind {
  if (kindType === "mask") {
    return {
      type: "mask",
      mask_type: maskType,
      value_expr: maskType === "CUSTOM" ? valueExpr : undefined,
    };
  }
  return { type: "row_filter", filter_expr: filterExpr };
}

function selectorOf(selectorType: SelectorType, tagKey: string, scope: Scope): Selector {
  if (selectorType === "tag") return { type: "tag", key: tagKey };
  return { ...scope, type: "path" };
}

/**
 * Create or edit a `Rule`. Mirrors `GrantDialog`: full-body `PATCH`, a preview
 * gate before save, refusals surfaced verbatim.
 *
 * D6's three unofferable combinations are enforced here by construction, not
 * by validating after the fact: `exempt` uses `PrincipalPicker` in its
 * `exempt` mode (no `exclude`, no `role`), and a path selector under
 * `row_filter` uses `ScopePicker`'s `rowFilterMode` (no column field at all,
 * catalog+schema+table jointly required).
 *
 * `initialSelector` and `initialKind` seed a NEW rule's selector and kind (the
 * Data object page's Add rule passes a `path` selector naming its object, and
 * on a table a row filter, which that selector completes); `existing` wins.
 */
export function RuleDialog({
  open,
  onOpenChange,
  existing,
  initialSelector,
  initialKind,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  existing?: Rule | null;
  initialSelector?: Selector;
  initialKind?: KindType;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] max-w-2xl overflow-y-auto">
        {open && <RuleForm existing={existing} initialSelector={initialSelector} initialKind={initialKind} onOpenChange={onOpenChange} />}
      </DialogContent>
    </Dialog>
  );
}

/**
 * The form itself — mounted only while the dialog is `open` (see
 * `RuleDialog` above), so every field below is freshly seeded from
 * `existing` via a lazy initializer rather than an effect that would have to
 * "reset" state a remount never let accumulate in the first place.
 */
function RuleForm({
  existing,
  initialSelector,
  initialKind,
  onOpenChange,
}: {
  existing?: Rule | null;
  initialSelector?: Selector;
  initialKind?: KindType;
  onOpenChange: (open: boolean) => void;
}) {
  const seed = existing ? existing.selector : initialSelector;
  // Read here rather than passed down — see the same note in `GrantDialog`
  // (review M-6).
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const catalogName = (id: string) => catalogs.find((c) => c.id === id)?.name ?? id;

  const [name, setName] = useState(existing?.name ?? "");
  const [kindType, setKindType] = useState<KindType>(existing?.kind.type ?? initialKind ?? "mask");
  const [maskType, setMaskType] = useState(
    existing?.kind.type === "mask" ? existing.kind.mask_type : "MASK",
  );
  const [valueExpr, setValueExpr] = useState(
    existing?.kind.type === "mask" ? (existing.kind.value_expr ?? "") : "",
  );
  const [filterExpr, setFilterExpr] = useState(
    existing?.kind.type === "row_filter" ? existing.kind.filter_expr : "",
  );
  const [selectorType, setSelectorType] = useState<SelectorType>(seed?.type ?? "tag");
  const [tagKey, setTagKey] = useState(seed?.type === "tag" ? seed.key : "");
  const [scope, setScope] = useState<Scope>(() => {
    if (seed?.type === "path") {
      const { catalog_id, schema, table, column } = seed;
      return { catalog_id, schema, table, column };
    }
    return EMPTY_SCOPE;
  });
  const [exempt, setExempt] = useState<PrincipalSet>(existing?.exempt ?? EMPTY_EXEMPT);

  const [preview, setPreview] = useState<PreviewResponse | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewedFor, setPreviewedFor] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);

  const queryClient = useQueryClient();
  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });

  const previewMutation = usePermPreview<ApiError>();
  const createRule = usePermCreateRule<ApiError>({ mutation: { onSuccess: invalidate } });
  const patchRule = usePermPatchRule<ApiError>({ mutation: { onSuccess: invalidate } });

  const kind = kindOf(kindType, maskType, valueExpr, filterExpr);
  const selector = selectorOf(selectorType, tagKey, scope);
  const body: NewRule = { name, kind, selector, exempt };
  const bodySnapshot = JSON.stringify({ id: existing?.id ?? null, body });

  // D6's row-filter-by-path constraint, restated as a Save gate: `ScopePicker`
  // in `rowFilterMode` never renders a column field, but this dialog uses the
  // picker without `onApply` (continuously controlled), so it must check the
  // same "jointly required, no column" condition itself before allowing Save.
  const rowFilterPathIncomplete =
    kindType === "row_filter" &&
    selectorType === "path" &&
    !(scope.catalog_id && scope.schema && scope.table && !scope.column);

  const blockedReason = !name.trim()
    ? "Name the rule."
    : kindType === "mask" && maskType === "CUSTOM" && !valueExpr.trim()
      ? "A custom mask needs an expression."
      : kindType === "row_filter" && !filterExpr.trim()
        ? "A row filter needs an expression."
        : selectorType === "tag" && !tagKey
          ? "Choose a tag."
          : selectorType === "path" && !scope.catalog_id
            ? "Choose a catalog."
            : rowFilterPathIncomplete
              ? "A path-selected row filter needs exactly a catalog, schema and table — no column."
              : null;

  /**
   * Switching Kind to `row_filter` must drop any `scope.column` (review M-4).
   * `ScopePicker` in `rowFilterMode` does not render a Column field at all
   * (hardcoded fact 4/5: Ranger's `rowFilterDef` has no column level), so a
   * column typed while the kind was `mask` survives invisibly and
   * `rowFilterPathIncomplete` then blocks Save with a reason naming a field
   * the operator cannot see or clear. The value is dropped on the switch,
   * where the constraint starts applying — not silently at submit, which
   * would send something other than what the form last showed.
   */
  function changeKind(next: KindType) {
    setKindType(next);
    if (next === "row_filter" && scope.column) {
      setScope({ ...scope, column: undefined });
    }
  }

  const previewStale = previewedFor !== bodySnapshot;

  async function runPreview() {
    setPreviewError(null);
    setPreview(null);
    try {
      const result = await previewMutation.mutateAsync({
        data: { id: existing?.id ?? null, rule: body },
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
        await patchRule.mutateAsync({ id: existing.id, data: body });
      } else {
        await createRule.mutateAsync({ data: body });
      }
      toast.success(existing ? "Rule updated." : "Rule created.");
      onOpenChange(false);
    } catch (e) {
      setSaveError((e as ApiError).message || "Failed to save the rule.");
    }
  }

  const saving = createRule.isPending || patchRule.isPending;
  const canSave = !blockedReason && !previewStale && !!preview && !previewError;

  return (
    <>
      <DialogHeader>
        <DialogTitle>{existing ? "Edit rule" : "New rule"}</DialogTitle>
        <DialogDescription>
          A named rule that masks a column or filters rows wherever a classification or a path
          matches.
        </DialogDescription>
        </DialogHeader>

        <div className="space-y-5">
          {saveError && (
            <Alert variant="destructive">
              <AlertDescription>{saveError}</AlertDescription>
            </Alert>
          )}

          <div className="space-y-1.5">
            <Label htmlFor="rule-name">Name</Label>
            <Input id="rule-name" value={name} onChange={(e) => setName(e.target.value)} autoFocus />
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="rule-kind">Kind</Label>
            <Select value={kindType} onValueChange={(v) => changeKind(v as KindType)}>
              <SelectTrigger id="rule-kind">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="mask">Mask a column</SelectItem>
                <SelectItem value="row_filter">Filter rows</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {kindType === "mask" ? (
            <div className="space-y-3">
              <div className="space-y-1.5">
                <Label htmlFor="rule-mask-type">Mask type</Label>
                <Select value={maskType} onValueChange={setMaskType}>
                  <SelectTrigger id="rule-mask-type">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {MASK_TYPES.map((m) => (
                      <SelectItem key={m.value} value={m.value}>
                        {m.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              {maskType === "CUSTOM" && (
                <div className="space-y-1.5">
                  <Label htmlFor="rule-value-expr">Custom expression</Label>
                  <Textarea
                    id="rule-value-expr"
                    rows={2}
                    value={valueExpr}
                    onChange={(e) => setValueExpr(e.target.value)}
                    placeholder="e.g. cast(NULL as {type})"
                  />
                </div>
              )}
            </div>
          ) : (
            <div className="space-y-1.5">
              <Label htmlFor="rule-filter-expr">Filter expression</Label>
              <Textarea
                id="rule-filter-expr"
                rows={2}
                value={filterExpr}
                onChange={(e) => setFilterExpr(e.target.value)}
                placeholder="e.g. region = 'us'"
              />
            </div>
          )}

          <div className="space-y-1.5">
            <Label htmlFor="rule-selector-type">Selects by</Label>
            <Select value={selectorType} onValueChange={(v) => setSelectorType(v as SelectorType)}>
              <SelectTrigger id="rule-selector-type">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="tag">Tag</SelectItem>
                <SelectItem value="path">Path</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {selectorType === "tag" ? (
            <TagKeySelector value={tagKey} onChange={setTagKey} />
          ) : (
            <div className="space-y-1.5">
              <Label>Scope</Label>
              <ScopePicker value={scope} onChange={setScope} rowFilterMode={kindType === "row_filter"} />
              {scope.catalog_id && (
                <p className="text-xs text-muted-foreground">
                  Selects {formatScope(scope, catalogName(scope.catalog_id))}
                </p>
              )}
            </div>
          )}

          <div className="space-y-1.5">
            <Label>Exempt</Label>
            <PrincipalPicker value={exempt} onChange={setExempt} exempt />
          </div>

          {blockedReason && (
            <Alert>
              <AlertDescription>{blockedReason}</AlertDescription>
            </Alert>
          )}

          <div className="space-y-2 rounded-md border border-border bg-muted/30 p-3">
            <div className="flex items-center justify-between gap-2">
              <p className="text-sm font-medium">Preview this write</p>
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
                Run a preview before saving. A tag rule is catalog-unscoped, so this can recompile
                every Trino profile in the installation — see which, here, before committing.
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
