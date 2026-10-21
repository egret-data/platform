import { useEffect, useId, useLayoutEffect, useRef, useState, type MutableRefObject } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import {
  getPermListRulesQueryKey,
  usePermCreateRule,
  usePermPatchRule,
  usePermPreview,
} from "@/api/generated/perm/perm";
import type { NewRule, PreviewResponse, PrincipalSet, RuleKind, Scope, Selector } from "@/api/generated/model";
import { useControlPlaneGovernanceListCatalogs } from "@/api/generated/control-plane-governance/control-plane-governance";
import { ApiError } from "@/api/client";
import { type Tag, useTags } from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useUnsavedChangesWarning } from "@/lib/useUnsavedChangesWarning";
import { formatScope } from "./permFormat";
import { MASK_TYPES, TERM } from "@/routes/governance/glossary";
import { useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { RemoveClassificationDialog } from "@/routes/governance/RemoveClassificationDialog";
import type { TableTagMaskConflict } from "@/routes/governance/apiErrors";
import { RequiredMark } from "@/routes/governance/tabParts";
import { invalidateRuleEnforcement } from "@/routes/governance/ruleEnforcementCache";
import {
  AlreadyMaskedNotice,
  EffectSummary,
  RecompileDetails,
  RefusalBody,
  showsNotEnforced,
  tickReason,
  type ConflictActions,
} from "@/routes/governance/RulePreview";
import {
  clearRuleDraft,
  entryScope,
  loadStoredRuleDraft,
  ruleDraftKey,
  saveRuleDraft,
  seedRule,
  type RuleEntry,
} from "@/routes/governance/ruleDraft";

/*
 * D7: `mask_type` is a fixed list in the UI even though the API takes a free
 * string — Ranger's own vocabulary, with `CUSTOM` revealing the expression
 * field. The list and its labels live in the glossary (`MASK_TYPES`).
 */

const EMPTY_SCOPE: Scope = { catalog_id: "" };

/** Create's reason when only the preview is missing or out of date (spec C8.2). */
export const PREVIEW_TO_ENABLE_CREATE = "Preview the rule to enable Create.";
/** The path-mask help (spec C8.1): an absent column is legal and means every column. */
export const EMPTY_COLUMN_MASKS_EVERY_COLUMN = "Leave Column empty to mask every column of the table.";

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
      <Label htmlFor="rule-tag-search">
        {TERM.classification}
        <RequiredMark />
      </Label>
      {selected ? (
        <div className="flex items-center gap-2">
          <Badge variant="secondary">{selected.name}</Badge>
          <span className="font-mono text-xs text-muted-foreground">{`${TERM.key} ${selected.key}`}</span>
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
              aria-required="true"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onFocus={() => setOpen(true)}
              onBlur={() => setTimeout(() => setOpen(false), 150)}
              placeholder="Search classifications by name…"
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
                    <span className="shrink-0 text-xs text-muted-foreground">{`${TERM.key} ${t.key}`}</span>
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

/** The form's fields, one per control. A `NewRule` is derived from them ({@link bodyOf}). */
type FormState = {
  name: string;
  kindType: KindType;
  maskType: string;
  valueExpr: string;
  filterExpr: string;
  selectorType: SelectorType;
  tagKey: string;
  scope: Scope;
  exempt: PrincipalSet;
};

/**
 * The form's fields for `rule`. A rule on a classification keeps `pathScope`
 * (the entry point's table or column) as its path, so switching Applies to =
 * Path lands on that path rather than an empty picker.
 */
function stateOf(rule: NewRule, pathScope: Scope | null = null): FormState {
  const { kind, selector } = rule;
  let scope = pathScope ?? EMPTY_SCOPE;
  if (selector.type === "path") {
    const { catalog_id, schema, table, column } = selector;
    scope = { catalog_id, schema, table, column };
  }
  return {
    name: rule.name,
    kindType: kind.type,
    maskType: kind.type === "mask" ? kind.mask_type : "MASK",
    valueExpr: kind.type === "mask" ? (kind.value_expr ?? "") : "",
    filterExpr: kind.type === "row_filter" ? kind.filter_expr : "",
    selectorType: selector.type,
    tagKey: selector.type === "tag" ? selector.key : "",
    scope,
    exempt: rule.exempt ?? { include: [], exclude: [] },
  };
}

function bodyOf(s: FormState): NewRule {
  const kind: RuleKind =
    s.kindType === "mask"
      ? { type: "mask", mask_type: s.maskType, value_expr: s.maskType === "CUSTOM" ? s.valueExpr : undefined }
      : { type: "row_filter", filter_expr: s.filterExpr };
  const selector: Selector = s.selectorType === "tag" ? { type: "tag", key: s.tagKey } : { ...s.scope, type: "path" };
  return { name: s.name, kind, selector, exempt: s.exempt };
}

/** A rule's content as a comparable string: the same derivation for the seed and the form. */
function snapshotOf(rule: NewRule): string {
  return JSON.stringify(bodyOf(stateOf(rule)));
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
 * **The entry contract (spec C8.3).** `entry` says where the dialog was opened
 * from, and that alone decides its defaults (`seedRule`) and its draft
 * (`ruleDraftKey`): a classification's page passes `classification`, a
 * table's Rules tab `table` (a catalog's or schema's, `scope`), the column
 * panel `column`, and a rule's page or an Edit action `edit`.
 *
 * **The draft (spec C8.4).** The form saves itself to the in-memory draft
 * store on every change, so closing the dialog by navigating away keeps it,
 * and reopening from the same entry point restores it. Cancel, Escape, the
 * close button and an outside click on a changed form ask "Discard this
 * rule?" first; Discard and a successful save forget the draft.
 */
export function RuleDialog({
  open,
  onOpenChange,
  entry,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  entry: RuleEntry;
}) {
  // The form decides whether a close needs asking (it knows whether it is dirty).
  const closeRequestRef = useRef<(() => void) | null>(null);
  // Start over remounts the form, so every field reseeds from the entry point.
  const [round, setRound] = useState(0);
  // What held focus when the dialog opened (Add rule, Edit), read before the
  // dialog's focus scope takes it. Callers open it from state, not a Radix
  // trigger, and after the nested "Discard this rule?" Radix had nowhere to
  // return focus and left it on <body> (live result §6).
  const opener = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!open) return;
    const active = document.activeElement;
    opener.current = active instanceof HTMLElement && active !== document.body ? active : null;
  }, [open]);
  const returnFocus = (e: Event) => {
    e.preventDefault();
    const target = opener.current?.isConnected ? opener.current : document.querySelector<HTMLElement>("h1");
    opener.current = null;
    target?.focus();
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) onOpenChange(true);
        else if (closeRequestRef.current) closeRequestRef.current();
        else onOpenChange(false);
      }}
    >
      {/* `scroll-pb-32`: a field focused near the end scrolls clear of the sticky footer. */}
      <DialogContent className="max-h-[85vh] max-w-2xl" bodyClassName="scroll-pb-32" onCloseAutoFocus={returnFocus}>
        {open && (
          <RuleForm
            key={round}
            entry={entry}
            onOpenChange={onOpenChange}
            closeRequestRef={closeRequestRef}
            onStartOver={() => setRound((r) => r + 1)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * The form itself — mounted only while the dialog is `open` (see
 * `RuleDialog` above), so every field is seeded once, from the draft of this
 * entry point when there is one, else from the entry point's defaults.
 */
function RuleForm({
  entry,
  onOpenChange,
  closeRequestRef,
  onStartOver,
}: {
  entry: RuleEntry;
  onOpenChange: (open: boolean) => void;
  closeRequestRef: MutableRefObject<(() => void) | null>;
  onStartOver: () => void;
}) {
  const existing = entry.kind === "edit" ? entry.rule : null;
  const { me } = useAuth();
  // A draft is a signed-in user's; with nobody signed in, nothing is kept.
  const draftKey = me ? ruleDraftKey(me.user_id, entry) : null;
  const seed = seedRule(entry);
  const seedSnapshot = snapshotOf(seed);
  const pathScope = entryScope(entry);
  /**
   * A rule carries no version, so an `edit` draft is restored only when the
   * rule is still the one it began from (its `base`). A draft of a rule
   * changed since is dropped, and the form says so: restoring it would PATCH
   * the old fields over the newer ones (round 1 review).
   */
  const [start] = useState(() => {
    const stored = draftKey ? loadStoredRuleDraft(draftKey) : undefined;
    const stale = stored !== undefined && entry.kind === "edit" && stored.base !== seedSnapshot;
    if (stored && !stale) return { state: stateOf(stored.draft, pathScope), base: stored.base, restored: true, discarded: false };
    return { state: stateOf(seed, pathScope), base: seedSnapshot, restored: false, discarded: stale };
  });

  // Read here rather than passed down — see the same note in `GrantDialog`
  // (review M-6).
  const catalogsQ = useControlPlaneGovernanceListCatalogs({ query: { retry: false } });
  const catalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
  const catalogName = (id: string) => catalogs.find((c) => c.id === id)?.name ?? id;

  const [form, setForm] = useState<FormState>(start.state);
  // Whether the user changed anything: only then is there a draft to keep or a close to confirm.
  const [touched, setTouched] = useState(start.restored);
  // The snapshot of the rule the form began from, kept beside its draft.
  const [base, setBase] = useState(start.base);
  const update = (patch: Partial<FormState>) => {
    setTouched(true);
    setForm((f) => ({ ...f, ...patch }));
  };
  // An untouched form follows its seed: a refetch of the rule being edited
  // shows the server's fields, and never becomes a draft of the old ones.
  if (!touched && seedSnapshot !== base) {
    setBase(seedSnapshot);
    setForm(stateOf(seed, pathScope));
  }
  const { name, kindType, maskType, valueExpr, filterExpr, selectorType, tagKey, scope, exempt } = form;

  const [preview, setPreview] = useState<PreviewResponse | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  // The refusal itself, so a coded one (spec C5, C6.3) can be explained rather than quoted.
  const [previewFailure, setPreviewFailure] = useState<unknown>(null);
  const [previewedFor, setPreviewedFor] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveFailure, setSaveFailure] = useState<unknown>(null);
  // "Create anyway" for a second mask on a column (spec C6.2); every preview asks again.
  const [ticked, setTicked] = useState(false);
  // Set by Remove from table, so the fresh preview's result takes focus from the button that left.
  const focusResultRef = useRef(false);
  // Set by Apply by path instead, so focus lands on the catalog picker once it renders.
  const focusCatalogRef = useRef(false);
  // The conflict whose Remove from table is being confirmed, and the button that asked.
  const [removing, setRemoving] = useState<{ conflict: TableTagMaskConflict & { warehouseId: string }; opener: HTMLElement } | null>(null);
  // The form a refused save was sent with: its actions apply only while the form still matches it.
  const [savedFor, setSavedFor] = useState<string | null>(null);
  // Bumped on every failure, so focus moves to the error even when its text repeats.
  const [errorSeq, setErrorSeq] = useState(0);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  const queryClient = useQueryClient();
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: getPermListRulesQueryKey() });
    // A changed rule may reach other catalogs: its "Not enforced yet" is asked again.
    invalidateRuleEnforcement(queryClient);
  };

  const previewMutation = usePermPreview<ApiError>();
  const createRule = usePermCreateRule<ApiError>({ mutation: { onSuccess: invalidate } });
  const patchRule = usePermPatchRule<ApiError>({ mutation: { onSuccess: invalidate } });
  const canRemoveTag = useCanEditCatalogTags();

  const body = bodyOf(form);
  const snapshot = JSON.stringify(body);
  const bodySnapshot = JSON.stringify({ id: existing?.id ?? null, body });
  const dirty = touched && snapshot !== seedSnapshot;

  // The draft follows the form: kept while the user's changes differ from the entry point's defaults.
  useEffect(() => {
    if (!draftKey) return;
    if (dirty) saveRuleDraft(draftKey, JSON.parse(snapshot) as NewRule, base);
    else clearRuleDraft(draftKey);
  }, [snapshot, draftKey, dirty, base]);
  useUnsavedChangesWarning(dirty);

  function requestClose() {
    if (dirty) setConfirmDiscard(true);
    else onOpenChange(false);
  }
  useEffect(() => {
    closeRequestRef.current = requestClose;
    return () => {
      closeRequestRef.current = null;
    };
  });

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
          ? "Choose a classification."
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
    update(next === "row_filter" && scope.column ? { kindType: next, scope: { ...scope, column: undefined } } : { kindType: next });
  }

  const previewStale = previewedFor !== bodySnapshot;

  /**
   * One click to what else this entry point could apply the rule to (spec
   * C8.3, Task 13): each classification the table or column carries but the
   * rule is not on, and, while it is on a classification, the entry's own path.
   */
  const entryClassifications = entry.kind === "column" || entry.kind === "table" ? (entry.classifications ?? []) : [];
  const alternatives: { id: string; label: string; apply: () => void }[] = [
    ...entryClassifications
      .filter((c) => !(selectorType === "tag" && tagKey === c.key))
      .map((c) => ({ id: `tag:${c.key}`, label: `Or apply it to ${c.name}`, apply: () => update({ selectorType: "tag", tagKey: c.key }) })),
    ...(pathScope && selectorType === "tag" && entryClassifications.length > 0
      ? [
          {
            id: "path",
            label: entry.kind === "column" ? `Or apply it to column ${entry.column} only` : `Or apply it to table ${pathScope.table} only`,
            apply: () => update({ selectorType: "path", scope: pathScope }),
          },
        ]
      : []),
  ];

  async function runPreview() {
    setPreviewError(null);
    setPreviewFailure(null);
    setSaveError(null);
    setSaveFailure(null);
    setTicked(false);
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
        setErrorSeq((n) => n + 1);
      }
      setPreview(data ?? null);
      setPreviewedFor(bodySnapshot);
      // The result renders under the fold, behind the sticky footer: take the reader to it.
      if (data) focusResultRef.current = true;
    } catch (e) {
      setPreviewError((e as ApiError).message || "Preview failed.");
      setPreviewFailure(e);
      setErrorSeq((n) => n + 1);
      setPreviewedFor(bodySnapshot);
    }
  }

  /**
   * After Remove from table is confirmed and done (spec C5.1; the confirm,
   * `RemoveClassificationDialog`, refreshes what an attach changes): preview
   * the same rule again, which shows the next table when there is one, and
   * hand focus to its result.
   */
  function afterRemoved() {
    if (blockedReason) return;
    focusResultRef.current = true;
    void runPreview();
  }

  const conflictActions: ConflictActions = {
    canRemove: canRemoveTag,
    removing: removing !== null,
    onRemove: (conflict, opener) => setRemoving({ conflict, opener }),
    onApplyByPath: () => {
      update({ selectorType: "path", scope: EMPTY_SCOPE });
      focusCatalogRef.current = true;
    },
  };

  async function save() {
    setSaveError(null);
    setSaveFailure(null);
    try {
      if (existing) {
        await patchRule.mutateAsync({ id: existing.id, data: body });
      } else {
        await createRule.mutateAsync({ data: body });
      }
      if (draftKey) clearRuleDraft(draftKey);
      toast.success(existing ? "Rule updated." : "Rule created.");
      onOpenChange(false);
    } catch (e) {
      setSaveError((e as ApiError).message || "Failed to save the rule.");
      setSaveFailure(e);
      setSavedFor(bodySnapshot);
      setErrorSeq((n) => n + 1);
    }
  }

  const saving = createRule.isPending || patchRule.isPending;
  const previewing = previewMutation.isPending;
  /**
   * Why Create is not available, or `null` (spec C8.2): the first of what
   * blocks the rule, a missing or out-of-date preview, and a refused preview.
   * Shown beside the button and tied to it as its description.
   */
  const effect = preview && !previewStale ? (preview.effect ?? null) : null;
  // Only a mask can be a second mask on a column; the server lists masks only, and the kind is checked anyway.
  const needsTick = !!effect && effect.kind === "mask" && effect.alreadyMasked.length > 0 && !ticked;
  const createReason =
    blockedReason ??
    (previewStale || (!preview && !previewError)
      ? PREVIEW_TO_ENABLE_CREATE
      : previewError
        ? "The preview was refused. Change the rule, then preview it again."
        : needsTick
          ? tickReason(!!existing)
          : null);
  const canSave = createReason === null;
  const reasonId = useId();
  const warnId = useId();
  // The reason when Create waits, and the "Not enforced yet" warning even when it does not (spec C7 A11y).
  const createDescribedBy =
    [canSave ? null : reasonId, showsNotEnforced(effect) ? warnId : null].filter(Boolean).join(" ") || undefined;
  const saveLabel = existing ? "Save changes" : "Create";

  /** The footer's error: the latest failure, in the server's words (FE-ACTION-3). */
  const footerError = saveError
    ? { title: "The rule was not saved.", message: saveError, error: saveFailure, actionable: savedFor === bodySnapshot }
    : previewError
      ? {
          // Kept after an edit, so the server's words stay readable, but named as the earlier version's.
          title: previewStale ? "The preview of an earlier version was refused." : "The preview was refused.",
          message: previewError,
          error: previewFailure,
          // An earlier version's refusal is explained, but its actions act on the current form only after a new preview.
          actionable: !previewStale,
        }
      : null;
  // Focus goes to the error's title, not the `role="alert"` container, which is announced already.
  const errorRef = useRef<HTMLParagraphElement>(null);
  const columnHelpId = useId();
  useEffect(() => {
    // The footer is sticky, so the error is in view; focus takes the reader there too.
    if (errorSeq > 0) errorRef.current?.focus();
  }, [errorSeq]);
  const resultRef = useRef<HTMLDivElement>(null);
  const previewButtonRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // A refused re-preview takes focus to its error instead (`errorSeq`).
    if (!focusResultRef.current || (!preview && !previewError)) return;
    if (preview) {
      // Centred, so the sticky footer cannot cover the "Not enforced yet" warning; then focus without a second scroll.
      resultRef.current?.scrollIntoView?.({ block: "center" });
      resultRef.current?.focus({ preventScroll: true });
    }
    focusResultRef.current = false;
  }, [preview, previewError]);
  useEffect(() => {
    if (selectorType !== "path" || !focusCatalogRef.current) return;
    document.getElementById("scope-catalog")?.focus();
    focusCatalogRef.current = false;
  }, [selectorType]);

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
        {start.discarded && (
          <p role="status" className="text-xs text-muted-foreground">
            This rule changed since your draft; the draft was discarded.
          </p>
        )}
        {start.restored && (
          <p role="status" className="text-xs text-muted-foreground">
            Restored your unsaved rule ·{" "}
            <Button
              type="button"
              variant="link"
              className="h-auto p-0 text-xs"
              onClick={() => {
                if (draftKey) clearRuleDraft(draftKey);
                onStartOver();
              }}
            >
              Start over
            </Button>
          </p>
        )}

        <div className="space-y-1.5">
          <Label htmlFor="rule-name">
            Name
            <RequiredMark />
          </Label>
          <Input
            id="rule-name"
            aria-required="true"
            value={name}
            onChange={(e) => update({ name: e.target.value })}
            autoFocus
          />
        </div>

        <div className="space-y-1.5">
          <Label htmlFor="rule-kind">{TERM.kind}</Label>
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
              <Select value={maskType} onValueChange={(v) => update({ maskType: v })}>
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
                <Label htmlFor="rule-value-expr">
                  Custom expression
                  <RequiredMark />
                </Label>
                <Textarea
                  id="rule-value-expr"
                  aria-required="true"
                  rows={2}
                  value={valueExpr}
                  onChange={(e) => update({ valueExpr: e.target.value })}
                  placeholder="e.g. cast(NULL as {type})"
                />
              </div>
            )}
          </div>
        ) : (
          <div className="space-y-1.5">
            <Label htmlFor="rule-filter-expr">
              Filter expression
              <RequiredMark />
            </Label>
            <Textarea
              id="rule-filter-expr"
              aria-required="true"
              rows={2}
              value={filterExpr}
              onChange={(e) => update({ filterExpr: e.target.value })}
              placeholder="e.g. region = 'us'"
            />
          </div>
        )}

        <div className="space-y-1.5">
          <Label htmlFor="rule-selector-type">{TERM.appliesTo}</Label>
          <Select value={selectorType} onValueChange={(v) => update({ selectorType: v as SelectorType })}>
            <SelectTrigger id="rule-selector-type">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="tag">{TERM.classification}</SelectItem>
              <SelectItem value="path">{TERM.path}</SelectItem>
            </SelectContent>
          </Select>
          {alternatives.length > 0 && (
            <ul role="list" className="flex flex-wrap gap-x-3 gap-y-1">
              {alternatives.map((a) => (
                <li key={a.id}>
                  <Button type="button" variant="link" className="h-auto p-0 text-xs" onClick={a.apply}>
                    {a.label}
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>

        {selectorType === "tag" ? (
          <TagKeySelector value={tagKey} onChange={(key) => update({ tagKey: key })} />
        ) : (
          <div className="space-y-1.5">
            <Label>Scope</Label>
            <ScopePicker
              value={scope}
              onChange={(next) => update({ scope: next })}
              rowFilterMode={kindType === "row_filter"}
              catalogRequired
              columnDescribedBy={kindType === "mask" ? columnHelpId : undefined}
            />
            {kindType === "mask" && (
              <p id={columnHelpId} className="text-xs text-muted-foreground">
                {EMPTY_COLUMN_MASKS_EVERY_COLUMN}
              </p>
            )}
            {scope.catalog_id && (
              <p className="text-xs text-muted-foreground">
                {TERM.appliesTo} {formatScope(scope, catalogName(scope.catalog_id))}
              </p>
            )}
          </div>
        )}

        <div className="space-y-1.5">
          <Label>{TERM.exempt}</Label>
          <PrincipalPicker value={exempt} onChange={(next) => update({ exempt: next })} exempt />
        </div>

        <div className="space-y-2 rounded-md border border-border bg-muted/30 p-3">
          <div className="flex items-center justify-between gap-2">
            <p className="text-sm font-medium">Preview this write</p>
            <Button
              ref={previewButtonRef}
              type="button"
              size="sm"
              variant="outline"
              aria-disabled={!!blockedReason || previewing}
              aria-describedby={blockedReason ? reasonId : undefined}
              onClick={() => {
                if (!blockedReason && !previewing) void runPreview();
              }}
            >
              {previewing ? "Checking…" : "Preview"}
            </Button>
          </div>

          {preview && !previewStale && (
            <div ref={resultRef} tabIndex={-1} className="space-y-2 text-sm">
              {effect && (
                <>
                  <EffectSummary effect={effect} warnId={warnId} appliesTo={selectorType} />
                  {effect.kind === "mask" && (
                    <AlreadyMaskedNotice
                      effect={effect}
                      rule={{ maskType, exempt }}
                      ticked={ticked}
                      onTick={setTicked}
                      editing={!!existing}
                    />
                  )}
                </>
              )}
              {/* Without an effect (an older server), the recompile is all there is to say: shown open. */}
              <RecompileDetails preview={preview} open={effect ? undefined : true} />
            </div>
          )}

          {!preview && !previewError && (
            <p className="text-xs text-muted-foreground">
              {selectorType === "tag"
                ? "A rule on a classification applies in every catalog where the classification is attached. Preview shows what it reaches and who enforces it, before you create it."
                : "Preview shows what this rule reaches and who enforces it, before you create it."}
            </p>
          )}
          {previewStale && (preview || previewError) && (
            <p className="text-xs text-muted-foreground">
              The form changed since that preview — run it again before saving.
            </p>
          )}
        </div>
      </div>

      {/* Sticky inside the dialog's scrolling body, so the reason and any error stay in view (N26). */}
      <div
        role="group"
        aria-label="Rule actions"
        className="sticky bottom-0 z-10 space-y-2 border-t border-border bg-background pt-3"
      >
        {footerError && (
          <Alert variant="destructive">
            <AlertTitle ref={errorRef} tabIndex={-1}>
              {footerError.title}
            </AlertTitle>
            <AlertDescription>
              <RefusalBody
                error={footerError.error}
                message={footerError.message}
                actions={footerError.actionable ? conflictActions : undefined}
              />
            </AlertDescription>
          </Alert>
        )}
        <div className="flex flex-wrap items-center justify-end gap-2">
          {/* Always rendered: a live region must exist before its text changes. */}
          {/* Its own row on a narrow screen, so Cancel and Create stay side by side (live result §5, 390 px). */}
          <p id={reasonId} aria-live="polite" className="mr-auto basis-full text-xs text-muted-foreground sm:basis-auto">
            {createReason}
          </p>
          <Button type="button" variant="outline" onClick={requestClose}>
            Cancel
          </Button>
          <Button
            type="button"
            aria-disabled={!canSave || saving}
            aria-describedby={createDescribedBy}
            onClick={() => {
              if (canSave && !saving) void save();
            }}
          >
            {saving ? "Saving…" : saveLabel}
          </Button>
        </div>
      </div>

      {removing && (
        <RemoveClassificationDialog
          open
          onOpenChange={(next) => {
            if (!next) setRemoving(null);
          }}
          tag={{ id: removing.conflict.tagId, name: removing.conflict.tag, key: selectorType === "tag" ? tagKey : "" }}
          target={{
            warehouseId: removing.conflict.warehouseId,
            schema: removing.conflict.schema,
            table: removing.conflict.table,
            column: "",
            label: `${removing.conflict.schema}.${removing.conflict.table}`,
          }}
          opener={removing.opener}
          // The Remove button leaves with the conflict: focus goes to the fresh result when it is already
          // there, else to Preview, from which the result takes it when it arrives.
          focusAfterRemove={() => resultRef.current ?? previewButtonRef.current}
          onRemoved={afterRemoved}
        />
      )}

      <AlertDialog open={confirmDiscard} onOpenChange={setConfirmDiscard}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard this rule?</AlertDialogTitle>
            <AlertDialogDescription>
              Your changes to this rule are not saved. Keep editing to go back to them, or discard them.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <Button
              type="button"
              variant="destructive"
              onClick={() => {
                if (draftKey) clearRuleDraft(draftKey);
                setConfirmDiscard(false);
                onOpenChange(false);
              }}
            >
              Discard
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
