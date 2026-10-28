import { Fragment } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle } from "lucide-react";

import type { ExistingMask, PreviewResponse, PrincipalSet, RuleEffect, TrinoColumnOnly } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { otherTablesText, ruleOverlap, tableTagMaskConflict, type TableTagMaskConflict } from "@/routes/governance/apiErrors";
import {
  effectivePerPrincipal,
  effectSentence,
  maskTypeLabel,
  notEnforcedWarning,
  principalLabel,
} from "@/routes/governance/ruleEffect";

/*
 * The rule dialog's preview, in effect terms (spec
 * 2026-10-21-classification-ux-design C5, C6, C7). The dialog owns the state
 * (the preview, the tick, the refusal); these parts only lay it out.
 */

/** The tick's verb: "Create" for a new rule, "Save" when editing one. */
const tickVerb = (editing: boolean) => (editing ? "Save" : "Create");

/** Create's (or Save's) reason while masks already on a column wait for the tick (spec C6.2). */
export const tickReason = (editing: boolean) =>
  `Tick '${tickVerb(editing)} anyway' to ${editing ? "save" : "create"} a second mask on this column.`;

const ruleHref = (id: string) => `/governance/rules/${encodeURIComponent(id)}`;

/** `wh.s.t.*` (the server's whole-table coordinate) reads as "every column of wh.s.t". */
function coordinateText(coordinate: string): string {
  return coordinate.endsWith(".*") ? `every column of ${coordinate.slice(0, -2)}` : coordinate;
}

/**
 * The lead sentence and, when no engine enforces the rule, the "Not enforced
 * yet" warning (spec C7.1-2). The warning is a `role="status"` region with
 * `warnId`, which Create lists in its `aria-describedby` (spec C7 A11y).
 */
export function EffectSummary({
  effect,
  warnId,
  appliesTo,
}: {
  effect: RuleEffect;
  warnId: string;
  /** What the rule is written on (a reach of nothing reads differently for each). */
  appliesTo: "tag" | "path";
}) {
  const warning = notEnforcedWarning(effect, appliesTo);
  return (
    <>
      <p className="font-medium">{effectSentence(effect)}</p>
      {warning && (
        <Alert role="status" id={warnId}>
          <AlertTriangle className="h-4 w-4" aria-hidden="true" />
          <AlertDescription>
            <strong>Not enforced yet.</strong> {warning.join(" ")}
          </AlertDescription>
        </Alert>
      )}
    </>
  );
}

/** Whether {@link EffectSummary} shows the warning (Create then lists it as a description). */
export const showsNotEnforced = (effect: RuleEffect | null | undefined) => !!effect && notEnforcedWarning(effect) !== null;

/** "Hash, except group data-stewards" or "Hash". */
function maskSummary(m: ExistingMask): string {
  const exempt = m.exempt.include.map(principalLabel);
  return exempt.length > 0 ? `${maskTypeLabel(m.maskType)}, except ${exempt.join(", ")}` : maskTypeLabel(m.maskType);
}

/** The masks grouped by the column they share, in the server's order. */
function byCoordinate(masks: ExistingMask[]): [string, ExistingMask[]][] {
  const groups = new Map<string, ExistingMask[]>();
  for (const m of masks) groups.set(m.coordinate, [...(groups.get(m.coordinate) ?? []), m]);
  return [...groups.entries()];
}

/**
 * Other masks already on a column this rule reaches (spec C6.2): who masks
 * it, with a link to each rule; the result per exemption as written (names
 * are compared as written, not expanded to people: see
 * `effectivePerPrincipal`); and the tick Create (Save, when editing) waits
 * for. A cut list (`truncated`) says more may exist.
 */
