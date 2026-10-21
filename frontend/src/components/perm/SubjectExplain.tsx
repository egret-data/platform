import { useId, useRef, useState, type ReactNode } from "react";
import { keepPreviousData } from "@tanstack/react-query";
import { Loader2, Search } from "lucide-react";
import type { ApiError } from "@/api/client";
import type { Action, ExplainPrincipalResponse, ExplainResponse, Scope } from "@/api/generated/model";
import { permListPeople, usePermExplain, usePermExplainPrincipal, usePermListPeople } from "@/api/generated/perm/perm";
import { ExplainResultView, SubjectUserField } from "@/components/perm/ExplainResult";
import { MembersExplainView } from "@/components/perm/MembersExplainView";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { useDebounced, useSuggestionCombobox } from "@/components/perm/useSuggestionCombobox";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/*
 * Asking explain about a user, a group or a role (DG redesign #2, spec §2,
 * §4.3, P7): the parts the People area's Check access (tab and dialog) and the
 * Data area's Who can read? share. Kept in `components/perm`, outside both
 * lazy areas, so neither imports the other's modules.
 */

export type SubjectKind = "user" | "group" | "role";

/** Who is asked about: a user by engine username, a group or a role by name. */
export type Subject = { kind: SubjectKind; ref: string };

export const SUBJECT_KIND_LABEL: Record<SubjectKind, string> = { user: "User", group: "Group", role: "Role" };

/** `Explain needs a user.`, and so on; a page builds a longer one from {@link subjectNoun}. */
export function subjectNoun(kind: SubjectKind): string {
  return kind === "user" ? "a user" : kind === "group" ? "a group" : "a role";
}

