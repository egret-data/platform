import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Plus, Tag as TagIcon, UserSearch } from "lucide-react";
import { Link, useLocation } from "react-router-dom";
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
import { ClassificationChip, useCanEditCatalogTags } from "@/routes/governance/ClassificationChip";
import { useRemoveClassification } from "@/routes/governance/RemoveClassificationDialog";
import { AddTagDialog, TAGS_EGRET_ONLY, type TagTarget } from "@/routes/governance/data/AddTagDialog";
import { grantCoverage, patternLevels, ruleSelects, sameName, tagNamesOn } from "@/routes/governance/data/coverage";
import { assignmentsOf, fullPath, tagReach, type BrowseResult, type CatalogState, type TagsResult } from "@/routes/governance/data/objectData";
import { dataRuleUrl, pathToUrl, type ObjectPath } from "@/routes/governance/data/objectPath";
import { CatalogGate } from "@/routes/governance/data/tabs/TabStates";
import { HandWrittenPointer, Section, TabError } from "@/routes/governance/tabParts";
import { HandWrittenSection } from "@/routes/governance/handwritten/HandWrittenSection";
import { TERM } from "@/routes/governance/glossary";
import { WhoCanReadDialog } from "@/routes/governance/data/WhoCanReadDialog";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { NotEnforcedBadge, useRuleEnforcementIn } from "@/routes/governance/NotEnforced";

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
 * - **Classifications**: its own, then its table's marked "from table" (a
 *   table's classification reaches its columns, `tagNamesOn`); only an egret
 *   catalog carries them (F9).
 * - **Rules** (masks; a row filter never applies to a column) that apply to
 *   it, by path or by a classification it carries, with their exemptions
 *   (`ruleSelects`).
 * - **Grants** set on it (column level, by name or by a pattern that matches
 *   it, `grantCoverage` "here"); the grants on the table and above are the
 *   Permissions tab's.
 *
 * F10: these are lists of what selects the column, never who sees what. Who
 * can read it is explain's answer, from "Who can read this column?".
 *
 * Add rule (`policy:update`, spec C8.3) opens `RuleDialog` on a mask of the
 * column's own classification when it has one (Task 13), else of the column's
 * path; each other classification it carries, and the column alone, is one
 * click away ("Or apply it to …").
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
  const canWriteRule = can("policy:update");
  const canTag = can("catalog_registry:update");
  const egret = catalog.warehouseId != null;
  const assignments = egret ? assignmentsOf(tags) : null;
  const [whoCanRead, setWhoCanRead] = useState(false);
  const [addRule, setAddRule] = useState(false);
  const [addTag, setAddTag] = useState<TagTarget | null>(null);
  const canRemoveTag = useCanEditCatalogTags();
  // After a remove, focus lands on the panel's Add classification (removing needs the same permission, so it is there).
  const addTagButton = useRef<HTMLButtonElement>(null);
  const removal = useRemoveClassification(() => addTagButton.current);

  const own = (assignments ?? []).filter((a) => a.column !== "" && sameName(a.column, path.column));
  const fromTable = (assignments ?? []).filter((a) => a.column === "");
  // What a rule on a classification would reach this column through: its own and its table's, once each.
  const carried = [
    ...own.map((a) => ({ key: a.tag.key, name: a.tag.name })),
    ...fromTable.map((a) => ({ key: a.tag.key, name: a.tag.name, wholeTable: true })),
  ].filter((c, i, all) => all.findIndex((o) => o.key === c.key) === i);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start gap-2">
        {canRead ? (
          <Button variant="outline" size="sm" onClick={() => setWhoCanRead(true)}>
            <UserSearch aria-hidden />
            Who can read this column?
          </Button>
        ) : null}
        {canWriteRule ? (
          <Button variant="outline" size="sm" onClick={() => setAddRule(true)}>
            <Plus aria-hidden />
            {TERM.addRule}
          </Button>
        ) : null}
        {canTag ? (
          <div className="space-y-1">
            <Button
              ref={addTagButton}
              variant="outline"
              size="sm"
              // Focusable while blocked, with its reason tied (N22); the click below does nothing then.
              aria-disabled={!egret ? true : undefined}
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
              {TERM.addClassification}
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
        <Section title={TERM.classifications}>
          {tags.isError ? (
            <TabError what="classifications" onRetry={() => void tags.refetch()} />
          ) : assignments === undefined ? (
            <ListSkeleton rows={1} cols={2} />
          ) : assignments === null ? null : own.length + fromTable.length === 0 ? (
            <p className="text-sm text-muted-foreground">No classification on this column</p>
          ) : (
            <TagList
              column={path.column}
              own={own}
              fromTable={fromTable}
              label={fullPath(path)}
              table={{
                href: pathToUrl({ catalog: path.catalog, schema: path.schema, object: path.object }),
                label: fullPath({ catalog: path.catalog, schema: path.schema, object: path.object }),
              }}
              onRemove={
                canRemoveTag && catalog.warehouseId != null
                  ? (a) =>
                      removal.ask({
                        tag: a.tag,
                        target: {
                          warehouseId: catalog.warehouseId!,
                          schema: a.schema,
                          table: a.table,
                          column: a.column,
                          label: fullPath(path),
                        },
                      })
                  : undefined
              }
            />
          )}
        </Section>
      ) : null}

      {canRead ? (
        <>
          <Section title={TERM.rules}>
            <Masks path={path} catalog={catalog} tags={tags} />
          </Section>
          <Section title="Grants">
            <Grants path={path} catalogId={catalog.id} canExplain={canRead} />
          </Section>
          {/* F12 (#4b spec §6): every hand-written policy that reaches the column. */}
          <HandWrittenSection
            catalogId={catalog.id}
            schema={path.schema}
            table={path.object}
            column={path.column}
            show="all"
          />
        </>
      ) : (
        <p className="text-sm text-muted-foreground">{PANEL_NEEDS_POLICY_READ}</p>
      )}

      <WhoCanReadDialog open={whoCanRead} onOpenChange={setWhoCanRead} path={path} catalogId={catalog.id} />
      <RuleDialog
        open={addRule}
        onOpenChange={setAddRule}
        entry={{
          kind: "column",
          catalogId: catalog.id,
          schema: path.schema,
          table: path.object,
          column: path.column,
          classifications: carried,
        }}
      />
      {removal.dialog}
      <AddTagDialog
        target={addTag}
        onOpenChange={(open) => {
          if (!open) setAddTag(null);
        }}
      />
    </div>
  );
}