export function AlreadyMaskedNotice({
  effect,
  rule,
  ticked,
  onTick,
  editing,
}: {
  effect: RuleEffect;
  rule: { maskType: string; exempt: PrincipalSet };
  ticked: boolean;
  onTick: (ticked: boolean) => void;
  editing: boolean;
}) {
  const groups = byCoordinate(effect.alreadyMasked);
  if (groups.length === 0) {
    return effect.truncated ? (
      <p className="text-xs text-muted-foreground">Other masks may reach these columns: the check stopped at its limit.</p>
    ) : null;
  }
  const cut = effect.truncated ? (
    <p className="text-xs text-muted-foreground">
      These may not be all: the check stopped at its limit, and more masks may reach these columns.
    </p>
  ) : null;

  const one = groups.length === 1;
  const target = one ? coordinateText(groups[0][0]) : "these columns";
  const count = one ? groups[0][1].length + 1 : 0;
  const masks = count === 2 ? "two masks" : one ? `${count} masks` : "more than one mask";
  return (
    <div className="space-y-2 rounded-md border border-border bg-background p-3">
      {groups.map(([coordinate, existing]) => (
        <div key={coordinate} className="space-y-1">
          <p>
            {`${coordinateText(coordinate)} is already masked by `}
            {existing.map((m, i) => (
              <Fragment key={m.ruleId}>
                {i > 0 && (i === existing.length - 1 ? " and " : ", ")}
                <Link to={ruleHref(m.ruleId)} className="font-medium text-primary hover:underline">
                  {m.ruleName}
                </Link>
                {` (${maskSummary(m)})`}
              </Fragment>
            ))}
            .
          </p>
          <ul
            aria-label={
              one ? "Result by exemption as written" : `Result by exemption as written on ${coordinateText(coordinate)}`
            }
            className="list-disc space-y-0.5 pl-5 text-muted-foreground"
          >
            {effectivePerPrincipal(rule, existing, { truncated: effect.truncated }).map((l) => (
              <li key={l.principal}>{`${l.principal}: ${l.text}`}</li>
            ))}
          </ul>
        </div>
      ))}
      <p className="text-xs text-muted-foreground">
        A person exempted under more than one name may see more than their line says.
      </p>
      {cut}
      <div className="flex items-start gap-2">
        <Checkbox id="rule-create-anyway" checked={ticked} onCheckedChange={(v) => onTick(v === true)} />
        <Label htmlFor="rule-create-anyway" className="font-normal">
          {`${tickVerb(editing)} anyway: I understand ${target} will have ${masks}`}
        </Label>
      </div>
    </div>
  );
}

/** Create's (or Save's) reason while a row filter that filters nothing on Trino waits for the tick (spec §5.8, C9). */
export const trinoTickReason = (editing: boolean) =>
  `Tick '${tickVerb(editing)} anyway' to ${editing ? "save" : "create"} a row filter that filters nothing on Trino on these tables.`;

