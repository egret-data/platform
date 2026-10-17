import { useEffect, useId, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Loader2, Users, X } from "lucide-react";

import { ApiError } from "@/api/client";
import {
  getPermListPeopleQueryKey,
  getPermReadUserQueryKey,
  permAllocateUsername,
  permListPeople,
} from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type {
  PermPrincipal,
  Principal,
  PrincipalSet,
  StaleUserRefsConflict,
  StoredUserRef,
} from "@/api/generated/model";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";

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
 * not attempt one: a `user` principal with no allocated username is never
 * offered as a pick; it is offered for allocation through the server
 * (`PrincipalSearchField`, D9b), and added under the name the server returns.
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
  const [roleRef, setRoleRef] = useState("");

  function add(p: PermPrincipal) {
    if (principals.some((x) => x.kind === p.kind && x.ref === p.ref)) return;
    onChange([...principals, p]);
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
              <PrincipalSearchField
                key={effectiveKind}
                kind={effectiveKind}
                label={label}
                principals={principals}
                onAdd={add}
              />
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

/** How long typing pauses before a search is sent (as `SubjectNameField` in `SubjectExplain.tsx`). */
const SEARCH_DEBOUNCE_MS = 200;

/** What the field says under itself after an allocation: the stored name, or the refusal (FE-FORM-4). */
type AllocationNotice =
  | { kind: "stored"; label: string; username: string }
  | { kind: "refused"; message: string; refs: StoredUserRef[] };

/** The stored grant principals or rule exemptions a 409 from the allocation route lists, if it is one. */
function staleRefsOf(e: unknown): StoredUserRef[] {
  if (!(e instanceof ApiError) || e.status !== 409) return [];
  const refs = (e.body as Partial<StaleUserRefsConflict> | undefined)?.refs;
  return Array.isArray(refs) ? refs : [];
}

/**
 * The user id of a `list_principals` user row, which carries only the email
 * (`name`): the People list's row whose email is exactly it. Emails are
 * unique; `q` is a substring match, so every other row is dropped.
 */
async function findUserId(principal: Principal): Promise<string> {
  // FE-DATA-2 exception: parameters computed at fetch time (the picked row's email, once allocation is confirmed).
  const res = await permListPeople({ kind: "user", q: principal.name, includeServiceAccounts: true, limit: 200 });
  const email = principal.name.toLowerCase();
  const matches = res.status === 200 ? res.data.items.filter((row) => row.detail?.toLowerCase() === email) : [];
  if (matches.length !== 1) {
    throw new Error(`${principal.label} could not be found among People. Search again, then allocate.`);
  }
  return matches[0].key;
}

/**
 * A user or group search: a combobox (ARIA 1.2, the pattern of
 * `SubjectNameField` in `SubjectExplain.tsx`) over `list_principals`.
 *
 * D9a (spec P12): the list stays open after a pick while the field has focus,
 * so several principals are picked in a row; typing or a click reopens it,
 * Escape closes it. Arrow keys move through the suggestions
 * (`aria-activedescendant`, focus stays in the field) and Enter picks one.
 *
 * D9b (spec P13, Q1): a user with no username is listed apart, outside the
 * listbox. A viewer holding `policy:update` (what every grant and rule write
 * needs, and what the route checks) gets "Allocate username and add": after a
 * confirmation, `POST /perm/people/users/{id}/username` allocates the name
 * egret would allocate at the user's first query, and the user is added under
 * the name the server returned. A 409 (a stored grant principal or rule
 * exemption already names that username) is shown beside the field in the
 * server's words with links to those rows, and nothing is added.
 */
function PrincipalSearchField({
  kind,
  label,
  principals,
  onAdd,
}: {
  kind: "user" | "group";
  label: string;
  principals: PermPrincipal[];
  onAdd: (p: PermPrincipal) => void;
}) {
  const id = useId();
  const listId = `${id}-list`;
  const noticeId = `${id}-notice`;
  const boxRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const skipNextOpen = useRef(false);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [q, setQ] = useState("");
  const [sent, setSent] = useState("");
  const [confirming, setConfirming] = useState<Principal | null>(null);
  const [notice, setNotice] = useState<AllocationNotice | null>(null);
  const can = usePolicyPermissions();
  const canAllocate = kind === "user" && can("policy:update");
  const queryClient = useQueryClient();

  useEffect(() => {
    const t = setTimeout(() => setSent(q), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [q]);

  const suggestions = useQuery({
    queryKey: ["perm-principals", kind, sent],
    queryFn: async () => {
      const res = await policyAdminListPrincipals({ q: sent, kind });
      return res.status === 200 ? res.data : [];
    },
    enabled: open,
    retry: false,
  });
  const rows = suggestions.data ?? [];
  // A `user` principal with no allocated `username` has no `ref` the engine
  // can ever see: it is never offered as a pick (see `refFor`), only for allocation.
  const pickable = rows.flatMap((principal) => {
    const ref = refFor(kind, principal);
    return ref ? [{ principal, ref }] : [];
  });
  const unallocated = rows.filter((principal) => !refFor(kind, principal));
  const activeRow = open && active >= 0 && active < pickable.length ? active : -1;
  const isAdded = (ref: string) => principals.some((p) => p.kind === kind && p.ref === ref);

  const close = () => {
    setOpen(false);
    setActive(-1);
  };

  // Escape closes the list and nothing else. An enclosing Radix dialog (the
  // grant and rule dialogs) listens for Escape in the document's capture
  // phase, before any handler on the field, and dismisses itself unless the
  // event is already `defaultPrevented`; a window capture listener runs first
  // and prevents it.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !(e.target instanceof Node) || !boxRef.current?.contains(e.target)) return;
      e.preventDefault();
      setOpen(false);
      setActive(-1);
      inputRef.current?.focus();
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [open]);

  // Focus moving between the field and the list (Tab to an allocation button) keeps it open.
  const closeUnlessInside = (next: EventTarget | null) => {
    if (next instanceof Node && boxRef.current?.contains(next)) return;
    close();
  };

  const pick = (ref: string) => {
    onAdd({ kind, ref });
    setNotice(null);
  };

  // The latest `onAdd`: an allocation settles after the chips may have changed.
  const onAddRef = useRef(onAdd);
  useEffect(() => {
    onAddRef.current = onAdd;
  });

  const allocate = useMutation({
    mutationFn: async (principal: Principal) => {
      const userId = await findUserId(principal);
      // FE-DATA-2 exception: parameters computed at fetch time (the id `findUserId` just read).
      const res = await permAllocateUsername(userId);
      if (res.status !== 200) throw new Error("The username could not be allocated. Try again.");
      return { userId, username: res.data.username, principal };
    },
    onSuccess: ({ userId, username, principal }) => {
      onAddRef.current({ kind: "user", ref: username });
      setNotice({ kind: "stored", label: principal.label, username });
      setConfirming(null);
      void queryClient.invalidateQueries({ queryKey: ["perm-principals"] });
      void queryClient.invalidateQueries({ queryKey: getPermListPeopleQueryKey() });
      void queryClient.invalidateQueries({ queryKey: getPermReadUserQueryKey(userId) });
    },
    onError: (e) => {
      setNotice({
        kind: "refused",
        message: refusalMessage(e, "policy:update", "The username could not be allocated. Try again."),
        refs: staleRefsOf(e),
      });
      setConfirming(null);
    },
  });

  return (
    <div className="flex-1 space-y-1.5">
      <div ref={boxRef} className="relative">
        <Input
          ref={inputRef}
          role="combobox"
          aria-label={`${label} ${kind} search`}
          aria-expanded={open}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={activeRow >= 0 ? `${listId}-${activeRow}` : undefined}
          aria-describedby={notice ? noticeId : undefined}
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setOpen(true);
            setActive(-1);
          }}
          onFocus={() => {
            if (skipNextOpen.current) skipNextOpen.current = false;
            else setOpen(true);
          }}
          onClick={() => setOpen(true)}
          onBlur={(e) => closeUnlessInside(e.relatedTarget)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setOpen(true);
              if (pickable.length === 0) return;
              const step = e.key === "ArrowDown" ? 1 : -1;
              setActive((i) =>
                i < 0 ? (step > 0 ? 0 : pickable.length - 1) : (i + step + pickable.length) % pickable.length,
              );
            } else if (e.key === "Enter" && activeRow >= 0) {
              // Picks the suggestion instead of submitting the dialog's form.
              e.preventDefault();
              pick(pickable[activeRow].ref);
            }
          }}
          placeholder={kind === "user" ? "Search users…" : "Search groups…"}
          autoComplete="off"
        />
        {open ? (
          <div
            className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1 shadow-md"
            onBlur={(e) => closeUnlessInside(e.relatedTarget)}
          >
            {pickable.length > 0 ? (
              <ul
                id={listId}
                role="listbox"
                aria-multiselectable="true"
                aria-label={`${label} ${kind} suggestions`}
              >
                {pickable.map(({ principal, ref }, i) => {
                  const added = isAdded(ref);
                  return (
                    <li
                      key={principal.name}
                      id={`${listId}-${i}`}
                      role="option"
                      aria-selected={added}
                      className={`flex w-full cursor-pointer items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent ${
                        i === activeRow ? "bg-accent" : ""
                      }`}
                      // Keeps focus in the field, so the list stays open for the next pick.
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => pick(ref)}
                      // Options never take focus (the field keeps it and handles the keys); this
                      // only answers an assistive technology that sends Enter to the option itself.
                      onKeyDown={(e) => {
                        if (e.key === "Enter") pick(ref);
                      }}
                    >
                      <span className="flex items-center gap-1.5">
                        {added ? <Check aria-hidden className="h-3 w-3 shrink-0 text-primary" /> : null}
                        {principal.label}
                      </span>
                      <span className="shrink-0 text-xs text-muted-foreground">
                        {added ? `added, stored as ${ref}` : kind === "user" ? `stored as ${ref}` : null}
                      </span>
                    </li>
                  );
                })}
              </ul>
            ) : null}
            {unallocated.length > 0 ? (
              <ul role="list" aria-label="Users with no username yet" className={pickable.length > 0 ? "mt-1 border-t border-border pt-1" : ""}>
                {unallocated.map((principal) => (
                  <li
                    key={principal.name}
                    className="flex w-full flex-wrap items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm text-muted-foreground"
                  >
                    <span>{principal.label}</span>
                    {canAllocate ? (
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        className="h-7 shrink-0 text-xs"
                        aria-label={`Allocate username and add ${principal.label}`}
                        // Keeps focus in the field until the confirmation takes it.
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => {
                          setNotice(null);
                          setConfirming(principal);
                        }}
                      >
                        Allocate username and add
                      </Button>
                    ) : (
                      <span className="flex flex-col items-end text-right">
                        <span className="shrink-0 text-xs italic">no allocated username yet</span>
                        <span className="text-xs">
                          egret allocates one at their first query; an account with policy:update can allocate
                          one here.
                        </span>
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            ) : null}
            {rows.length === 0 ? (
              <div id={listId} className="px-2 py-1.5 text-xs text-muted-foreground">
                {suggestions.isFetching || q !== sent
                  ? "Searching…"
                  : suggestions.isError
                    ? "The list could not be loaded. Search again."
                    : "No matches."}
              </div>
            ) : null}
          </div>
        ) : null}
      </div>

      {notice?.kind === "stored" ? (
        <p id={noticeId} role="status" className="text-xs text-muted-foreground">
          {`${notice.label} was allocated a username and added, stored as ${notice.username}.`}
        </p>
      ) : null}
      {notice?.kind === "refused" ? (
        <div id={noticeId} role="alert" className="space-y-1 text-xs text-destructive">
          <p>{notice.message}</p>
          {notice.refs.length > 0 ? (
            <ul role="list" className="space-y-0.5">
              {notice.refs.map((r) => (
                <li key={`${r.source}:${r.id}:${r.position}:${r.ref}`}>
                  <Link
                    to={`/governance/${r.source === "rule" ? "rules" : "grants"}/${encodeURIComponent(r.id)}`}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="font-medium text-primary hover:underline"
                  >
                    {`${r.source === "rule" ? "rule" : "grant"} ${r.id}`}
                    <span className="sr-only"> (opens in a new tab)</span>
                  </Link>
                  <span className="text-muted-foreground">{` · ${r.position} names ${r.ref}`}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}

      <AlertDialog
        open={confirming !== null}
        onOpenChange={(next) => {
          if (!next && !allocate.isPending) setConfirming(null);
        }}
      >
        <AlertDialogContent
          onCloseAutoFocus={(e) => {
            // Back to the field, with the list closed so the outcome under it is readable.
            e.preventDefault();
            skipNextOpen.current = true;
            inputRef.current?.focus();
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{`Allocate a username to ${confirming?.label ?? ""}?`}</AlertDialogTitle>
            <AlertDialogDescription>
              egret gives this user the engine username it would allocate at their first query, and adds it
              here. Engines then know them by that name. The username is kept even if you close this form
              without saving.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={allocate.isPending}>Cancel</AlertDialogCancel>
            <Button
              type="button"
              disabled={allocate.isPending}
              onClick={() => {
                if (confirming) allocate.mutate(confirming);
              }}
            >
              {allocate.isPending ? <Loader2 aria-hidden className="animate-spin" /> : null}
              Allocate and add
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
