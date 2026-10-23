import { useId } from "react";
import { Label } from "@/components/ui/label";
import {
  Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { FIELD_LABEL } from "@/components/chart/settings/FieldLabel";
import type { MoveDestination } from "./inlineParameters";

/** The sentinel the picker carries for "not on any card". Metabase's `TOP_NAV_VALUE`
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
 *  Metabase's `MoveParameterMenu` (`:67-134`). A grouped `Select` rather than a popover, matching
 *  every other picker in this panel (`FilterSettingsPanel.tsx`).
 *
 *  Metabase's select carries no visible caption because its options are icon-prefixed and
 *  self-describing. egret labels every other control in this panel, so it gets one; that is the
 *  only intentional difference. */
export function MoveParameterField({ value, destinations, onChange }: Props) {
  const id = useId();
  if (destinations.length === 0) return null;
  const grouped = destinations.some((d) => d.tabName !== null);
  const tabNames = [...new Set(destinations.map((d) => d.tabName))].filter((n): n is string => n !== null);
  const item = (d: MoveDestination) => <SelectItem key={d.cardId} value={d.cardId}>{d.label}</SelectItem>;
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id} className={FIELD_LABEL}>Where this filter appears</Label>
      <Select value={value ?? TOP_OF_PAGE} onValueChange={(v) => onChange(v === TOP_OF_PAGE ? null : v)}>
        <SelectTrigger id={id} aria-label="filter location" className="w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value={TOP_OF_PAGE}>Top of page</SelectItem>
          {grouped
            ? tabNames.map((name) => (
                <SelectGroup key={name}>
                  <SelectLabel>{name}</SelectLabel>
                  {destinations.filter((d) => d.tabName === name).map(item)}
                </SelectGroup>
              ))
            : destinations.map(item)}
        </SelectContent>
      </Select>
    </div>
  );
}
