import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Tag as TagIcon, UserSearch } from "lucide-react";
import { usePermListGrants, usePermListRules } from "@/api/generated/perm/perm";
import type { GovernanceCatalog, GrantView, ResourceTagDto, RuleView } from "@/api/generated/model";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { useWorkspaceNames } from "@/routes/governance/openInData";
import { formatPrincipals, formatRuleKind, grantSentence } from "@/routes/governance/permFormat";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { AddTagDialog, TAGS_EGRET_ONLY, type TagTarget } from "@/routes/governance/data/AddTagDialog";
import { grantCoverage, patternLevels, ruleSelects, sameName, tagNamesOn } from "@/routes/governance/data/coverage";
import { assignmentsOf, fullPath, tagReach, type BrowseResult, type CatalogState, type TagsResult } from "@/routes/governance/data/objectData";
import type { ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate, Section, TabError } from "@/routes/governance/data/tabs/TabStates";
import { WhoCanReadDialog } from "@/routes/governance/data/WhoCanReadDialog";

/**
 * What a viewer without `policy:read` reads instead of the grants and rules.
 * Also this module's bundle marker (`scripts/assert-governance-data-lazy.mjs`).
 */
export const PANEL_NEEDS_POLICY_READ = "Reading grants and rules needs the policy:read permission.";

/** A column of a table, view or materialized view: `path` with its `column` set. */
type ColumnPath = ObjectPath & { schema: string; object: string; column: string };

/**
 * The column panel (spec §2.2): a right-hand sheet that `?column=` opens, from
 * a column's name in the Overview or a link. It is a Radix dialog, so focus is
 * trapped in it and Escape closes it; closing removes `?column=` and keeps the
 * rest of the search.
 *
 * It opens from a link, not a Radix trigger, so Radix has nowhere to return
 * focus on close and would leave it on `<body>` (D6). The panel remembers
 * what held focus when it opened (the column's link) and returns focus there
 * on close, however focus moved inside the panel meanwhile; when it opened
 * from the URL, focus goes to the Overview's link for the column, else to the
 * page's heading.
 *
 * It lists, for this one column:
 * - **Tags**: its own, then its table's marked "from table" (a table's tag
 *   reaches its columns, `tagNamesOn`); only an egret catalog carries tags (F9).
 * - **Masking rules** that select it, by path or by a tag it carries, with
 *   their exemptions (`ruleSelects`).
 * - **Grants** set on it (column level, by name or by a pattern that matches
 *   it, `grantCoverage` "here"); the grants on the table and above are the
 *   Permissions tab's.
 *
 * F10: these are lists of what selects the column, never who sees what. Who
 * can read it is explain's answer, from "Who can read this column?".
 */
export function ColumnPanel({
  path,
  catalog,
  browse,
  tags,
  onClose,
}: {
  path: ObjectPath;
  catalog: CatalogState;
  browse: BrowseResult;
  tags: TagsResult;
  onClose: () => void;
}) {
  const asked = path.column ?? "";
  const check = checkColumn(browse, asked);
  // The source's own spelling once it is known: the URL may differ in case.
  const column = check.status === "found" ? check.name : asked;
  const objectPath: ObjectPath = { catalog: path.catalog, schema: path.schema, object: path.object };
  const open = path.object !== undefined && path.column !== undefined;
  // The last column shown: by the time the panel closes, `?column=` is gone from the URL.
  const [lastColumn, setLastColumn] = useState(asked);
  if (asked !== "" && asked !== lastColumn) setLastColumn(asked);
  // What held focus when the panel opened, read before the sheet mounts and
  // its focus scope moves focus inside; only on opening.
  const opener = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    if (!open) return;
    const active = document.activeElement;
    opener.current = active instanceof HTMLElement && active !== document.body ? active : null;
  }, [open]);
  const returnFocus = (e: Event) => {
    e.preventDefault();
    const element = opener.current?.isConnected ? opener.current : null;
    opener.current = null;
    const target =
      element ??
      [...document.querySelectorAll<HTMLElement>("[data-column-link]")].find((a) => sameName(a.dataset.columnLink ?? "", lastColumn)) ??
      document.querySelector<HTMLElement>("h1");
    target?.focus();
  };

  let body: ReactNode = null;
  if (path.schema !== undefined && path.object !== undefined && path.column !== undefined) {
    if (check.status === "loading") body = <ListSkeleton rows={4} cols={2} />;
    else if (check.status === "unchecked") {
      body = (
        <Alert>
          <AlertDescription>{`“${asked}” could not be checked: the columns of ${fullPath(objectPath)} could not be listed.`}</AlertDescription>
        </Alert>
      );
    } else if (check.status === "missing") {
      body = (
        <Alert>
          <AlertDescription>{`No column named “${asked}” in ${fullPath(objectPath)}.`}</AlertDescription>
        </Alert>
      );
    } else {
      const schema = path.schema;
      const object = path.object;
      body = (
        <CatalogGate catalog={catalog}>
          {(c) => <PanelBody path={{ ...path, schema, object, column }} catalog={c} tags={tags} />}
        </CatalogGate>
      );
    }
  }
  return (
    <Sheet
      open={open}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <SheetContent className="max-w-md" onCloseAutoFocus={returnFocus}>
        <SheetHeader>
          <SheetTitle className="break-all">{column}</SheetTitle>
          <SheetDescription className="flex flex-wrap gap-x-3">
            <span className="font-mono text-xs break-all">{fullPath({ ...path, column })}</span>
            {check.status === "found" ? <span className="font-mono text-xs">{check.type}</span> : null}
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-auto">{body}</div>
      </SheetContent>
    </Sheet>
  );
}

