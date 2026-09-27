import { useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Ban, EyeOff, Filter, HelpCircle, Loader2, Search, ShieldCheck, X } from "lucide-react";

import { usePermExplain } from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { Action, ExplainReason, ExplainResponse, Scope } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge, type badgeVariants } from "@/components/ui/badge";
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
import type { VariantProps } from "class-variance-authority";

const EMPTY_SCOPE: Scope = { catalog_id: "" };

type BadgeVariant = NonNullable<VariantProps<typeof badgeVariants>["variant"]>;

/**
 * One reason KIND's rendering — the class is the deliverable (D9), not the
 * boolean. `no_grant_matched` and `explicit_deny` read deliberately
 * differently: the first means nobody said yes, the second means somebody
 * said no. Ranger's own Access audit collapses both into a single `-`, which
 * is exactly the blindness this page exists to fix.
 */
const REASON_META: Record<
  string,
  { label: string; hint: string; variant: BadgeVariant; icon: typeof ShieldCheck }
> = {
  allowed_by_grant: {
    label: "Allowed by grant",
    hint: "A grant matches this subject and target.",
    variant: "success",
    icon: ShieldCheck,
  },
  explicit_deny: {
    label: "Explicit deny",
    hint: "Somebody forbade this — a deny grant matches. A different administrative problem than no grant matching at all.",
    variant: "destructive",
    icon: Ban,
  },
  no_grant_matched: {
    label: "No grant matched",
    hint: 'Nobody granted this — not the same as a deny. Ranger’s own audit shows "-" for exactly this case.',
    variant: "warning",
    icon: HelpCircle,
  },
  masked_by_rule: {
    label: "Masked by rule",
    hint: "A masking rule applies to this target.",
    variant: "processing",
    icon: EyeOff,
  },
  filtered_by_rule: {
    label: "Filtered by rule",
    hint: "A row-filter rule applies to this target.",
    variant: "secondary",
    icon: Filter,
  },
};

function reasonMeta(kind: string) {
  return (
    REASON_META[kind] ?? {
      label: kind,
      hint: "Unrecognized reason class.",
      variant: "outline" as const,
      icon: HelpCircle,
    }
  );
}

/** One `ExplainReason`, rendered so its class — not `allowed` — is legible on its own. */
function ReasonRow({ reason }: { reason: ExplainReason }) {
  const meta = reasonMeta(reason.kind);
  const Icon = meta.icon;
  return (
    <li className="flex flex-col gap-1.5 rounded-md border border-border p-3 sm:flex-row sm:items-start sm:gap-3">
      <Badge variant={meta.variant} className="w-fit shrink-0 gap-1">
        <Icon className="h-3 w-3" />
        {meta.label}
      </Badge>
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-sm text-muted-foreground">{meta.hint}</p>
        <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs">
          {/* SP4b-1's PATCH keeps a grant/rule's id stable across an edit —
              this link would rot on a delete-and-recreate. */}
          {reason.grant_id && (
            <Link
              to={`/governance/data-permissions?grant=${encodeURIComponent(reason.grant_id)}`}
              className="text-primary hover:underline"
            >
              View grant {reason.grant_id}
            </Link>
          )}
          {reason.rule_id && (
            <Link
              to={`/governance/mask-filter-rules?rule=${encodeURIComponent(reason.rule_id)}`}
              className="text-primary hover:underline"
            >
              View rule {reason.rule_id}
            </Link>
          )}
          {reason.mask_type && (
            <span className="text-muted-foreground">
              mask type <span className="font-mono">{reason.mask_type}</span>
            </span>
          )}
          {reason.filter_expr && (
            <span className="text-muted-foreground">
              filter <span className="font-mono">{reason.filter_expr}</span>
            </span>
          )}
        </div>
      </div>
    </li>
  );
}

/**
 * The subject's single `user` field — a search over the same principal
 * source `PrincipalPicker` uses (`policyAdminListPrincipals`, kind `user`).
 * Stores the allocated `username`, never the email `list_principals` also
 * returns and never a client-side guess — the same fact `PrincipalPicker`
 * encodes (design spec: "a user's `ref` is the bare username, never an
 * email"). Typing a value directly is also accepted (not every operator
 * knows a name to search for), so a principal with no allocated username
 * is shown but not selectable, exactly as `PrincipalPicker` shows it.
 */
