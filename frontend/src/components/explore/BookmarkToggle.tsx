import { Star } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The header filter: show only bookmarked rows.
 *
 * The shared implementation, extracted here so a later migration phase has
 * something to adopt. It is NOT yet the one every page uses: `Alerts`,
 * `Charts`, `Dashboards` and `Datasets` were converted onto `ListToolbarButton`
 * directly (package U, 2026-08-22), not onto this wrapper, so the dedup this
 * component exists for has not happened for them. `Endpoints.tsx`
 * remains this file's only consumer. Adopting this wrapper in the other four
 * pages is still outstanding — a later package, not this one.
 *
 * It holds no state of its own — the page owns the filter, because the page is
 * what applies it to its rows.
 */
export function BookmarkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return <ListToolbarButton icon={Star} label="Bookmarks" active={active} filled onClick={onToggle} />;
}

/**
 * The per-row star.
 *
 * `name` is required and reaches the accessible name, because a list of thirty
 * rows otherwise gives a screen-reader user thirty controls called "Bookmark"
 * with nothing to tell them apart — the same defect the row-actions menus had.
 * The label also states the DIRECTION of the click: a filled star that says
 * "Bookmark" is telling the user the opposite of what pressing it does.
 */
export function BookmarkStar({
  bookmarked,
  name,
  onToggle,
}: {
  bookmarked: boolean;
  name: string;
  onToggle: () => void;
}) {
  const label = bookmarked ? `Remove bookmark from ${name}` : `Bookmark ${name}`;
  return <ListToolbarButton icon={Star} label={label} active={bookmarked} filled onClick={onToggle} />;
}
