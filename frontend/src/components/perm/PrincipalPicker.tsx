import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, X } from "lucide-react";

import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { PermPrincipal, PrincipalSet } from "@/api/generated/model";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

/**
 * The kinds this picker ever offers. `PrincipalKind` (the generated TS union)
 * also has a fourth member, `workspace_members` — that one is deliberately
 * left out.
 *
 * **Hardcoded fact 1/5 (design spec D6):** `workspace_members` exists only
 * for the compiler's own internal assignment rows
 * (`egret-perm-compile::members`) and always 400s if a client authors it
 * over HTTP. The API exposes no flag distinguishing this compiler-internal
 * enum member from the three a human can actually use, so the exclusion has
 * to live here, in code, rather than being derived from the response.
 */
type OfferableKind = "user" | "group" | "role";

/**
 * Mirrors `slugify_localpart` in `crates/egret-auth/src/users.rs` (SP4a D7's
 * measurement: the engine sees a bare username, e.g. Trino's `current_user`
 * answered `admin`, never `admin@egret.local`).
 *
 * **Hardcoded fact 5/5 (design spec, "a user's `ref` is the bare username,
 * never an email"):** `policyAdminListPrincipals` (the endpoint this picker
 * reuses, built for the raw-JSON policy editor) returns a user's email as
 * `Principal.name` — that surface's own doc comment says so. `PermPrincipal`
 * needs the bare username instead, and there is no endpoint that returns an
 * arbitrary user's already-allocated `users.username` (only `ensure_username`
 * exists, and it is not exposed over HTTP). This is therefore a BEST-EFFORT,
 * client-side mirror of the server's own derivation, not a guaranteed match:
 * the server appends `-2`, `-3`, … on a collision, which this function
 * cannot know about. The picker shows the derived value next to the display
 * name (both in the suggestion list and in the chip) so an admin can see
 * exactly what will be stored and catch a mismatch before saving.
 */
export function slugifyLocalpart(email: string): string {
  const local = email.split("@")[0]?.toLowerCase() ?? "";
  const slug = local.replace(/[^a-z0-9_]+/g, "-").replace(/^-+|-+$/g, "");
  return slug || "user";
}

/** What a chip stores a `ref` as, for a given kind. Only `user` needs the
 * email→username translation; a group's Ranger identity is its plain name. */
function refFor(kind: OfferableKind, principalName: string): string {
  return kind === "user" ? slugifyLocalpart(principalName) : principalName;
}

export interface PrincipalPickerProps {
  value: PrincipalSet;
  onChange: (next: PrincipalSet) => void;
  /**
   * True when editing a `Rule.exempt` set rather than a `Grant.principals`
   * subtract.
   *
   * **Hardcoded fact 3/5 (design spec D6):** `exempt.exclude` and a `role`
   * principal inside `exempt` are legal per the schema — `Rule.exempt` is a
   * full `PrincipalSet` — but Ranger's mask and row-filter item types
   * (`MaskItemInput`/`RowFilterItemInput`) have no allow/deny-exception
   * block and no roles bucket, so `egret-perm-compile::rules::compile_rule`
   * refuses both unconditionally (`CompileError::UnexpressibleExemption`).
   * In this mode the picker renders no Exclude section at all and offers
   * only `user`/`group` for Include — the control is absent, not merely
   * disabled or validated after the fact.
   */
  exempt?: boolean;
  disabled?: boolean;
}

/**
 * Edits a `PrincipalSet` — `{include, exclude}` of `{kind, ref}` — for a
 * grant's principal subtract or a rule's exemption set. See `exempt` above
 * for what changes when it is the latter.
 */
export function PrincipalPicker({
  value,
  onChange,
  exempt = false,
  disabled = false,
}: PrincipalPickerProps) {
  const kinds: OfferableKind[] = exempt ? ["user", "group"] : ["user", "group", "role"];

  return (
    <div className="space-y-4">
      <PrincipalListEditor
        label="Include"
        principals={value.include}
        onChange={(include) => onChange({ ...value, include })}
        kinds={kinds}
        disabled={disabled}
      />
      {!exempt && (
        <PrincipalListEditor
          label="Exclude"
          principals={value.exclude}
          onChange={(exclude) => onChange({ ...value, exclude })}
          kinds={kinds}
          disabled={disabled}
        />
      )}
    </div>
  );
}