type ColumnCheck =
  | { status: "loading" }
  | { status: "unchecked" }
  | { status: "missing" }
  | { status: "found"; name: string; type: string };

/**
 * Whether the table's listing has the column `?column=` names, compared
 * without regard to case. A column the listing does not have gets no panel
 * (a tag written for it would be stored on nothing); one that cannot be
 * checked (the listing failed or was not listed) says so rather than claim it
 * carries nothing.
 */
function checkColumn(browse: BrowseResult, asked: string): ColumnCheck {
  if (browse.isError) return { status: "unchecked" };
  if (browse.data?.status !== 200) return { status: "loading" };
  const columns = browse.data.data.columns;
  if (browse.data.data.notListed || !columns) return { status: "unchecked" };
  const col = columns.find((c) => sameName(c.name, asked));
  return col ? { status: "found", name: col.name, type: col.type } : { status: "missing" };
}

function PanelBody({ path, catalog, tags }: { path: ColumnPath; catalog: GovernanceCatalog; tags: TagsResult }) {
  const can = usePolicyPermissions();
  const canRead = can("policy:read");
  const canTag = can("catalog_registry:update");
  const egret = catalog.warehouseId != null;
  const assignments = egret ? assignmentsOf(tags) : null;
  const [whoCanRead, setWhoCanRead] = useState(false);
  const [addTag, setAddTag] = useState<TagTarget | null>(null);

  const own = (assignments ?? []).filter((a) => a.column !== "" && sameName(a.column, path.column));
  const fromTable = (assignments ?? []).filter((a) => a.column === "");

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start gap-2">
        {canRead ? (
          <Button variant="outline" size="sm" onClick={() => setWhoCanRead(true)}>
            <UserSearch aria-hidden />
            Who can read this column?
          </Button>
        ) : null}
        {canTag ? (
          <div className="space-y-1">
            <Button
              variant="outline"
              size="sm"
              disabled={!egret}
              aria-describedby={egret ? undefined : "column-add-tag-reason"}
              onClick={() =>
                egret &&
                setAddTag({
                  catalog: path.catalog,
                  catalogId: catalog.id,
                  warehouseId: catalog.warehouseId!,
                  schema: path.schema,
                  table: path.object,
                  column: path.column,
                  assigned: own.map((a) => a.tag.id),
                })
              }
            >
              <TagIcon aria-hidden />
              Add tag
            </Button>
            {egret ? null : (
              <p id="column-add-tag-reason" className="text-xs text-muted-foreground">
                {TAGS_EGRET_ONLY}
              </p>
            )}
          </div>
        ) : null}
      </div>

      {egret ? (
        <Section title="Tags">
          {tags.isError ? (
            <TabError what="tags" onRetry={() => void tags.refetch()} />
          ) : assignments === undefined ? (
            <ListSkeleton rows={1} cols={2} />
          ) : assignments === null ? null : own.length + fromTable.length === 0 ? (
            <p className="text-sm text-muted-foreground">No tag on this column</p>
          ) : (
            <TagList column={path.column} own={own} fromTable={fromTable} />
          )}
        </Section>
      ) : null}

      {canRead ? (
        <>
          <Section title="Masking rules">
            <Masks path={path} catalog={catalog} tags={tags} />
          </Section>
          <Section title="Grants">
            <Grants path={path} catalogId={catalog.id} />
          </Section>
        </>
      ) : (
        <p className="text-sm text-muted-foreground">{PANEL_NEEDS_POLICY_READ}</p>
      )}

      <WhoCanReadDialog open={whoCanRead} onOpenChange={setWhoCanRead} path={path} catalogId={catalog.id} />
      <AddTagDialog
        target={addTag}
        onOpenChange={(open) => {
          if (!open) setAddTag(null);
        }}
      />
    </div>
  );
}

