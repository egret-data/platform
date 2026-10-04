import { useCallback, useMemo, useState } from "react";

/**
 * Multi-select state for a list of `{id}` rows.
 *
 * **`rows` must be the set actually RENDERED — not the page's filtered/sorted
 * model of it.** The two are different the moment a page groups its rows: a
 * collapsed folder's rows are in the group model but not on screen.
 *
 * The hook cannot enforce this. It sees an array of `{id}` and knows nothing
 * about grouping, collapse, pagination or virtualization, so it has no way to
 * tell "everything the page has" from "everything the user can see".
 *
 * Handed more than is on screen, two things go wrong, and both are silent:
 *
 * - `toggleAll` ("Select all") writes the ids of rows the user was never shown
 *   into the selection, and `allSelected`/`someSelected` answer about them too;
 * - `selectedIds` then reports them, so the "{n} selected" strip claims rows
 *   that are not in front of the user and a bulk action acts on them. On the
 *   grouped lists that is a Move to Trash, so the cost is recoverable — but on
 *   `UsersPage` it is Disable and Assign role, which are platform-wide account
 *   actions with no trash to fish them back out of.
 *
 * `selectedIds` re-derives from `rows` on every render rather than just handing
 * back `selected`, which is what covers the other order of events: a row picked
 * while visible and then hidden (its folder collapsed, a filter applied) drops
 * out of the count and out of the action, instead of staying selected
 * invisibly. Do not "simplify" it to `Array.from(selected)`.
 *
 * Every grouped list — `Charts`, `Datasets`, `Dashboards`, `Alerts`,
 * and `Endpoints` — now derives a `visibleRows` from its groups,
 * dropping the collapsed ones, and passes that. Copy one of them rather than
 * reaching for `rows`.
 *
 * The two remaining callers, `MembersPanel` and `UsersPage`, are PARTLY WRONG
 * still — read this as an outstanding bug, not an exemption. They do not group,
 * so they look compliant, but they hand their whole filtered list to
 * `FilterableTable`, which owns the search box and (on `UsersPage`) a
 * `pageSize={25}` pager, and draws only its own page.
 *
 * The worst of it is fixed: that table's header "Select all" used to toggle
 * over its post-search set rather than the page it drew, so one click with 25
 * rows on screen selected every filtered user and bulk Disable turned all of
 * them off. It now toggles only the rows it rendered.
 *
 * What remains is the other order of events. Both pages derive their targets
 * from `bulk.selected` filtered against the FULL list (`MembersPanel:110`,
 * `UsersPage:312`) rather than from `selectedIds`, so a row picked while
 * visible and then hidden — by the table's own search box, or by paging away —
 * stays selected and is still acted on. Note what that means for this file:
 * passing those pages a different `rows` argument would correct their counts
 * and leave their actions untouched. The fix has to make `FilterableTable`
 * report the rows it drew, and the pages read their targets from that.
 *
 * Do not copy those two.
 */
export function useBulkSelect<T extends { id: string }>(rows: T[]) {
  const [bulkMode, setBulkModeRaw] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const toggle = useCallback((id: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) {
        n.delete(id);
      } else {
        n.add(id);
      }
      return n;
    });
  }, []);

  const clear = useCallback(() => setSelected(new Set()), []);

  const allSelected = rows.length > 0 && rows.every((r) => selected.has(r.id));
  const someSelected = rows.some((r) => selected.has(r.id));

  const toggleAll = useCallback(() => {
    setSelected((s) => {
      const everyOn = rows.length > 0 && rows.every((r) => s.has(r.id));
      return everyOn ? new Set() : new Set(rows.map((r) => r.id));
    });
  }, [rows]);

  const setBulkMode = useCallback((v: boolean) => {
    setBulkModeRaw(v);
    if (!v) setSelected(new Set());
  }, []);

  // ids that still exist in the visible rows — see the note above: this filter
  // is the ONLY thing that unselects a row hidden AFTER it was picked.
  const selectedIds = useMemo(
    () => rows.filter((r) => selected.has(r.id)).map((r) => r.id),
    [rows, selected],
  );

  return {
    bulkMode,
    setBulkMode,
    selected,
    selectedIds,
    toggle,
    toggleAll,
    clear,
    allSelected,
    someSelected,
  };
}
