import { useId, useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ChevronLeft, ChevronRight, ShieldQuestion, Users } from "lucide-react";
import type { PersonRow } from "@/api/generated/model";
import { usePermListPeople } from "@/api/generated/perm/perm";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { TabError } from "@/routes/governance/tabParts";
import { principalUrl } from "@/routes/governance/people/peopleUrl";
import { UnmatchedNames } from "@/routes/governance/people/UnmatchedNames";
import { CheckAccessDialog } from "@/routes/governance/people/CheckAccessDialog";

type PersonKind = "user" | "group" | "role";
type TabId = PersonKind | "unmatched";

const TABS: { id: TabId; label: string }[] = [
  { id: "user", label: "Users" },
  { id: "group", label: "Groups" },
  { id: "role", label: "Roles" },
  { id: "unmatched", label: "Unmatched names" },
];

/** Each kind's nouns, written out (FE-COPY-4). */
const NOUN: Record<PersonKind, { one: string; many: string }> = {
  user: { one: "user", many: "users" },
  group: { one: "group", many: "groups" },
  role: { one: "role", many: "roles" },
};

const PLACEHOLDER: Record<TabId, string> = {
  user: "Name, email or username",
  group: "Group name",
  role: "Role name",
  unmatched: "Stored name",
};

export const PAGE_SIZE = 50;

/**
 * `/governance/people` (spec §2, P1, P11): Users · Groups · Roles · Unmatched
 * names. Everything a reader would share is in the URL (FE-PAGE-2): the tab
 * (`?kind=`, Users when absent), the search (`?q=`), the page (`?page=`) and
 * whether service accounts are listed (`?serviceAccounts=1`). A new tab or a
 * new search starts again at the first page. Typing replaces the history
 * entry; a tab or a page pushes one, so Back walks them.
 */
export default function PeopleIndex() {
  const [search, setSearch] = useSearchParams();
  const asked = search.get("kind");
  const kind: TabId = TABS.find((t) => t.id === asked)?.id ?? "user";
  const q = search.get("q") ?? "";
  const pageNo = Math.max(1, Number.parseInt(search.get("page") ?? "1", 10) || 1);
  const showServiceAccounts = search.get("serviceAccounts") === "1";
  const searchId = useId();
  const saId = useId();
  // Check access (spec §2): a dialog; its question and answer are a POST's, not a link's.
  const [checking, setChecking] = useState(false);

  const change = (changes: Record<string, string | null>, replace = false) =>
    setSearch(
      (prev) => {
        const next = new URLSearchParams(prev);
        for (const [k, v] of Object.entries(changes)) {
          if (v === null || v === "") next.delete(k);
          else next.set(k, v);
        }
        return next;
      },
      { replace },
    );

  const clearSearch = () => change({ q: null, page: null });

  return (
    <ListPageShell
      title="People & groups"
      description="Users, groups and roles, and the egret grants and rules that reach each one."
      toolbar={<ListToolbarButton icon={ShieldQuestion} label="Check access" onClick={() => setChecking(true)} />}
    >
      <CheckAccessDialog open={checking} onOpenChange={setChecking} />
      <Tabs value={kind} onValueChange={(t) => change({ kind: t === "user" ? null : t, page: null })}>
        {/* Wraps, so every tab is in view at 390 px. */}
        <TabsList className="h-auto max-w-full flex-wrap justify-start">
          {TABS.map((t) => (
            <TabsTrigger key={t.id} value={t.id}>
              {t.label}
            </TabsTrigger>
          ))}
        </TabsList>
        <div className="mt-4 flex flex-wrap items-center gap-x-6 gap-y-3">
          <div className="w-full max-w-sm">
            <Label htmlFor={searchId} className="sr-only">
              Search
            </Label>
            <Input
              id={searchId}
              type="search"
              value={q}
              placeholder={PLACEHOLDER[kind]}
              onChange={(e) => change({ q: e.target.value, page: null }, true)}
            />
          </div>
          {kind === "user" ? (
            <div className="flex items-center gap-2">
              <Switch
                id={saId}
                checked={showServiceAccounts}
                onCheckedChange={(on) => change({ serviceAccounts: on ? "1" : null, page: null })}
              />
              <Label htmlFor={saId}>Show service accounts</Label>
            </div>
          ) : null}
        </div>
        {TABS.map((t) => (
          <TabsContent key={t.id} value={t.id} className="mt-4">
            {t.id === kind ? (
              kind === "unmatched" ? (
                <UnmatchedNames q={q} onClearSearch={clearSearch} />
              ) : (
                <PeopleList
                  kind={kind}
                  q={q}
                  pageNo={pageNo}
                  showServiceAccounts={showServiceAccounts}
                  onPage={(n) => change({ page: n === 1 ? null : String(n) })}
                  onClearSearch={clearSearch}
                />
              )
            ) : null}
          </TabsContent>
        ))}
      </Tabs>
    </ListPageShell>
  );
}