function TagList({ column, own, fromTable }: { column: string; own: ResourceTagDto[]; fromTable: ResourceTagDto[] }) {
  return (
    <ul aria-label={`Tags of ${column}`} className="flex flex-wrap gap-1">
      {own.map((a) => (
        <li key={`own-${a.tag.id}`}>
          <Badge variant="secondary" title={a.tag.key}>
            {a.tag.name}
          </Badge>
        </li>
      ))}
      {fromTable.map((a) => (
        <li key={`table-${a.tag.id}`}>
          <Badge variant="outline" title={a.tag.key}>
            {a.tag.name}
            <span className="font-normal text-muted-foreground">{" from table"}</span>
          </Badge>
        </li>
      ))}
    </ul>
  );
}

/** The mask rules that select the column, by path or by a tag it carries (its own or its table's). */
function Masks({ path, catalog, tags }: { path: ColumnPath; catalog: GovernanceCatalog; tags: TagsResult }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  if (rulesQ.isError) return <TabError what="masking rules" onRetry={() => void rulesQ.refetch()} />;
  const reach = tagReach(path, catalog, tags);
  if (rulesQ.data?.status !== 200 || reach.status === "loading") return <ListSkeleton rows={2} cols={2} />;
  const names = reach.status === "ok" ? tagNamesOn(reach.assignments, path) : [];
  const selected: { rule: RuleView; by: "path" | "tag" }[] = [];
  for (const r of rulesQ.data.data) {
    if (r.kind.type !== "mask") continue;
    const by = ruleSelects(r, path, catalog.id, names);
    if (by) selected.push({ rule: r, by });
  }
  selected.sort((a, b) => a.rule.name.localeCompare(b.rule.name));
  const unchecked =
    reach.status === "error" && rulesQ.data.data.some((r) => r.kind.type === "mask" && r.selector.type === "tag") ? (
      <p className="text-xs text-muted-foreground">Tag rules not checked: the tags could not be loaded.</p>
    ) : null;
  if (selected.length === 0) {
    return (
      <>
        <p className="text-sm text-muted-foreground">No masking rule selects this column</p>
        {unchecked}
      </>
    );
  }
  return (
    <div className="space-y-1">
      <ul aria-label={`Masking rules of ${path.column}`} className="space-y-1">
        {selected.map(({ rule, by }) => (
          <li key={rule.id} className="text-sm break-words">
            {rule.name}
            <span className="text-xs text-muted-foreground">
              {` ${formatRuleKind(rule)} · `}
              {by === "path" ? "by path" : `by tag ${rule.tag_name ?? ""}`}
              {rule.exempt.include.length > 0 ? ` · except ${formatPrincipals(rule.exempt.include)}` : ""}
            </span>
          </li>
        ))}
      </ul>
      {unchecked}
    </div>
  );
}

/** The grants set on the column: by its name, or by a pattern that matches it (marked). */
function Grants({ path, catalogId }: { path: ColumnPath; catalogId: string }) {
  const grantsQ = usePermListGrants({ query: { retry: false } });
  const workspaceName = useWorkspaceNames();
  const sentenceOf = (g: GrantView) => grantSentence(g, workspaceName);
  if (grantsQ.isError) return <TabError what="grants" onRetry={() => void grantsQ.refetch()} />;
  if (grantsQ.data?.status !== 200) return <ListSkeleton rows={2} cols={2} />;
  const onColumn = grantsQ.data.data
    .filter((g) => grantCoverage(g, path, catalogId) === "here")
    .sort((a, b) => sentenceOf(a).localeCompare(sentenceOf(b)));
  const note = (
    <p className="text-xs text-muted-foreground">Grants on the table and above reach this column too. See Permissions.</p>
  );
  if (onColumn.length === 0) {
    return (
      <>
        <p className="text-sm text-muted-foreground">No grant is set on this column</p>
        {note}
      </>
    );
  }
  return (
    <div className="space-y-2">
      <ul aria-label={`Grants on ${path.column}`} className="space-y-1">
        {onColumn.map((g) => (
          <GrantItem key={g.id} grant={g} sentence={sentenceOf(g)} />
        ))}
      </ul>
      {note}
    </div>
  );
}

function GrantItem({ grant, sentence }: { grant: GrantView; sentence: string }) {
  const patterns = patternLevels(grant.scope);
  let pattern: ReactNode = null;
  if (patterns.length > 0) {
    pattern = (
      <span className="text-xs text-muted-foreground">{` · matches ${patterns.map((p) => `“${p.value}”`).join(", ")}`}</span>
    );
  }
  return (
    <li className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm break-words">
      <span>{sentence}</span>
      <span>
        {grant.effect === "deny" ? <Badge variant="destructive">Deny</Badge> : <Badge variant="outline">Allow</Badge>}
        {pattern}
      </span>
    </li>
  );
}
