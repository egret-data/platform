import { Fragment } from "react";
import { Link } from "react-router-dom";
import type { GrantView, PermPrincipal } from "@/api/generated/model";
import { grantPredicate, principalWord, type WorkspaceNames } from "@/routes/governance/permFormat";
import { principalLinkFor, principalUrl } from "@/routes/governance/people/peopleUrl";

/**
 * One stored principal, its name linked to its People page (spec P14). A
 * workspace's members have no page and stay text. Reads exactly as
 * `principalWord`.
 */
function PrincipalName({ p, workspaceName }: { p: PermPrincipal; workspaceName?: WorkspaceNames }) {
  const link = principalLinkFor(p);
  if (!link) return <>{principalWord(p, workspaceName)}</>;
  return (
    <>
      {`${p.kind} `}
      <Link to={principalUrl(link)} className="break-all text-primary hover:underline">
        {p.ref}
      </Link>
    </>
  );
}

/** `formatPrincipals`, with each name a link. */
export function PrincipalNames({ list, workspaceName }: { list: PermPrincipal[]; workspaceName?: WorkspaceNames }) {
  if (list.length === 0) return <>nobody</>;
  return (
    <>
      {list.map((p, i) => (
        <Fragment key={`${p.kind}|${p.ref}`}>
          {i > 0 ? ", " : null}
          <PrincipalName p={p} workspaceName={workspaceName} />
        </Fragment>
      ))}
    </>
  );
}

/** `grantSentence`, with each principal's name a link; the same text. */
export function GrantSentenceText({ grant, workspaceName }: { grant: GrantView; workspaceName?: WorkspaceNames }) {
  const { include, exclude } = grant.principals;
  return (
    <>
      <PrincipalNames list={include} workspaceName={workspaceName} />
      {` ${grantPredicate(grant)}`}
      {exclude.length > 0 ? (
        <>
          {" except "}
          <PrincipalNames list={exclude} workspaceName={workspaceName} />
        </>
      ) : null}
    </>
  );
}
