import { Plus, Trash2 } from "lucide-react";

import type { Group, Selector, SessionRule } from "@/api/controlPlane";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConfigSection } from "./config/ConfigSection";

// ── raw key→value editor helpers ─────────────────────────────────────────────

export type Kv = { key: string; value: string };

export function recordToKv(rec: Record<string, string> | undefined): Kv[] {
  return rec ? Object.entries(rec).map(([key, value]) => ({ key, value })) : [];
}

export function kvToRecord(rows: Kv[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const row of rows) {
    const key = row.key.trim();
    if (key) out[key] = row.value;
  }
  return out;
}

/** A small list of {key,value} input rows with add/remove. */
export function KeyValueEditor({
  rows,
  onChange,
  keyPlaceholder = "key",
  valuePlaceholder = "value",
  addLabel = "Add property",
}: {
  rows: Kv[];
  onChange: (rows: Kv[]) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
  addLabel?: string;
}) {
  return (
    <div className="space-y-2">
      {rows.map((row, i) => (
        <div key={i} className="flex items-center gap-2">
          <Input
            value={row.key}
            placeholder={keyPlaceholder}
            onChange={(e) =>
              onChange(
                rows.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)),
              )
            }
          />
          <Input
            value={row.value}
            placeholder={valuePlaceholder}
            onChange={(e) =>
              onChange(
                rows.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)),
              )
            }
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="shrink-0 text-muted-foreground"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
            aria-label="Remove row"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...rows, { key: "", value: "" }])}
      >
        <Plus className="h-4 w-4" />
        {addLabel}
      </Button>
    </div>
  );
}

// ── resource groups ──────────────────────────────────────────────────────────

function blankGroup(): Group {
  return { name: "", softMemoryLimit: "", hardConcurrencyLimit: 0, maxQueued: 0 };
}

function blankSelector(): Selector {
  return { group: "" };
}

/** Parse a numeric input to a non-negative integer, defaulting to 0 when blank/invalid. */
function toNum(s: string): number {
  const n = Number(s.trim());
  return Number.isFinite(n) && n >= 0 ? n : 0;
}

/**
 * A single (recursive) resource-group node editor. Renders the group's own
 * fields plus an indented list of `<GroupEditor>`s for its sub-groups.
 */
function GroupEditor({
  value,
  onChange,
  onRemove,
  depth,
}: {
  value: Group;
  onChange: (next: Group) => void;
  onRemove: () => void;
  depth: number;
}) {
  const subGroups = value.subGroups ?? [];
  const set = <K extends keyof Group>(key: K, v: Group[K]) =>
    onChange({ ...value, [key]: v });

  return (
    <div
      className={
        depth > 0
          ? "space-y-3 border-l-2 border-muted pl-4"
          : "space-y-3 rounded-md border p-3"
      }
    >
      <div className="flex items-start gap-2">
        <div className="grid flex-1 grid-cols-2 gap-3 sm:grid-cols-3">
          <div className="space-y-1.5">
            <Label className="text-xs">Name</Label>
            <Input
              value={value.name}
              placeholder="global"
              onChange={(e) => set("name", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">Soft memory limit</Label>
            <Input
              value={value.softMemoryLimit}
              placeholder="80%"
              onChange={(e) => set("softMemoryLimit", e.target.value)}
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">Scheduling policy</Label>
            <Input
              value={value.schedulingPolicy ?? ""}
              placeholder="fair"
              onChange={(e) =>
                set("schedulingPolicy", e.target.value || undefined)
              }
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">Hard concurrency limit</Label>
            <Input
              type="number"
              min={0}
              value={String(value.hardConcurrencyLimit)}
              onChange={(e) => set("hardConcurrencyLimit", toNum(e.target.value))}
            />
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">Max queued</Label>
            <Input
              type="number"
              min={0}
              value={String(value.maxQueued)}
              onChange={(e) => set("maxQueued", toNum(e.target.value))}
            />
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="mt-6 shrink-0 text-muted-foreground"
          onClick={onRemove}
          aria-label="Remove group"
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>

      {subGroups.length > 0 && (
        <div className="space-y-3">
          {subGroups.map((sub, i) => (
            <GroupEditor
              key={i}
              value={sub}
              depth={depth + 1}
              onChange={(next) =>
                set(
                  "subGroups",
                  subGroups.map((g, j) => (j === i ? next : g)),
                )
              }
              onRemove={() =>
                set(
                  "subGroups",
                  subGroups.filter((_, j) => j !== i),
                )
              }
            />
          ))}
        </div>
      )}

      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => set("subGroups", [...subGroups, blankGroup()])}
      >
        <Plus className="h-4 w-4" />
        Add subgroup
      </Button>
    </div>
  );
}

/**
 * Resource-groups builder: a list of root groups (each a recursive tree) plus a
 * list of selectors mapping match criteria to a dot-path group. Emits the wire
 * `{ rootGroups, selectors }` shape; the caller omits it when there are no roots.
 */
export function ResourceGroupsSection({
  value,
  onChange,
}: {
  value?: { rootGroups: Group[]; selectors: Selector[] };
  onChange: (next: { rootGroups: Group[]; selectors: Selector[] }) => void;
}) {
  const rootGroups = value?.rootGroups ?? [];
  const selectors = value?.selectors ?? [];
  const emit = (roots: Group[], sels: Selector[]) =>
    onChange({ rootGroups: roots, selectors: sels });

  return (
    <ConfigSection
      title="Resource groups"
      description="Structured resource-groups.json — hierarchical query admission control (coordinator only)."
      docsUrl="https://trino.io/docs/current/admin/resource-groups.html"
    >
      <div className="space-y-3">
        {rootGroups.map((g, i) => (
          <GroupEditor
            key={i}
            value={g}
            depth={0}
            onChange={(next) =>
              emit(
                rootGroups.map((r, j) => (j === i ? next : r)),
                selectors,
              )
            }
            onRemove={() =>
              emit(
                rootGroups.filter((_, j) => j !== i),
                selectors,
              )
            }
          />
        ))}
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => emit([...rootGroups, blankGroup()], selectors)}
      >
        <Plus className="h-4 w-4" />
        Add root group
      </Button>

      <div className="space-y-2 pt-2">
        <Label className="text-xs font-medium">Selectors</Label>
        {selectors.map((s, i) => {
          const setSel = <K extends keyof Selector>(key: K, v: Selector[K]) =>
            emit(
              rootGroups,
              selectors.map((r, j) => (j === i ? { ...r, [key]: v } : r)),
            );
          return (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={s.user ?? ""}
                placeholder="user"
                onChange={(e) => setSel("user", e.target.value || undefined)}
              />
              <Input
                value={s.source ?? ""}
                placeholder="source"
                onChange={(e) => setSel("source", e.target.value || undefined)}
              />
              <Input
                value={s.queryType ?? ""}
                placeholder="queryType"
                onChange={(e) => setSel("queryType", e.target.value || undefined)}
              />
              <Input
                value={s.group}
                placeholder="global.etl"
                onChange={(e) => setSel("group", e.target.value)}
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  emit(
                    rootGroups,
                    selectors.filter((_, j) => j !== i),
                  )
                }
                aria-label="Remove selector"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          );
        })}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => emit(rootGroups, [...selectors, blankSelector()])}
        >
          <Plus className="h-4 w-4" />
          Add selector
        </Button>
      </div>
    </ConfigSection>
  );
}

