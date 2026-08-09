import type { MoveDestination } from "./inlineParameters";

/** The sentinel the `<select>` carries for "not on any card". Metabase's `TOP_NAV_VALUE`
 *  (MoveParameterMenu.tsx:30) with egret's wording; it is never a card id, and the handler
 *  converts it to `null` before anything downstream sees it. */
const TOP_OF_PAGE = "top-of-page";

interface Props {
  /** The host card's id, or null when the filter is on the top bar. */
  value: string | null;
  destinations: MoveDestination[];
  onChange: (cardId: string | null) => void;
}

/** Where a filter appears: the top of the page, or a heading/question card, grouped by tab —
 *  Metabase's `MoveParameterMenu` (`:67-134`). A native grouped `<select>` rather than a popover,
 *  matching every other control in this panel (`FilterSettingsPanel.tsx`) and the `<optgroup>`
 *  pattern `FilterPill`'s temporal-unit picker already uses.
 *
 *  Metabase's select carries no visible caption because its options are icon-prefixed and
 *  self-describing. egret labels every other control in this panel, so it gets one; that is the
 *  only intentional difference. */
export function MoveParameterField({ value, destinations, onChange }: Props) {
  if (destinations.length === 0) return null;
  const grouped = destinations.some((d) => d.tabName !== null);
  const tabNames = [...new Set(destinations.map((d) => d.tabName))].filter((n): n is string => n !== null);
  return (
    <label className="flex flex-col gap-1 text-xs text-muted-foreground">Where this filter appears
      <select aria-label="filter location" className="h-8 w-full rounded border bg-background px-2 text-sm"
        value={value ?? TOP_OF_PAGE}
        onChange={(e) => onChange(e.target.value === TOP_OF_PAGE ? null : e.target.value)}>
        <option value={TOP_OF_PAGE}>Top of page</option>
        {grouped
          ? tabNames.map((name) => (
              <optgroup key={name} label={name}>
                {destinations.filter((d) => d.tabName === name).map((d) => (
                  <option key={d.cardId} value={d.cardId}>{d.label}</option>
                ))}
              </optgroup>
            ))
          : destinations.map((d) => <option key={d.cardId} value={d.cardId}>{d.label}</option>)}
      </select>
    </label>
  );
}