/** A subject kind picker (FE-FORM-3): user, group or role. */
export function SubjectKindField({ value, onChange }: { value: SubjectKind; onChange: (kind: SubjectKind) => void }) {
  const id = useId();
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>Subject</Label>
      <Select value={value} onValueChange={(v) => onChange(v as SubjectKind)}>
        <SelectTrigger id={id} className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {(["user", "group", "role"] as const).map((k) => (
            <SelectItem key={k} value={k}>
              {SUBJECT_KIND_LABEL[k]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/**
 * The subject field for a kind: a user is Explain access's user search
 * (`SubjectUserField`, which stores the engine username); a group or a role is
 * a search over the People list (`GET /perm/people`, `policy:read`), storing
 * the name. Typing a name directly is accepted too. `public` is not offered:
 * it has no members to ask about, and the server refuses it with a sentence.
 */
export function SubjectField({
  kind,
  value,
  onChange,
  listInFlow = false,
}: {
  kind: SubjectKind;
  value: string;
  onChange: (ref: string) => void;
  /** Suggestions in the flow under the field, pushing what follows down instead of covering it. */
  listInFlow?: boolean;
}) {
  if (kind === "user") return <SubjectUserField value={value} onChange={onChange} listInFlow={listInFlow} />;
  return <SubjectNameField kind={kind} value={value} onChange={onChange} listInFlow={listInFlow} />;
}

/**
 * A group or role name: a combobox over the People list, with the keyboard
 * and focus behaviour every governance suggestion field shares
 * (`useSuggestionCombobox`: arrows, Enter, Escape closing only the list, even
 * inside a dialog). The search is sent once typing pauses; a click picks too.
 */
function SubjectNameField({
  kind,
  value,
  onChange,
  listInFlow,
}: {
  kind: "group" | "role";
  value: string;
  onChange: (name: string) => void;
  listInFlow: boolean;
}) {
  const id = useId();
  const q = useDebounced(value);
  // Searched from the first focus on (as the list first opens), never on mount.
  const [enabled, setEnabled] = useState(false);
  const people = usePermListPeople({ kind, q: q || undefined, limit: 20 }, { query: { enabled, retry: false, placeholderData: keepPreviousData } });
  const rows = people.data?.status === 200 ? people.data.data.items.filter((p) => p.key !== "public") : [];
  const boxRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const combo = useSuggestionCombobox({ count: rows.length, onPick: (i) => onChange(rows[i].key), boxRef, inputRef });

  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>{SUBJECT_KIND_LABEL[kind]}</Label>
      <div ref={boxRef} className={listInFlow ? "space-y-1" : "relative"}>
        <Input
          id={id}
          {...combo.inputProps}
          ref={inputRef}
          onFocus={() => {
            setEnabled(true);
            combo.inputProps.onFocus();
          }}
          value={value}
          onChange={(e) => {
            setEnabled(true);
            onChange(e.target.value);
            combo.typed();
          }}
          placeholder={kind === "group" ? "Search groups…" : "Search roles…"}
        />
        {combo.open ? (
          <div
            className={`${listInFlow ? "" : "absolute z-10 mt-1 shadow-md "}max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1`}
            {...combo.popupProps}
          >
            <ul id={combo.listId} role="listbox" aria-label={`${SUBJECT_KIND_LABEL[kind]} suggestions`}>
              {rows.map((p, i) => (
                <li
                  key={p.key}
                  id={combo.optionId(i)}
                  role="option"
                  aria-selected={i === combo.activeRow}
                  className={`flex w-full cursor-pointer items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent ${
                    i === combo.activeRow ? "bg-accent" : ""
                  }`}
                  // Keeps focus in the field, so the list does not close before the click lands.
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => combo.pick(i)}
                  // Options never take focus (the field keeps it and handles the keys); this
                  // only answers an assistive technology that sends Enter to the option itself.
                  onKeyDown={(e) => {
                    if (e.key === "Enter") combo.pick(i);
                  }}
                >
                  <span>{p.label}</span>
                  {p.detail ? <span className="shrink-0 text-xs text-muted-foreground">{p.detail}</span> : null}
                </li>
              ))}
            </ul>
            {rows.length === 0 ? (
              <p className="px-2 py-1.5 text-xs text-muted-foreground">
                {people.isFetching || q !== value
                  ? "Searching…"
                  : people.isError
                    ? "The list could not be loaded; type a name."
                    : "No matches."}
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}

/** Read or write, with what `write` means on each engine. */
export function ActionField({ value, onChange }: { value: Action; onChange: (action: Action) => void }) {
  const id = useId();
  return (
    <div className="space-y-1.5">
      <Label htmlFor={id}>Action</Label>
      <Select value={value} onValueChange={(v) => onChange(v as Action)}>
        <SelectTrigger id={id} className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="read">read</SelectItem>
          <SelectItem value="write">write</SelectItem>
        </SelectContent>
      </Select>
      <p className="text-xs text-muted-foreground">
        Ranger has no <span className="font-mono">write</span>: it is evaluated as insert and delete on Trino, and insert,
        update and delete on StarRocks, each answered separately.
      </p>
    </div>
  );
}

/** One question: who, what, where. */
export type Question = { subject: Subject; action: Action; target: Scope };

/** The identity of a question; an answer or a refusal shows only under the question it was asked for. */
export function questionKey(q: Question): string {
  const t = q.target;
  return JSON.stringify([q.subject.kind, q.subject.ref, q.action, t.catalog_id, t.schema ?? null, t.table ?? null, t.column ?? null]);
}

export type SubjectAnswer =
  | { key: string; kind: "user"; response: ExplainResponse }
  | { key: string; kind: "group" | "role"; response: ExplainPrincipalResponse };

/**
 * Asks explain: a user through `POST /perm/explain`, a group or a role through
 * `POST /perm/explain/principal` (each member's own answer, server-read
 * membership). Each answer and each refusal keeps the question it was asked
 * for, and `answerFor`/`failureFor` return it only for that question: an answer
 * still on its way when the form changes never appears under the new one.
 *
 * Only the latest question asked may settle (`latest`): the two kinds use two
 * mutation hooks, and react-query drops a superseded call's callbacks only
 * within one hook, so a slow user answer would otherwise land after a group's
 * and replace it.
 */
export function useSubjectExplain() {
  const [answer, setAnswer] = useState<SubjectAnswer | null>(null);
  const [failure, setFailure] = useState<{ key: string; error: ApiError } | null>(null);
  const [unknown, setUnknown] = useState<{ key: string; subject: Subject } | null>(null);
  const [pendingKey, setPendingKey] = useState<string | null>(null);
  const user = usePermExplain<ApiError>();
  const members = usePermExplainPrincipal<ApiError>();
  const latest = useRef<number>(0);

  /**
   * `lookUpUser`: a typed user is first looked up by exact username
   * (`GET /perm/people?username=`, the column explain's subject names), since
   * explain answers for any username, known or not (live result finding 3).
   * Only a lookup that answers "none" stops the question; one that cannot be
   * made (refused, failed) asks explain as before, so nothing here claims
   * there is no such user without the server saying so.
   */
  const ask = (q: Question, { lookUpUser = false }: { lookUpUser?: boolean } = {}) => {
    const key = questionKey(q);
    const asked = ++latest.current;
    const current = () => latest.current === asked;
    setAnswer(null);
    setFailure(null);
    setUnknown(null);
    setPendingKey(key);
    const settle = () => setPendingKey((k) => (k === key ? null : k));
    const fail = (error: ApiError) => {
      if (!current()) return;
      setFailure({ key, error });
      settle();
    };
    if (q.subject.kind === "user") {
      const send = () =>
        user.mutate(
          { data: { subject: { user: q.subject.ref }, action: q.action, target: q.target } },
          {
            onSuccess: (res) => {
              if (!current()) return;
              if (res.status === 200) setAnswer({ key, kind: "user", response: res.data });
              settle();
            },
            onError: fail,
          },
        );
      if (!lookUpUser) {
        send();
        return;
      }
      // FE-DATA-2 exception: parameters computed at fetch time (the username being asked about), once per Explain.
      permListPeople({ kind: "user", username: q.subject.ref, limit: 1 }).then(
        (res) => {
          if (!current()) return;
          if (res.status === 200 && res.data.total === 0) {
            setUnknown({ key, subject: q.subject });
            settle();
          } else send();
        },
        () => {
          if (current()) send();
        },
      );
    } else {
      const kind = q.subject.kind;
      members.mutate(
        { data: { principal: { kind, ref: q.subject.ref }, action: q.action, target: q.target } },
        {
          onSuccess: (res) => {
            if (!current()) return;
            if (res.status === 200) setAnswer({ key, kind, response: res.data });
            settle();
          },
          onError: fail,
        },
      );
    }
  };

  return {
    ask,
    /** Whether the question `key` names is on its way. */
    pendingFor: (key: string) => pendingKey === key,
    answerFor: (key: string) => (answer?.key === key ? answer : null),
    failureFor: (key: string) => (failure?.key === key ? failure.error : null),
    /** The typed subject no one has, when the lookup for the question `key` names said so. */
    unknownFor: (key: string) => (unknown?.key === key ? unknown.subject : null),
  };
}

/** `No group named data. Pick a group from the suggestions.` (FE-COPY-2: what happened, and what to do). */
export function noSubjectNamed(kind: SubjectKind, name: string): string {
  const noun = kind === "user" ? "user" : kind === "group" ? "group" : "role";
  return `No ${noun} named ${name}. Pick ${subjectNoun(kind)} from the suggestions.`;
}

/**
 * The server's refusal of a group or role it does not have
 * (`routes/people.rs` `explain_principal`: 400 `no group is named <name>`,
 * after the `bad request: ` prefix), read back into kind and name.
 */
const NO_PRINCIPAL_NAMED = /\bno (group|role) is named (.+)$/;

/** A refused explain, in words: a group or role nobody has, the permission it needs, or the server's sentence. */
export function ExplainFailure({ error }: { error: ApiError }) {
  const unknown = error.status === 400 ? NO_PRINCIPAL_NAMED.exec(error.message) : null;
  return (
    <Alert variant="destructive">
      <AlertDescription>
        {unknown
          ? noSubjectNamed(unknown[1] as SubjectKind, unknown[2])
          : error.status === 403
            ? "Explaining access needs the policy:read permission."
            : error.message || "Access could not be explained. Try again."}
      </AlertDescription>
    </Alert>
  );
}

/** A typed subject no one has: said instead of an answer. */
function UnknownSubject({ subject }: { subject: Subject }) {
  return (
    <Alert variant="destructive">
      <AlertDescription>{noSubjectNamed(subject.kind, subject.ref)}</AlertDescription>
    </Alert>
  );
}

/** A user's answer per profile (`ExplainResultView`), or a group's or role's members (`MembersExplainView`). */
export function SubjectExplainAnswer({ answer }: { answer: SubjectAnswer }) {
  if (answer.kind === "user") return <ExplainResultView result={answer.response} />;
  return <MembersExplainView response={answer.response} kind={answer.kind} />;
}

const EMPTY_SCOPE: Scope = { catalog_id: "" };

/**
 * The whole question-and-answer: whatever of subject, action and target the
 * caller does not fix is a field (subject kind and subject, `ActionField`,
 * `ScopePicker`), then Explain, then the refusal or the answer for exactly the
 * question on screen (`useSubjectExplain`). Explain is `aria-disabled` until
 * the question is complete, says what is missing (FE-ACTION-4) through
 * `aria-describedby`, and stays in the tab order (N22); a press while blocked
 * does nothing. A typed user is looked up first (`useSubjectExplain`'s
 * `lookUpUser`); a page's own subject is not.
 */
export function SubjectExplainForm({
  subject: fixedSubject,
  action: fixedAction,
  target: fixedTarget,
  initialKind = "user",
  layout,
}: {
  /** A principal page's own subject; absent, the form asks for one. */
  subject?: Subject;
  action?: Action;
  target?: Scope;
  initialKind?: SubjectKind;
  /** Lays the fields out; gets the subject, action-and-target and submit groups. Default: stacked. */
  layout?: (parts: { subject: ReactNode; action: ReactNode; target: ReactNode; submit: ReactNode }) => ReactNode;
}) {
  const needsId = useId();
  const [kind, setKind] = useState<SubjectKind>(fixedSubject?.kind ?? initialKind);
  const [ref, setRef] = useState("");
  const [action, setAction] = useState<Action>(fixedAction ?? "read");
  const [target, setTarget] = useState<Scope>(fixedTarget ?? EMPTY_SCOPE);
  const explain = useSubjectExplain();

  const subject: Subject = fixedSubject ?? { kind, ref: ref.trim() };
  const question: Question = { subject, action: fixedAction ?? action, target: fixedTarget ?? target };
  const key = questionKey(question);
  const missing = [
    subject.ref ? null : subjectNoun(subject.kind),
    question.target.catalog_id.trim() ? null : "a target catalog",
  ]
    .filter(Boolean)
    .join(" and ");
  const pending = explain.pendingFor(key);
  const failure = explain.failureFor(key);
  const unknown = explain.unknownFor(key);
  const answer = explain.answerFor(key);
  const blocked = missing !== "" || pending;

  const parts = {
    subject: fixedSubject ? null : (
      <>
        <SubjectKindField
          value={kind}
          onChange={(k) => {
            setKind(k);
            setRef("");
          }}
        />
        {/* In flow: an overlaid list covered Explain at 390 px (live result §5). */}
        <SubjectField kind={kind} value={ref} onChange={setRef} listInFlow />
      </>
    ),
    action: fixedAction ? null : <ActionField value={action} onChange={setAction} />,
    target: fixedTarget ? null : <ScopePicker value={target} onChange={setTarget} />,
    submit: (
      <>
        <Button
          type="submit"
          aria-disabled={blocked || undefined}
          aria-describedby={missing ? needsId : undefined}
          onClick={(e) => {
            if (blocked) e.preventDefault();
          }}
        >
          {pending ? <Loader2 aria-hidden className="animate-spin" /> : <Search aria-hidden />}
          Explain
        </Button>
        {missing ? (
          <p id={needsId} className="w-full text-xs text-muted-foreground">
            {`Explain needs ${missing}.`}
          </p>
        ) : null}
      </>
    ),
  };

  return (
    <div className="space-y-4">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!blocked) explain.ask(question, { lookUpUser: !fixedSubject });
        }}
      >
        {layout ? (
          layout(parts)
        ) : (
          <div className="space-y-4">
            {parts.subject}
            {parts.action}
            {parts.target}
            <div className="flex flex-wrap items-center gap-2">{parts.submit}</div>
          </div>
        )}
      </form>
      {failure ? <ExplainFailure error={failure} /> : null}
      {unknown ? <UnknownSubject subject={unknown} /> : null}
      {answer ? <SubjectExplainAnswer answer={answer} /> : null}
    </div>
  );
}