function PeopleList({
  kind,
  q,
  pageNo,
  showServiceAccounts,
  onPage,
  onClearSearch,
}: {
  kind: PersonKind;
  q: string;
  pageNo: number;
  showServiceAccounts: boolean;
  onPage: (n: number) => void;
  onClearSearch: () => void;
}) {
  const offset = (pageNo - 1) * PAGE_SIZE;
  const people = usePermListPeople(
    {
      kind,
      q: q || undefined,
      limit: PAGE_SIZE,
      offset,
      includeServiceAccounts: kind === "user" && showServiceAccounts ? true : undefined,
    },
    // The previous page stays on screen while the next one loads; never another kind's rows.
    { query: { placeholderData: (prev) => (prev?.status === 200 && prev.data.items[0]?.kind === kind ? prev : undefined) } },
  );
  const noun = NOUN[kind];

  if (people.isError) return <TabError what={noun.many} onRetry={() => void people.refetch()} />;
  if (people.data?.status !== 200) return <ListSkeleton rows={6} cols={3} />;
  const { items, total } = people.data.data;

  if (items.length === 0) {
    if (q) {
      return (
        <EmptyState
          icon={Users}
          title={`No ${noun.one} matches “${q}”.`}
          action={
            <Button variant="outline" size="sm" onClick={onClearSearch}>
              Clear search
            </Button>
          }
        />
      );
    }
    if (total > 0) {
      return (
        <EmptyState
          title={`Page ${pageNo} is past the last ${noun.one}.`}
          action={
            <Button variant="outline" size="sm" onClick={() => onPage(1)}>
              Go to the first page
            </Button>
          }
        />
      );
    }
    return (
      <EmptyState
        icon={Users}
        title={`No ${noun.many}.`}
        description={
          kind === "user" && !showServiceAccounts
            ? "Service accounts are hidden. Show service accounts lists them too."
            : undefined
        }
      />
    );
  }

  const last = offset + items.length;
  return (
    <div className="space-y-3">
      {kind === "user" ? <UsersTable rows={items} /> : kind === "group" ? <GroupsTable rows={items} /> : <RolesTable rows={items} />}
      {kind === "group" ? (
        <Note>A member with no username gets one when a grant naming the group is compiled.</Note>
      ) : kind === "role" ? (
        <Note>
          Holders are users. A holder is named on engines once they have a username; a group holding a role is not
          counted, as engines do not apply it.
        </Note>
      ) : null}
      <nav aria-label="Pages" className="flex flex-wrap items-center justify-end gap-2">
        <span className="text-sm tabular-nums text-muted-foreground">{`${offset + 1}–${last} of ${total}`}</span>
        <Button variant="outline" size="sm" aria-label="Previous page" disabled={pageNo === 1} onClick={() => onPage(pageNo - 1)}>
          <ChevronLeft aria-hidden />
          Previous
        </Button>
        <Button variant="outline" size="sm" aria-label="Next page" disabled={last >= total} onClick={() => onPage(pageNo + 1)}>
          Next
          <ChevronRight aria-hidden />
        </Button>
      </nav>
    </div>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <p className="text-xs text-muted-foreground">{children}</p>;
}

function NameLink({ row }: { row: PersonRow }) {
  const to =
    row.kind === "user"
      ? principalUrl({ kind: "user", userId: row.key })
      : principalUrl({ kind: row.kind === "group" ? "group" : "role", name: row.key });
  return (
    <Link to={to} className="break-all font-medium text-primary hover:underline">
      {row.label}
    </Link>
  );
}

function UsersTable({ rows }: { rows: PersonRow[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Email</TableHead>
          <TableHead>Username</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={r.key}>
            <TableCell>
              <div className="flex flex-wrap items-center gap-2">
                <NameLink row={r} />
                {r.serviceAccount ? <Badge variant="outline">Service account</Badge> : null}
              </div>
            </TableCell>
            <TableCell className="break-all">{r.serviceAccount ? null : r.detail}</TableCell>
            <TableCell>
              {r.username ? (
                <span className="break-all font-mono text-xs">{r.username}</span>
              ) : (
                <span className="text-muted-foreground">No username yet</span>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

const SOURCE: Record<string, string> = { local: "Local", oidc: "OIDC" };

/** `3 members (1 with no username yet)`. */
function membersText(r: PersonRow): string {
  const n = r.memberCount ?? 0;
  const without = r.withoutUsername ?? 0;
  const count = `${n} ${n === 1 ? "member" : "members"}`;
  return without > 0 ? `${count} (${without} with no username yet)` : count;
}

/** `3 holders (1 not on engines: no username)`. */
function holdersText(r: PersonRow): string {
  const n = r.memberCount ?? 0;
  const without = r.withoutUsername ?? 0;
  const count = `${n} ${n === 1 ? "holder" : "holders"}`;
  return without > 0 ? `${count} (${without} not on engines: no username)` : count;
}

function GroupsTable({ rows }: { rows: PersonRow[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Source</TableHead>
          <TableHead>Members</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={r.key}>
            <TableCell>
              <NameLink row={r} />
            </TableCell>
            <TableCell>{r.detail ? (SOURCE[r.detail] ?? r.detail) : null}</TableCell>
            <TableCell className="tabular-nums">{membersText(r)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function RolesTable({ rows }: { rows: PersonRow[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Scopes</TableHead>
          <TableHead>Holders</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={r.key}>
            <TableCell>
              <NameLink row={r} />
            </TableCell>
            <TableCell>{r.detail}</TableCell>
            <TableCell className="tabular-nums">{holdersText(r)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
