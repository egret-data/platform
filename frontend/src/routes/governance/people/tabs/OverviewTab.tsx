import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import type { GroupRef, MemberRef, Membership, MembershipScope, NotOnEngines, Via } from "@/api/generated/model";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Section } from "@/routes/governance/tabParts";
import type { Principal } from "@/routes/governance/people/principalData";
import { principalUrl } from "@/routes/governance/people/peopleUrl";

/**
 * Whether engines apply a membership, in words (FE-A11Y-4, spec §3), exactly
 * as the server marks it (`onEngines`, `notOnEngines`). Nothing here works out
 * membership: a role or workspace held through a group is what the server
 * marks `held_through_group`, and that is what reads "egret only".
 */
export function appliedText(m: { onEngines: boolean; notOnEngines?: NotOnEngines | null }): string {
  if (m.onEngines) return "Yes";
  switch (m.notOnEngines) {
    case "held_through_group":
      return "egret only — not applied on engines: held through a group";
    case "expired":
      return "No: expired. Engines may apply it until the next recompile.";
    case "no_username":
      return "No: no username yet";
    default:
      return "No";
  }
}

function scopeText(scope: MembershipScope | null | undefined): string {
  if (!scope) return "";
  if (scope.type === "platform") return "Platform";
  if (scope.type === "workspace") return `Workspace ${scope.name}`;
  return `${scope.objectType ?? "Object"} ${scope.id}`;
}

/** `Never`, the date, or `Expired <date>`. */
function expiryText(m: Membership): string {
  if (!m.expiresAt) return "Never";
  const day = m.expiresAt.slice(0, 10);
  return m.expired ? `Expired ${day}` : day;
}

const linkClass = "break-all text-primary hover:underline";

function GroupLink({ name, label }: { name: string; label?: string }) {
  return (
    <Link to={principalUrl({ kind: "group", name })} className={linkClass}>
      {label ?? name}
    </Link>
  );
}

function RoleLink({ name }: { name: string }) {
  return (
    <Link to={principalUrl({ kind: "role", name })} className={linkClass}>
      {name}
    </Link>
  );
}

/** How a membership is held: directly, or through a group (linked). */
function Through({ via }: { via: Via }) {
  if (via.kind === "group_role" || via.kind === "group_workspace") return <GroupLink name={via.group} label={`Group ${via.group}`} />;
  return <>Direct</>;
}

function workspaceName(m: Membership): string {
  if (m.via.kind === "workspace" || m.via.kind === "group_workspace") return m.via.name;
  return m.scope?.type === "workspace" ? m.scope.name : "";
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="text-sm text-muted-foreground">{children}</p>;
}