/** "wh", "lake and wh", "a, b and c". */
function andList(items: string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * A row filter on a classification that Trino would not apply on some tables
 * (spec §5.8, D3): the classification is on their columns, not on the table,
 * and Trino filters by a classification only on the whole table. StarRocks
 * filters them. Create (Save, when editing) waits for the same tick as a second
 * mask (C6). `id` is listed in Create's `aria-describedby`.
 */
export function TrinoColumnOnlyNotice({
  only,
  classification,
  ticked,
  onTick,
  editing,
  id,
}: {
  only: TrinoColumnOnly;
  /** The classification's name, as the form shows it. */
  classification: string;
  ticked: boolean;
  onTick: (ticked: boolean) => void;
  editing: boolean;
  id: string;
}) {
  const one = only.tables === 1;
  return (
    <div data-testid="trino-column-only" className="space-y-2 rounded-md border border-border bg-background p-3">
      <p id={id} className="flex gap-2">
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>
          {`On Trino this row filter filters nothing on `}
          <span className="tabular-nums">{only.tables}</span>
          {` ${one ? "table" : "tables"} in ${andList(only.catalogs)}: `}
          <em>{classification}</em>
          {` is on ${one ? "its" : "their"} columns, not on the table. StarRocks filters ${one ? "it" : "them"}. Attach `}
          <em>{classification}</em>
          {" to the table to filter on Trino."}
        </span>
      </p>
      <div className="flex items-start gap-2">
        <Checkbox id="rule-create-anyway-trino" checked={ticked} onCheckedChange={(v) => onTick(v === true)} />
        <Label htmlFor="rule-create-anyway-trino" className="font-normal">
          {`${tickVerb(editing)} anyway: I understand this row filter filters nothing on Trino on these tables`}
        </Label>
      </div>
    </div>
  );
}

/**
 * The profiles and counts a write recompiles, folded away (spec C7.3). The
 * skip list keys on its index on purpose (SP4b-2 finding 4): `SkipReport` is
 * `{source_kind, id, reason}` with no profile field, so the same tag rule
 * skipped in two profiles is two byte-identical entries.
 */
export function RecompileDetails({ preview, open }: { preview: PreviewResponse; open?: boolean }) {
  return (
    <details open={open} className="text-sm">
      <summary className="cursor-pointer text-muted-foreground">What egret will recompile</summary>
      <div className="mt-1 space-y-1">
        <p>
          Would recompile{" "}
          {preview.affected.length === 0 ? "no profiles" : preview.affected.map((p) => p.name).join(", ")}.
        </p>
        <p className="text-muted-foreground">
          created {preview.compile.created} · updated {preview.compile.updated} · deleted {preview.compile.deleted} ·
          unchanged {preview.compile.unchanged}
        </p>
        {preview.compile.skipped.length > 0 && (
          <ul className="list-disc space-y-0.5 pl-5 text-muted-foreground">
            {preview.compile.skipped.map((s, i) => (
              <li key={`${s.source_kind}-${s.id}-${i}`}>
                {s.source_kind} {s.id}: {s.reason}
              </li>
            ))}
          </ul>
        )}
      </div>
    </details>
  );
}

function ServerDetails({ message }: { message: string }) {
  return (
    <details>
      <summary className="cursor-pointer">Details</summary>
      <p className="mt-1 break-words">{message}</p>
    </details>
  );
}

/** The table-level classification conflict's actions (spec C5.1). */
export type ConflictActions = {
  /** Whether the viewer may remove a classification (`catalog_registry:update`). */
  canRemove: boolean;
  /** The Remove confirm is open or its removal is in flight: neither action may start another change. */
  removing: boolean;
  /** Opens the Remove confirm; `opener` takes focus back on Cancel. */
  onRemove: (conflict: TableTagMaskConflict & { warehouseId: string }, opener: HTMLElement) => void;
  onApplyByPath: () => void;
};

/** Remove from table and Apply by path instead (spec C5.1). */
function ConflictButtons({ conflict, actions }: { conflict: TableTagMaskConflict; actions: ConflictActions }) {
  const { warehouseId } = conflict;
  return (
    <div className="flex flex-wrap items-center gap-2">
      {/* Without a warehouse the write in flight is the attachment itself: nothing to remove (spec C5 B2.1). */}
      {warehouseId &&
        (actions.canRemove ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            aria-disabled={actions.removing}
            onClick={(event) => {
              if (!actions.removing) actions.onRemove({ ...conflict, warehouseId }, event.currentTarget);
            }}
          >
            Remove from table
          </Button>
        ) : (
          <span>Removing needs the catalog_registry:update permission.</span>
        ))}
      <Button
        type="button"
        size="sm"
        variant="outline"
        aria-disabled={actions.removing}
        onClick={() => {
          if (!actions.removing) actions.onApplyByPath();
        }}
      >
        Apply by path instead
      </Button>
    </div>
  );
}

/**
 * A refused preview or save, in the server's words unless it is one the
 * area explains (FE-ACTION-3): the table-level classification conflict
 * (spec C5) and an overlap of two masks (spec C6.3), each with the server's
 * sentence under Details. Without `actions` (the refusal of an earlier
 * version of the form) the conflict is explained but offers nothing to do.
 * `unsavedName` is the name of the rule being created: a refused preview of a
 * new rule names it with an id that was rolled back, so it is not linked (KI-289).
 */
export function RefusalBody({
  error,
  message,
  actions,
  unsavedName,
}: {
  error: unknown;
  message: string;
  actions?: ConflictActions;
  unsavedName?: string;
}) {
  const conflict = tableTagMaskConflict(error);
  if (conflict) {
    return (
      <div className="space-y-2">
        <p>{`${conflict.tag} is on the whole table ${conflict.schema}.${conflict.table}, so masking by ${conflict.tag} would make that table unreadable.`}</p>
        {otherTablesText(conflict) && <p>{otherTablesText(conflict)}</p>}
        {actions && <ConflictButtons conflict={conflict} actions={actions} />}
        <ServerDetails message={conflict.error} />
      </div>
    );
  }
  const overlap = ruleOverlap(error);
  if (overlap) {
    const kind = overlap.kind === "row_filter" ? "row filters" : "masks";
    return (
      <div className="space-y-2">
        <p>
          {`This would make two ${kind} decide ${overlap.coordinate} differently: `}
          {overlap.rules.map((r, i) => (
            <Fragment key={`${r.id ?? ""}-${r.name}`}>
              {i > 0 && (i === overlap.rules.length - 1 ? " and " : ", ")}
              {r.id && r.name !== unsavedName ? (
                <Link to={ruleHref(r.id)} className="font-medium underline">
                  {r.name}
                </Link>
              ) : (
                r.name
              )}
            </Fragment>
          ))}
          .
        </p>
        <ServerDetails message={overlap.error} />
      </div>
    );
  }
  return <>{message}</>;
}