// ── session properties ───────────────────────────────────────────────────────

/** Editable form model for a session-property rule (client tags as CSV text). */
export type SessionRuleForm = {
  matchUser: string;
  matchSource: string;
  matchClientTags: string;
  properties: Kv[];
};

export function sessionRulesToForm(rules: SessionRule[] | undefined): SessionRuleForm[] {
  return (rules ?? []).map((r) => ({
    matchUser: r.matchUser ?? "",
    matchSource: r.matchSource ?? "",
    matchClientTags: (r.matchClientTags ?? []).join(", "),
    properties: recordToKv(r.properties),
  }));
}

export function formToSessionRules(forms: SessionRuleForm[]): SessionRule[] {
  return forms
    .map((f) => {
      const rule: SessionRule = { properties: kvToRecord(f.properties) };
      if (f.matchUser.trim()) rule.matchUser = f.matchUser.trim();
      if (f.matchSource.trim()) rule.matchSource = f.matchSource.trim();
      const tags = f.matchClientTags
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
      if (tags.length) rule.matchClientTags = tags;
      return rule;
    })
    .filter(
      (r) =>
        Object.keys(r.properties).length > 0 ||
        r.matchUser !== undefined ||
        r.matchSource !== undefined ||
        r.matchClientTags !== undefined,
    );
}

function blankSessionRule(): SessionRuleForm {
  return { matchUser: "", matchSource: "", matchClientTags: "", properties: [] };
}

/**
 * Session-properties builder: a list of match rules, each applying a set of
 * session properties to matching users/sources/client-tags.
 */
export function SessionPropertiesSection({
  value,
  onChange,
}: {
  value: SessionRuleForm[];
  onChange: (next: SessionRuleForm[]) => void;
}) {
  const set = (i: number, next: SessionRuleForm) =>
    onChange(value.map((r, j) => (j === i ? next : r)));

  return (
    <ConfigSection
      title="Session properties"
      description="Structured session-property-config.json — default session properties applied by match rule."
      docsUrl="https://trino.io/docs/current/admin/session-property-managers.html"
    >
      {value.map((rule, i) => (
        <div key={i} className="space-y-3 rounded-md border p-3">
          <div className="flex items-start gap-2">
            <div className="grid flex-1 grid-cols-1 gap-3 sm:grid-cols-3">
              <div className="space-y-1.5">
                <Label className="text-xs">Match user</Label>
                <Input
                  value={rule.matchUser}
                  placeholder="admin"
                  onChange={(e) => set(i, { ...rule, matchUser: e.target.value })}
                />
              </div>
              <div className="space-y-1.5">
                <Label className="text-xs">Match source</Label>
                <Input
                  value={rule.matchSource}
                  placeholder="etl-pipeline"
                  onChange={(e) => set(i, { ...rule, matchSource: e.target.value })}
                />
              </div>
              <div className="space-y-1.5">
                <Label className="text-xs">Match client tags</Label>
                <Input
                  value={rule.matchClientTags}
                  placeholder="etl, batch"
                  onChange={(e) =>
                    set(i, { ...rule, matchClientTags: e.target.value })
                  }
                />
              </div>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="mt-6 shrink-0 text-muted-foreground"
              onClick={() => onChange(value.filter((_, j) => j !== i))}
              aria-label="Remove rule"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
          <div className="space-y-1.5">
            <Label className="text-xs">Properties</Label>
            <KeyValueEditor
              rows={rule.properties}
              onChange={(rows) => set(i, { ...rule, properties: rows })}
              keyPlaceholder="query_max_memory"
              valuePlaceholder="10GB"
            />
          </div>
        </div>
      ))}

      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...value, blankSessionRule()])}
      >
        <Plus className="h-4 w-4" />
        Add rule
      </Button>
    </ConfigSection>
  );
}
