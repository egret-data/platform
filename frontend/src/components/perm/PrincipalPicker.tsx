import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Users, X } from "lucide-react";

import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { PermPrincipal, Principal, PrincipalSet } from "@/api/generated/model";
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
 * What a chip stores a `ref` as, for a given kind — the value
 * `list_principals` (`crates/egret-policy/src/admin/mod.rs`) actually
 * allocated, never a client-side guess.
 *
 * **Hardcoded fact 5/5 (design spec, "a user's `ref` is the bare username,
 * never an email"):** SP4a D7 measured live that the engine sees a bare
 * username (Trino's `current_user` answered `admin`, never
 * `admin@egret.local`). `Principal.username` now carries that allocated
 * `users.username` directly — minted by `ensure_username`, which lowercases,
 * slugifies, AND appends `-2`/`-3`/… on collision. A client-side slugify of
 * the email can never reproduce the collision suffix, so this picker does
 * not attempt one: a `user` principal with no allocated username is not
 * offered at all (see `suggestions` below).
 *
 * A group's Ranger identity is its plain name, so it has no `username` and
 * needs no translation.
 *
 * Exported because explain's subject field (`SubjectUserField`) resolves the same `ref` from
 * the same `list_principals` response (review M-2 — it had a hand-copy, correct
 * today and exactly the drift the task ordering existed to prevent: a page that
 * stored the email instead would silently answer `no_grant_matched`, a wrong
 * answer that looks like a real one).
 */
export function refFor(kind: OfferableKind, principal: Principal): string | undefined {
  return kind === "user" ? (principal.username ?? undefined) : principal.name;
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
                      const ref = refFor(effectiveKind, principal);
                      // A `user` principal with no allocated `username` (never
                      // triggered `ensure_username`) has no `ref` the engine
                      // can ever see — refuse to offer it rather than
                      // inventing one. See `refFor` above.
                      if (effectiveKind === "user" && !ref) {
                        return (
                          <div
                            key={`${effectiveKind}-${principal.name}`}
                            className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm text-muted-foreground"
                          >
                            <span>{principal.label}</span>
                            <span className="shrink-0 text-xs italic">
                              no allocated username yet
                            </span>
                          </div>
                        );
                      }
                      return (
                        <button
                          key={`${effectiveKind}-${principal.name}`}
                          type="button"
                          className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                          // Fires before the input's onBlur closes the list.
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => add({ kind: effectiveKind, ref: ref as string })}
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
            // J-F3 Task 1b (design R8). SP4a D7 measured that no engine egret
            // provisions sees a group (no group provider, no usersync), so this
            // used to warn "not enforced". The compiler now names the group's
            // MEMBERS in the engine policy instead, and every membership change
            // recompiles — so the fact to show at the point of choice is that
            // the grant follows the membership, not that it does nothing.
            // (`public`, the one group engines resolve themselves, stays a
            // group; the sentence is still true of it in effect.)
            <Badge
              variant="outline"
              className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal"
            >
              <Users className="mt-0.5 h-3 w-3 shrink-0" />
              Applies to the group&apos;s current members: engines see no groups, so each member
              is named in the policy, and adding or removing a member updates it.
            </Badge>
          )}
        </div>
      )}
    </div>
  );
}