function SubjectUserField({
  value,
  onChange,
}: {
  value: string;
  onChange: (username: string) => void;
}) {
  const [open, setOpen] = useState(false);

  const suggestions = useQuery({
    queryKey: ["perm-principals", "user", value],
    queryFn: async () => {
      const res = await policyAdminListPrincipals({ q: value, kind: "user" });
      return res.status === 200 ? res.data : [];
    },
    enabled: open,
    retry: false,
  });

  return (
    <div className="relative space-y-1.5">
      <Label htmlFor="explain-user">User</Label>
      <Input
        id="explain-user"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        placeholder="Search users…"
        autoComplete="off"
      />
      {open && (
        <div className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1 shadow-md">
          {(suggestions.data ?? []).map((p) => {
            const ref = p.username ?? undefined;
            if (!ref) {
              return (
                <div
                  key={p.name}
                  className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm text-muted-foreground"
                >
                  <span>{p.label}</span>
                  <span className="shrink-0 text-xs italic">no allocated username yet</span>
                </div>
              );
            }
            return (
              <button
                key={p.name}
                type="button"
                className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  onChange(ref);
                  setOpen(false);
                }}
              >
                <span>{p.label}</span>
                <span className="shrink-0 text-xs text-muted-foreground">stored as {ref}</span>
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
  );
}

/** Free-text chip list for `groups`/`roles` — unlike `user`, explain accepts any
 *  string a caller asserts the subject belongs to; there is no allocated ref to
 *  resolve. */
function ChipListField({
  label,
  values,
  onChange,
  placeholder,
}: {
  label: string;
  values: string[];
  onChange: (next: string[]) => void;
  placeholder: string;
}) {
  const [input, setInput] = useState("");
  const inputId = `explain-${label.toLowerCase()}`;

  function add() {
    const t = input.trim();
    if (t && !values.includes(t)) onChange([...values, t]);
    setInput("");
  }
  function removeAt(i: number) {
    onChange(values.filter((_, idx) => idx !== i));
  }

  return (
    <div className="space-y-1.5">
      <Label htmlFor={inputId}>{label}</Label>
      {values.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {values.map((v, i) => (
            <Badge key={`${v}-${i}`} variant="secondary" className="gap-1">
              {v}
              <button
                type="button"
                aria-label={`Remove ${v} from ${label.toLowerCase()}`}
                onClick={() => removeAt(i)}
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))}
        </div>
      )}
      <Input
        id={inputId}
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            add();
          }
        }}
        placeholder={placeholder}
      />
    </div>
  );
}

/**
 * Governance → Explain access (D9, SP4b-2 Task 5).
 *
 * Admin-facing: subject + action + target in; the reason CLASS out, not just
 * `allowed`. Reuses `ScopePicker` for the target (Hardcoded facts live there,
 * not here) and `policyAdminListPrincipals` — `PrincipalPicker`'s own data
 * source — for the subject's user field, so the username-vs-email fact
 * (Hardcoded fact 5/5) is never re-derived here.
 *
 * Gated `policy:read` at the route (`App.tsx`), not `policy:update` — this is
 * the one route on this surface a read-only user can call, because `explain`
 * opens no transaction and writes nothing.
 */
