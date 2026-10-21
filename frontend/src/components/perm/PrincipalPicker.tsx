import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import { Link } from "react-router-dom";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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
import { useDebounced, useSuggestionCombobox } from "@/components/perm/useSuggestionCombobox";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { TERM } from "@/routes/governance/glossary";

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
  /**
   * Edits the Include list only (an engine grant's principals, spec EP21):
   * no Exclude list is rendered, and `user`/`group`/`role` stay offered.
   */
  includeOnly?: boolean;
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
  includeOnly = false,
  disabled = false,
}: PrincipalPickerProps) {
  const kinds: OfferableKind[] = exempt ? ["user", "group"] : ["user", "group", "role"];

  return (
    <div className="space-y-4">
      <PrincipalListEditor
        label={exempt ? TERM.exempt : "Include"}
        principals={value.include}
        onChange={(include) => onChange({ ...value, include })}
        kinds={kinds}
        disabled={disabled}
        listInFlow={exempt}
        searchAll={exempt}
      />
      {!exempt && !includeOnly && (
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
  listInFlow = false,
  searchAll = false,
}: {
  label: string;
  principals: PermPrincipal[];
  onChange: (next: PermPrincipal[]) => void;
  kinds: OfferableKind[];
  disabled?: boolean;
  /**
   * A rule's exemptions (Task 13): no kind switch. One field searches users
   * and groups at once and each result says its kind, so a group is one
   * search and one pick, not a switch of kind first.
   */
  searchAll?: boolean;
  /**
   * N26 (spec 2026-10-21-classification-ux C8.5), a rule's exemptions only:
   * the help line sits under the search field and the suggestions follow it
   * in the flow, instead of a popup floating over the line that explains the
   * choice.
   */
  listInFlow?: boolean;
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

  // J-F3 Task 1b (design R8). SP4a D7 measured that no engine egret
  // provisions sees a group (no group provider, no usersync), so this
  // used to warn "not enforced". The compiler now names the group's
  // MEMBERS in the engine policy instead, and every membership change
  // recompiles — so the fact to show at the point of choice is that
  // the grant follows the membership, not that it does nothing.
  // (`public`, the one group engines resolve themselves, stays a
  // group; the sentence is still true of it in effect.)
  const groupHelp =
    effectiveKind === "group" || searchAll ? (
      <Badge variant="outline" className="w-fit items-start gap-1.5 whitespace-normal text-left font-normal">
        <Users className="mt-0.5 h-3 w-3 shrink-0" />
        {searchAll ? "A group applies to its current members" : "Applies to the group’s current members"}: engines see no
        groups, so each member is named in the policy, and adding or removing a member updates it.
      </Badge>
    ) : null;

  return (
    <div className="space-y-2">
      {/* With one field for every kind, the field's own label names the list (the form's label above it). */}
      {searchAll ? null : <Label className="text-xs font-medium text-muted-foreground">{label}</Label>}

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
            {searchAll ? (
              <PrincipalSearchField
                kind="any"
                label={label}
                principals={principals}
                onAdd={add}
                help={groupHelp}
                listInFlow={listInFlow}
              />
            ) : (
            <>
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
                help={listInFlow ? groupHelp : undefined}
                listInFlow={listInFlow}
              />
            )}
            </>
            )}
          </div>

          {listInFlow ? null : groupHelp}
        </div>
      )}
    </div>
  );
}

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
 * unique; `q` is a substring match (on name, email and username), so every
 * other row is dropped. A service account's row shows `service account`
 * instead of its email (final review minor d), so one is matched by its label
 * among the service-account rows `q` found by that email.
 */
async function findUserId(principal: Principal): Promise<string> {
  let res: Awaited<ReturnType<typeof permListPeople>>;
  try {
    // FE-DATA-2 exception: parameters computed at fetch time (the picked row's email, once allocation is confirmed).
    res = await permListPeople({ kind: "user", q: principal.name, includeServiceAccounts: true, limit: 200 });
  } catch (e) {
    // The lookup is `GET /perm/people`, which needs policy:read; the allocation's own refusal names policy:update.
    if (e instanceof ApiError && e.status === 403) {
      throw new Error("This account cannot look the user up: it needs the policy:read permission.", { cause: e });
    }
    throw e;
  }
  const email = principal.name.toLowerCase();
  const items = res.status === 200 ? res.data.items : [];
  const byEmail = items.filter((row) => !row.serviceAccount && row.detail?.toLowerCase() === email);
  const matches = byEmail.length > 0 ? byEmail : items.filter((row) => row.serviceAccount && row.label === principal.label);
  if (matches.length !== 1) {
    throw new Error(
      `${principal.label} could not be found among People. Search again, then allocate. A service account that cannot be found here gets its username at its first token.`,
    );
  }
  return matches[0].key;
}

