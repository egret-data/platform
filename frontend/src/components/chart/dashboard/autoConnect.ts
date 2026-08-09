import { toast } from "sonner";

/** The shape auto-connect needs from a draft card. Deliberately structural rather than importing
 *  `DraftCard`: the module has to be usable from a test that never builds a dashboard. */
export interface AutoConnectCard {
  id: string;
  card_type: string;
  options: Record<string, unknown>;
  tabKey?: string | null;
}

export interface AutoConnectTarget {
  cardId: string;
  column: string;
}

/** Metabase's timeouts, verbatim (auto-wire-parameters/constants.ts:1-2). */
export const AUTO_CONNECT_TOAST_MS = 12000;
export const AUTO_CONNECT_UNDO_TOAST_MS = 8000;

/** toasts.ts:41-43. The curly quotes are Metabase's. */
export function autoConnectPrompt(columnName: string, hasMultipleTabs: boolean): string {
  return hasMultipleTabs
    ? `Auto-connect this filter to all questions containing “${columnName}”, in the current tab?`
    : `Auto-connect this filter to all questions containing “${columnName}”?`;
}

/** toasts.ts:78. */
export function autoConnectDoneMessage(columnName: string): string {
  return `The filter was auto-connected to all questions containing “${columnName}”.`;
}

/** The columns a card can offer a parameter — the same list the mapping overlay shows. A
 *  temporal-unit parameter re-grids an already-grouped column, so it can only be mapped to a
 *  column the saved spec already buckets. */
export function mappableColumns(
  spec: { dimensions: string[]; columns: string[]; temporal_units?: Record<string, string> },
  isTemporalUnit: boolean,
): string[] {
  if (isTemporalUnit) return Object.keys(spec.temporal_units ?? {});
  return [...spec.dimensions, ...spec.columns];
}

const mappingsOf = (c: AutoConnectCard): Record<string, string> =>
  (c.options.paramMappings as Record<string, string> | undefined) ?? {};

/**
 * The other cards this filter could be connected to. Ported from Metabase's
 * `getAllDashboardCardsWithUnmappedParameters` + `getMatchingParameterOption`:
 *
 *  - chart cards only (a text/heading/link card has no columns);
 *  - the SAME TAB only — Metabase's toast says "in the current tab" precisely because it never
 *    reaches across one;
 *  - never the card the user just mapped;
 *  - never a card that already has a mapping for this parameter, so an existing hand-made choice
 *    is not overwritten (this is also what makes the undo safe — see `revertAutoConnect`);
 *  - the column must be present by NAME on the card's chart.
 */
export function findAutoConnectTargets(args: {
  cards: AutoConnectCard[];
  columnsOf: (cardId: string) => string[];
  paramId: string;
  column: string;
  sourceCardId: string;
  tabKey: string | null;
}): AutoConnectTarget[] {
  const { cards, columnsOf, paramId, column, sourceCardId, tabKey } = args;
  return cards
    .filter((c) =>
      c.card_type === "chart" &&
      c.id !== sourceCardId &&
      (c.tabKey ?? null) === tabKey &&
      !mappingsOf(c)[paramId] &&
      columnsOf(c.id).includes(column))
    .map((c) => ({ cardId: c.id, column }));
}

/** The patch: each target gains `paramMappings[paramId]`. Returns a new array and new option
 *  objects — the draft setters compare by reference. */
export function applyAutoConnect<T extends AutoConnectCard>(
  cards: T[], paramId: string, targets: AutoConnectTarget[],
): T[] {
  const byId = new Map(targets.map((t) => [t.cardId, t.column]));
  return cards.map((c) => {
    const column = byId.get(c.id);
    if (!column) return c;
    return { ...c, options: { ...c.options, paramMappings: { ...mappingsOf(c), [paramId]: column } } };
  });
}

/** The exact inverse: remove the mapping from the cards in `targets` and nobody else. A blanket
 *  "delete this parameter's mapping everywhere" would also destroy the mapping the user made by
 *  hand on the source card, and any mapping auto-connect skipped because one was already there. */
export function revertAutoConnect<T extends AutoConnectCard>(
  cards: T[], paramId: string, targets: AutoConnectTarget[],
): T[] {
  const ids = new Set(targets.map((t) => t.cardId));
  return cards.map((c) => {
    if (!ids.has(c.id)) return c;
    const { [paramId]: _removed, ...rest } = mappingsOf(c);
    return { ...c, options: { ...c.options, paramMappings: rest } };
  });
}

/**
 * The two toasts. Metabase asks first and connects second — the toast is an OFFER, not a report —
 * and then immediately offers the undo, because the accepted action rewrote many cards at once.
 * Both halves are required; a connect without the undo is the failure mode §7 of the design calls
 * out by name.
 */
export function offerAutoConnect(opts: {
  targets: AutoConnectTarget[];
  columnName: string;
  hasMultipleTabs: boolean;
  connect: () => void;
  revert: () => void;
}): void {
  if (opts.targets.length === 0) return;
  toast(autoConnectPrompt(opts.columnName, opts.hasMultipleTabs), {
    duration: AUTO_CONNECT_TOAST_MS,
    action: {
      label: "Auto-connect",
      onClick: () => {
        opts.connect();
        toast(autoConnectDoneMessage(opts.columnName), {
          duration: AUTO_CONNECT_UNDO_TOAST_MS,
          action: { label: "Undo", onClick: () => opts.revert() },
        });
      },
    },
  });
}