function PrincipalListEditor({
  label,
  principals,
  onChange,
  kinds,
  disabled,
}: {
  label: string;
  principals: PermPrincipal[];
  onChange: (next: PermPrincipal[]) => void;
  kinds: OfferableKind[];
  disabled?: boolean;
}) {
  const [kind, setKind] = useState<OfferableKind>(kinds[0]);
  const effectiveKind = kinds.includes(kind) ? kind : kinds[0];
  const [q, setQ] = useState("");
  const [open, setOpen] = useState(false);
  const [roleRef, setRoleRef] = useState("");

  const suggestions = useQuery({
    queryKey: ["perm-principals", effectiveKind, q],
    queryFn: async () => {
      const res = await policyAdminListPrincipals({
        q,
        kind: effectiveKind === "role" ? undefined : effectiveKind,
      });
      return res.status === 200 ? res.data : [];
    },
    enabled: open && effectiveKind !== "role",
    retry: false,
  });

  function add(p: PermPrincipal) {
    if (principals.some((x) => x.kind === p.kind && x.ref === p.ref)) return;
    onChange([...principals, p]);
    setQ("");
    setOpen(false);
    setRoleRef("");
  }

  function removeAt(i: number) {
    onChange(principals.filter((_, idx) => idx !== i));
  }

  return (
    <div className="space-y-2">
      <Label className="text-xs font-medium text-muted-foreground">{label}</Label>

      <div className="flex flex-wrap items-center gap-1.5">
        {principals.map((p, i) => (
          <Badge key={`${p.kind}:${p.ref}:${i}`} variant="secondary" className="gap-1.5">
            <span className="text-[10px] font-semibold uppercase text-muted-foreground">
              {p.kind}
            </span>
            {p.ref}
            <button
              type="button"
              aria-label={`Remove ${p.ref} from ${label.toLowerCase()}`}
              onClick={() => removeAt(i)}
              disabled={disabled}
            >
              <X className="h-3 w-3" />
            </button>
          </Badge>
        ))}
        {principals.length === 0 && (
          <span className="text-xs text-muted-foreground">None</span>
        )}
      </div>

      {!disabled && (
        <div className="space-y-1.5">
          <div className="flex gap-2">
            <Select
              value={effectiveKind}
              onValueChange={(v) => setKind(v as OfferableKind)}
            >
              <SelectTrigger className="w-28 shrink-0" aria-label={`${label} kind`}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {kinds.map((k) => (
                  <SelectItem key={k} value={k}>
                    {k}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            {effectiveKind === "role" ? (
              <>
                <Input
                  value={roleRef}
                  onChange={(e) => setRoleRef(e.target.value)}
                  placeholder="Role name"
                  aria-label={`${label} role name`}
                  className="flex-1"
                />
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => add({ kind: "role", ref: roleRef.trim() })}
                  disabled={!roleRef.trim()}
                >
                  Add
                </Button>
              </>
            ) : (
              <div className="relative flex-1">
                <Input
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                  onFocus={() => setOpen(true)}
                  onBlur={() => setTimeout(() => setOpen(false), 150)}
                  placeholder={effectiveKind === "user" ? "Search users…" : "Search groups…"}
                  aria-label={`${label} ${effectiveKind} search`}
                />
                {open && (
                  <div className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1 shadow-md">
                    {(suggestions.data ?? []).map((principal) => {
                      const ref = refFor(effectiveKind, principal.name);
                      return (
                        <button
                          key={`${effectiveKind}-${principal.name}`}
                          type="button"
                          className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                          // Fires before the input's onBlur closes the list.
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => add({ kind: effectiveKind, ref })}
                        >
                          <span>{principal.label}</span>
                          <span className="shrink-0 text-xs text-muted-foreground">
                            stored as {ref}
                          </span>
                        </button>
                      );
                    })}
                    {(suggestions.data ?? []).length === 0 && (
                      <div className="px-2 py-1.5 text-xs text-muted-foreground">
                        {suggestions.isFetching ? "Searching…" : "No matches."}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>

          {effectiveKind === "group" && (
            // Hardcoded fact 2/5 (design spec D3, SP4a D7 measured live): Trino
            // has no group provider, egret writes none, and there is no
            // usersync, so a Ranger policy item naming a group matches nobody.
            // Rendered at the point of choice, not a distant help panel.
            // REMOVE THIS WARNING once egret ships a group provider — that is
            // the only thing that would make it stale.
            <Badge
              variant="warning"
              className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
            >
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
              Not enforced yet: no group provider or usersync, so this won&apos;t match
              inside the engine.
            </Badge>
          )}
        </div>
      )}
    </div>
  );
}