/**
 * A user or group search: a combobox (ARIA 1.2, `useSuggestionCombobox`, as
 * every governance suggestion field) over `list_principals`.
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
  help,
  listInFlow = false,
}: {
  /** `any`: users and groups at once, each result naming its kind (a rule's exemptions). */
  kind: "user" | "group" | "any";
  label: string;
  principals: PermPrincipal[];
  onAdd: (p: PermPrincipal) => void;
  /** Rendered under the field and before the suggestions (see `listInFlow` on `PrincipalListEditor`). */
  help?: ReactNode;
  listInFlow?: boolean;
}) {
  const noticeId = `${useId()}-notice`;
  const [q, setQ] = useState("");
  const sent = useDebounced(q);
  // Searched from the first focus on (as the list first opens), never on mount.
  const [enabled, setEnabled] = useState(false);
  const [confirming, setConfirming] = useState<Principal | null>(null);
  const [notice, setNotice] = useState<AllocationNotice | null>(null);
  const can = usePolicyPermissions();
  const canAllocate = kind !== "group" && can("policy:update");
  const queryClient = useQueryClient();

  const suggestions = useQuery({
    queryKey: ["perm-principals", kind, sent],
    queryFn: async () => {
      // No `kind` asks for users and groups (`list_principals`).
      const res = await policyAdminListPrincipals(kind === "any" ? { q: sent } : { q: sent, kind });
      const rows = res.status === 200 ? res.data : [];
      // Only the kinds a human may pick: never anything else the server may add.
      return kind === "any" ? rows.filter((p) => p.kind === "user" || p.kind === "group") : rows;
    },
    enabled,
    retry: false,
    // The previous search's rows stay while the next loads, so an option never vanishes under the pointer.
    placeholderData: keepPreviousData,
  });
  const rows = suggestions.data ?? [];
  // A `user` principal with no allocated `username` has no `ref` the engine
  // can ever see: it is never offered as a pick (see `refFor`), only for allocation.
  const kindOf = (principal: Principal): "user" | "group" =>
    kind === "any" ? (principal.kind === "group" ? "group" : "user") : kind;
  const pickable = rows.flatMap((principal) => {
    const ref = refFor(kindOf(principal), principal);
    return ref ? [{ principal, ref, kind: kindOf(principal) }] : [];
  });
  const unallocated = rows.filter((principal) => !refFor(kindOf(principal), principal));
  const isAdded = (k: "user" | "group", ref: string) => principals.some((p) => p.kind === k && p.ref === ref);

  // The shared keyboard and focus behaviour (Escape closes only the list, even
  // in the grant and rule dialogs). Focus moving between the field and the
  // popup (Tab to an allocation button) keeps it open, and so does a pick.
  const boxRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const combo = useSuggestionCombobox({
    count: pickable.length,
    closeOnPick: false,
    boxRef,
    inputRef,
    onPick: (i) => {
      onAdd({ kind: pickable[i].kind, ref: pickable[i].ref });
      setNotice(null);
    },
  });

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
      // FE-ACTION-2: the form stays open, so a toast confirms it too; the line under the field keeps the name.
      toast.success(`${principal.label} was allocated the username ${username} and added.`);
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
      <div ref={boxRef} className={listInFlow ? "space-y-1.5" : "relative"}>
        <Input
          {...combo.inputProps}
          ref={inputRef}
          aria-label={kind === "any" ? `${label} search` : `${label} ${kind} search`}
          aria-describedby={notice ? noticeId : undefined}
          onFocus={() => {
            setEnabled(true);
            combo.inputProps.onFocus();
          }}
          value={q}
          onChange={(e) => {
            setEnabled(true);
            setQ(e.target.value);
            combo.typed();
          }}
          placeholder={kind === "any" ? "Search users and groups…" : kind === "user" ? "Search users…" : "Search groups…"}
        />
        {help}
        {combo.open ? (
          <div
            className={`${listInFlow ? "" : "absolute z-10 mt-1 shadow-md "}max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1`}
            {...combo.popupProps}
          >
            <ul
              id={combo.listId}
              role="listbox"
              aria-multiselectable="true"
              aria-label={kind === "any" ? `${label} suggestions` : `${label} ${kind} suggestions`}
            >
              {pickable.map(({ principal, ref, kind: rowKind }, i) => {
                const added = isAdded(rowKind, ref);
                const stored = added ? `added, stored as ${ref}` : rowKind === "user" ? `stored as ${ref}` : null;
                // With every kind listed, each row says which kind it is.
                const kindText = kind === "any" ? (rowKind === "group" ? "Group" : "User") : null;
                return (
                  <li
                    key={`${rowKind}:${principal.name}`}
                    id={combo.optionId(i)}
                    role="option"
                    aria-selected={added}
                    className={`flex w-full cursor-pointer items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent ${
                      i === combo.activeRow ? "bg-accent" : ""
                    }`}
                    // Keeps focus in the field, so the list stays open for the next pick.
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => combo.pick(i)}
                    // Options never take focus (the field keeps it and handles the keys); this
                    // only answers an assistive technology that sends Enter to the option itself.
                    onKeyDown={(e) => {
                      if (e.key === "Enter") combo.pick(i);
                    }}
                  >
                    <span className="flex items-center gap-1.5">
                      {added ? <Check aria-hidden className="h-3 w-3 shrink-0 text-primary" /> : null}
                      {principal.label}
                    </span>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {[kindText, stored].filter(Boolean).join(" · ")}
                    </span>
                  </li>
                );
              })}
            </ul>
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
              <p className="px-2 py-1.5 text-xs text-muted-foreground">
                {suggestions.isFetching || q !== sent
                  ? "Searching…"
                  : suggestions.isError
                    ? "The list could not be loaded. Search again."
                    : "No matches."}
              </p>
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
            combo.skipNextFocusOpen();
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