function TagList({
  column,
  own,
  fromTable,
  label,
  onRemove,
  table,
}: {
  column: string;
  own: ResourceTagDto[];
  fromTable: ResourceTagDto[];
  /** The column as the chips name it: `ice.sales.orders.email`. */
  label: string;
  /** Offers Remove on the column's own classifications (spec C4). */
  onRemove?: (a: ResourceTagDto) => void;
  /** The table the inherited ones come from: "from table" links to its page, which closes this panel. */
  table: { href: string; label: string };
}) {
  return (
    <ul aria-label={`${TERM.classifications} of ${column}`} className="flex flex-wrap gap-1">
      {own.map((a) => (
        <li key={`own-${a.tag.id}`}>
          <ClassificationChip tag={a.tag} objectLabel={label} onRemove={onRemove ? () => onRemove(a) : undefined} />
        </li>
      ))}
      {fromTable.map((a) => (
        <li key={`table-${a.tag.id}`}>
          <ClassificationChip tag={a.tag} objectLabel={label} inherited inheritedFrom={table} />
        </li>
      ))}
    </ul>
  );
}

/** The mask rules that apply to the column, by path or by a classification it carries (its own or its table's). */
function Masks({ path, catalog, tags }: { path: ColumnPath; catalog: GovernanceCatalog; tags: TagsResult }) {
  const rulesQ = usePermListRules({ query: { retry: false } });
  const workspaceName = useWorkspaceNames(catalog);
  // This page with the panel open: the rule page's way back reopens it.
  const location = useLocation();
  const back = location.pathname + location.search;
  const reach = tagReach(path, catalog, tags);
  const names = reach.status === "ok" ? tagNamesOn(reach.assignments, path) : [];
  const selected: { rule: RuleView; by: "path" | "tag" }[] = [];
  if (rulesQ.data?.status === 200 && reach.status !== "loading") {
    for (const r of rulesQ.data.data) {
      if (r.kind.type !== "mask") continue;
      const by = ruleSelects(r, path, catalog.id, names);
      if (by) selected.push({ rule: r, by });
    }
  }
  selected.sort((a, b) => a.rule.name.localeCompare(b.rule.name));
  // Whether each rule is enforced IN THIS CATALOG: said beside it once the server says so.
  const enforcement = useRuleEnforcementIn(
    selected.map((s) => s.rule.id),
    catalog.id,
  );
  if (rulesQ.isError) return <TabError what="rules" onRetry={() => void rulesQ.refetch()} />;
  if (rulesQ.data?.status !== 200 || reach.status === "loading") return <ListSkeleton rows={2} cols={2} />;
  const unchecked =
    reach.status === "error" && rulesQ.data.data.some((r) => r.kind.type === "mask" && r.selector.type === "tag") ? (
      <p className="text-xs text-muted-foreground">Rules on classifications not checked: the classifications could not be loaded.</p>
    ) : null;
  if (selected.length === 0) {
    return (
      <>
        <p className="text-sm text-muted-foreground">No rule applies to this column.</p>
        {unchecked}
      </>
    );
  }
  return (
    <div className="space-y-1">
      <ul aria-label={`${TERM.rules} of ${path.column}`} className="space-y-1">
        {selected.map(({ rule, by }) => (
          <li key={rule.id} className="text-sm break-words">
            <Link to={dataRuleUrl(rule.id, back)} className="font-medium text-primary hover:underline">
              {rule.name}
            </Link>
            <span className="text-xs text-muted-foreground">
              {` ${formatRuleKind(rule)} · `}
              {by === "path" ? "by path" : `by classification ${rule.tag_name ?? ""}`}
              {rule.exempt.include.length > 0 ? ` · except ${formatPrincipals(rule.exempt.include, workspaceName)}` : ""}
            </span>
            <NotEnforcedBadge enforcement={enforcement?.get(rule.id)} appliesTo={rule.selector.type} className="ml-1" />
          </li>
        ))}
      </ul>
      {unchecked}
    </div>
  );
}

/** The grants set on the column: by its name, or by a pattern that matches it (marked). */
function Grants({ path, catalogId, canExplain }: { path: ColumnPath; catalogId: string; canExplain: boolean }) {
  const grantsQ = usePermListGrants({ query: { retry: false } });
  const workspaceName = useWorkspaceNames();
  const sentenceOf = (g: GrantView) => grantSentence(g, workspaceName);
  if (grantsQ.isError) return <TabError what="grants" onRetry={() => void grantsQ.refetch()} />;
  if (grantsQ.data?.status !== 200) return <ListSkeleton rows={2} cols={2} />;
  const onColumn = grantsQ.data.data
    .filter((g) => grantCoverage(g, path, catalogId) === "here")
    .sort((a, b) => sentenceOf(a).localeCompare(sentenceOf(b)));
  const note = (
    <>
      <p className="text-xs text-muted-foreground">Grants on the table and above reach this column too. See Permissions.</p>
      <HandWrittenPointer explainLabel={canExplain ? "Who can read this column?" : null} />
    </>
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
