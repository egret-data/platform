import { Link } from "react-router-dom";
import { Lock } from "lucide-react";
import type { Tag } from "@/api/tags";
import { useAuth } from "@/auth/useAuth";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";

/** The colour a tag without one is drawn in. */
export const DEFAULT_COLOR = "#6366f1";

/**
 * The server's rule for a classification key (`validate_key`,
 * `crates/egret-bi/src/tags.rs`: lowercase ascii letters, digits, `:` and
 * `-`), written once on this side so the form checks it as you type.
 */
export const TAG_KEY_PATTERN = /^[a-z0-9:-]+$/;

/**
 * What the key field says while its value breaks `TAG_KEY_PATTERN`, or null:
 * the characters it may not hold, each once (a space as "space"). What it may
 * hold is the field's hint, read after this.
 */
export function tagKeyRuleError(key: string): string | null {
  const bad = [...new Set([...key].filter((c) => !TAG_KEY_PATTERN.test(c)))];
  if (bad.length === 0) return null;
  return `Not allowed in a key: ${bad.map((c) => (c === " " ? "space" : c)).join(", ")}.`;
}

/** A valid tag key from a name: lowercase, every run outside `[a-z0-9:-]` (an `_` included) → `-`. */
export function slugifyTagKey(s: string): string {
  return s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9:-]+/g, "-")
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

/**
 * A name as the server compares names for duplicates: `lower(btrim(name))`.
 * A hint only: JavaScript's case folding and trimming need not match
 * Postgres's for every non-ASCII name, and the server's `tag_name_taken` decides.
 */
const foldName = (name: string) => name.trim().toLowerCase();

/**
 * The classification in `tags` that already has `name` (case-insensitive,
 * trimmed, as the server's `tag_name_taken` check compares), other than
 * `exceptId`. The server stays the authority: this only says it sooner.
 */
export function nameHolder(tags: Tag[] | undefined, name: string, exceptId?: string): { id: string; name: string } | null {
  const folded = foldName(name);
  if (!folded || !tags) return null;
  return tags.find((t) => t.id !== exceptId && foldName(t.name) === folded) ?? null;
}

/** "A classification named <Public> already exists.", the holder linked to its page. */
export function NameTaken({ holder }: { holder: { id: string; name: string } }) {
  return (
    <>
      {"A classification named "}
      <Link to={tagUrl(holder.id)} className="font-medium underline">
        {holder.name}
      </Link>
      {" already exists."}
    </>
  );
}

/**
 * A classification's colour: the native colour picker under its label, and a
 * line that says what the colour is for and its value as text (spec C10.4),
 * so the value reads without opening the picker.
 */
export function ColourField({ id, color, onChange }: { id: string; color: string; onChange: (color: string) => void }) {
  const helpId = `${id}-help`;
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>Colour</Label>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          type="color"
          className="h-9 w-12 shrink-0 p-1"
          value={color}
          aria-describedby={helpId}
          onChange={(e) => onChange(e.target.value)}
        />
        <p id={helpId} className="text-xs text-muted-foreground">
          {"Colour of the swatch shown next to the name. "}
          <span className="inline-flex items-center gap-1 font-mono">
            <TagSwatch color={color} />
            {color}
          </span>
        </p>
      </div>
    </div>
  );
}
