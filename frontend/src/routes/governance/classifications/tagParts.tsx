import { Lock } from "lucide-react";
import type { Tag } from "@/api/tags";
import { useAuth } from "@/auth/useAuth";

/** The colour a tag without one is drawn in. */
export const DEFAULT_COLOR = "#6366f1";

/** A valid tag key from a name: lowercase, every run outside `[a-z0-9:_-]` → `-`. */
export function slugifyTagKey(s: string): string {
  return s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9:_-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
}

/** The tag writes (`POST`/`PATCH`/`DELETE /tags`, the access policy) are `tag:manage`; platform admin passes every key. */
export function useCanManageTags(): boolean {
  const { me } = useAuth();
  return !!me && (me.platform_roles.includes("platform_admin") || me.permissions.includes("tag:manage"));
}

/** A tag's colour as a small round swatch beside its name (decorative: the name says which tag). */
export function TagSwatch({ color, size = "sm" }: { color: string; size?: "sm" | "lg" }) {
  const box = size === "lg" ? "h-8 w-8 rounded-lg" : "h-3 w-3 rounded-full";
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 border border-border ${box}`}
      style={{ backgroundColor: color || DEFAULT_COLOR }}
    />
  );
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "1 role · 2 groups"; the parts that are zero are left out. Empty when the tag has no clearance. */
export function clearanceCounts(t: Pick<Tag, "required_roles" | "required_groups">): string {
  const parts: string[] = [];
  if (t.required_roles.length) parts.push(plural(t.required_roles.length, "role", "roles"));
  if (t.required_groups.length) parts.push(plural(t.required_groups.length, "group", "groups"));
  return parts.join(" · ");
}

/** "T tables · C columns", or null when the tag is on no table or column. */
export function dataCounts(tables: number, columns: number): string | null {
  if (tables === 0 && columns === 0) return null;
  return `${plural(tables, "table", "tables")} · ${plural(columns, "column", "columns")}`;
}

/** A tag's clearance in words with a lock (never colour alone, FE-A11Y-4), or "None". */
export function ClearanceCell({ tag }: { tag: Pick<Tag, "required_roles" | "required_groups"> }) {
  const counts = clearanceCounts(tag);
  if (!counts) return <span className="text-muted-foreground">None</span>;
  return (
    <span className="inline-flex items-center gap-1">
      <Lock className="h-3.5 w-3.5 shrink-0" aria-hidden />
      <span className="tabular-nums">{counts}</span>
    </span>
  );
}