function GroupsTable({ rows }: { rows: Membership[] }) {
  if (rows.length === 0) return <Empty>No groups.</Empty>;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Group</TableHead>
          <TableHead>Applied on engines</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((m, i) => (
          <TableRow key={i}>
            <TableCell>{m.via.kind === "group" ? <GroupLink name={m.via.name} /> : null}</TableCell>
            <TableCell>{appliedText(m)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** Roles held: by a user (with how) or by a group (always through the group, so no Through column). */
function RolesTable({ rows, through }: { rows: Membership[]; through: boolean }) {
  if (rows.length === 0) return <Empty>No roles.</Empty>;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Role</TableHead>
          <TableHead>Scope</TableHead>
          {through ? <TableHead>Through</TableHead> : null}
          <TableHead>Expires</TableHead>
          <TableHead>Applied on engines</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((m, i) => (
          <TableRow key={i}>
            <TableCell>{m.role ? <RoleLink name={m.role} /> : null}</TableCell>
            <TableCell>{scopeText(m.scope)}</TableCell>
            {through ? (
              <TableCell>
                <Through via={m.via} />
              </TableCell>
            ) : null}
            <TableCell className="whitespace-nowrap tabular-nums">{expiryText(m)}</TableCell>
            <TableCell>{appliedText(m)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function WorkspacesTable({ rows, through }: { rows: Membership[]; through: boolean }) {
  if (rows.length === 0) return <Empty>No workspaces.</Empty>;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Workspace</TableHead>
          <TableHead>Role</TableHead>
          {through ? <TableHead>Through</TableHead> : null}
          <TableHead>Expires</TableHead>
          <TableHead>Applied on engines</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((m, i) => (
          <TableRow key={i}>
            <TableCell>{workspaceName(m)}</TableCell>
            <TableCell>{m.role ? <RoleLink name={m.role} /> : null}</TableCell>
            {through ? (
              <TableCell>
                <Through via={m.via} />
              </TableCell>
            ) : null}
            <TableCell className="whitespace-nowrap tabular-nums">{expiryText(m)}</TableCell>
            <TableCell>{appliedText(m)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/**
 * A group's members or a role's holders. One with no username carries the
 * kind's sentence (spec §2): a group member gets a username when a grant
 * naming the group compiles; a role holder is not named on engines until
 * they have one.
 */
function MembersTable({ rows, of, empty }: { rows: MemberRef[]; of: "group" | "role"; empty: string }) {
  if (rows.length === 0) return <Empty>{empty}</Empty>;
  const noUsername =
    of === "group"
      ? "No username yet: a grant naming this group allocates one at compile"
      : "Not named on engines until they have a username";
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Username</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((m) => (
          <TableRow key={m.userId}>
            <TableCell>
              <div className="flex flex-wrap items-center gap-2">
                <Link to={principalUrl({ kind: "user", userId: m.userId })} className={linkClass}>
                  {m.label}
                </Link>
                {m.serviceAccount ? <Badge variant="outline">Service account</Badge> : null}
              </div>
            </TableCell>
            <TableCell>
              {m.username ? (
                <span className="break-all font-mono text-xs">{m.username}</span>
              ) : (
                <span className="text-muted-foreground">{noUsername}</span>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

const SOURCE: Record<string, string> = { local: "Local", oidc: "OIDC" };

/** Groups holding a role: egret's own checks count them; engines do not apply them (spec P3). */
function HolderGroupsTable({ rows }: { rows: GroupRef[] }) {
  if (rows.length === 0) return <Empty>No group holds this role.</Empty>;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Group</TableHead>
          <TableHead>Source</TableHead>
          <TableHead>Applied on engines</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((g) => (
          <TableRow key={g.name}>
            <TableCell>
              <GroupLink name={g.name} />
            </TableCell>
            <TableCell>{SOURCE[g.source] ?? g.source}</TableCell>
            <TableCell>egret only — not applied on engines</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** The Overview tab (spec §2): memberships, members or holders, as the server reads them. */
export function OverviewTab({ principal }: { principal: Principal }) {
  if (principal.kind === "user") {
    const u = principal.data;
    return (
      <div className="space-y-6">
        <Section title="Groups">
          <GroupsTable rows={u.groups} />
        </Section>
        <Section title="Roles">
          <RolesTable rows={u.roles} through />
        </Section>
        <Section title="Workspaces">
          <WorkspacesTable rows={u.workspaces} through />
        </Section>
      </div>
    );
  }
  if (principal.kind === "group") {
    const g = principal.data;
    return (
      <div className="space-y-6">
        <Section title="Members">
          <MembersTable rows={g.members} of="group" empty="No members. Grants naming this group match nobody." />
        </Section>
        <Section title="Roles held">
          <RolesTable rows={g.roles} through={false} />
        </Section>
        <Section title="Workspaces held">
          <WorkspacesTable rows={g.workspaces} through={false} />
        </Section>
      </div>
    );
  }
  const r = principal.data;
  return (
    <div className="space-y-6">
      <Section title="Holders">
        <MembersTable rows={r.holders} of="role" empty="No user holds this role. Grants naming it match nobody." />
      </Section>
      <Section title="Groups holding this role">
        <HolderGroupsTable rows={r.holderGroups} />
      </Section>
    </div>
  );
}