export default function ExplainAccess() {
  const [user, setUser] = useState("");
  const [groups, setGroups] = useState<string[]>([]);
  const [roles, setRoles] = useState<string[]>([]);
  const [action, setAction] = useState<Action>("read");
  const [target, setTarget] = useState<Scope>(EMPTY_SCOPE);

  const [result, setResult] = useState<ExplainResponse | null>(null);
  const [explainError, setExplainError] = useState<ApiError | null>(null);

  const explainMutation = usePermExplain<ApiError>();

  const canSubmit = user.trim().length > 0 && target.catalog_id.trim().length > 0;
  /**
   * Why `Explain` is disabled, in words (SP4b-2 finding 2). A disabled button
   * with no explanation is the failure this finding is about: with
   * `policy:read` alone the target picker holds zero options — it says so
   * itself now, and needing `catalog_registry:read` for an admin-tier page's
   * picker is acceptable (D4) — but the button must not simply sit there inert
   * either way.
   */
  const missing = [
    user.trim().length === 0 ? "a user" : null,
    target.catalog_id.trim().length === 0 ? "a target catalog" : null,
  ]
    .filter(Boolean)
    .join(" and ");

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setExplainError(null);
    try {
      const res = await explainMutation.mutateAsync({
        data: {
          subject: {
            user: user.trim(),
            groups: groups.length ? groups : undefined,
            roles: roles.length ? roles : undefined,
          },
          action,
          target,
        },
      });
      setResult(res.status === 200 ? res.data : null);
    } catch (err) {
      setResult(null);
      setExplainError(err as ApiError);
    }
  }

  const forbidden = explainError?.status === 403;

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Explain access</h1>
        <p className="text-sm text-muted-foreground">
          Why a subject can or cannot reach a target — the reason class, not just
          the boolean.
        </p>
      </div>

      <form
        onSubmit={handleSubmit}
        className="grid gap-6 rounded-lg border border-border p-4 md:grid-cols-2 md:p-6"
      >
        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Subject</h2>
          <SubjectUserField value={user} onChange={setUser} />
          <ChipListField label="Groups" values={groups} onChange={setGroups} placeholder="Add a group…" />
          <ChipListField label="Roles" values={roles} onChange={setRoles} placeholder="Add a role…" />

          <div className="space-y-1.5">
            <Label htmlFor="explain-action">Action</Label>
            <Select value={action} onValueChange={(v) => setAction(v as Action)}>
              <SelectTrigger id="explain-action" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="read">read</SelectItem>
                <SelectItem value="write">write</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Target</h2>
          <ScopePicker value={target} onChange={setTarget} />
          <Button type="submit" disabled={!canSubmit || explainMutation.isPending}>
            {explainMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Search className="h-4 w-4" />
            )}
            Explain
          </Button>
          {!canSubmit && (
            <p className="text-xs text-muted-foreground">Explain needs {missing}.</p>
          )}
        </div>
      </form>

      {explainError && (
        <Alert variant="destructive">
          <AlertDescription>
            {forbidden ? (
              <>
                You need the <code>policy:read</code> permission.
              </>
            ) : (
              explainError.message || "Failed to explain access."
            )}
          </AlertDescription>
        </Alert>
      )}

      {result && (
        <div data-testid="explain-result" className="space-y-4 rounded-lg border border-border p-4 md:p-6">
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant={result.allowed ? "success" : "destructive"}>
              {result.allowed ? "Allowed" : "Not allowed"}
            </Badge>
            <span className="text-sm text-muted-foreground">
              — the reason class below is the answer; this is only a summary.
            </span>
          </div>

          <ul className="space-y-2">
            {result.reasons.map((r, i) => (
              <ReasonRow key={`${r.kind}-${r.grant_id ?? r.rule_id ?? i}`} reason={r} />
            ))}
            {result.reasons.length === 0 && (
              <li className="text-sm text-muted-foreground">No reasons returned.</li>
            )}
          </ul>

          <div className="space-y-1.5">
            <Label>Tags considered</Label>
            {result.tags.length > 0 ? (
              <div className="flex flex-wrap gap-1">
                {result.tags.map((t) => (
                  <Badge key={t} variant="outline">
                    {t}
                  </Badge>
                ))}
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">None.</p>
            )}
          </div>

          {/* D9's two honest limits, stated where the answer is — not in a
              help page. A tool that looks like a complete oracle while being
              partial is worse than one that says what it does not know. */}
          <Alert>
            <AlertDescription className="space-y-1 text-xs">
              <p>
                This tool cannot see hand-written Ranger policy: it can answer &quot;not
                allowed&quot; for a subject the engine actually allows through a seeded
                admin policy.
              </p>
              <p>
                Tags populate only when the target names both a schema and a table —
                a catalog-only target always returns none.
              </p>
            </AlertDescription>
          </Alert>
        </div>
      )}
    </div>
  );
}
