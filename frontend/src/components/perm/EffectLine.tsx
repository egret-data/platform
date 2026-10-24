import { Link } from "react-router-dom";

import type { EffectPart, ExplainEffectLine } from "@/api/generated/model";
import { tagUrl } from "@/routes/governance/classifications/tagUrl";
import { pathToUrl } from "@/routes/governance/data/objectPath";
import { principalLinkFor, principalUrl } from "@/routes/governance/people/peopleUrl";

/** A link inside a sentence: underlined, so it never reads by colour alone (FE-A11Y-4). */
const INLINE_LINK_CLASS = "underline underline-offset-2 hover:text-primary";

/**
 * One run of an effect line. Every part renders its `text` verbatim and
 * nothing else, so the line's text content is the server's `text`.
 */
function Part({ part }: { part: EffectPart }) {
  switch (part.kind) {
    case "text":
      return <>{part.text}</>;
    case "strong":
      return <strong className="font-semibold">{part.text}</strong>;
    case "code":
      return <code className="font-mono text-xs">{part.text}</code>;
    case "subject":
      return <span className="font-medium">{part.text}</span>;
    case "rule":
      return (
        <Link to={`/governance/rules/${encodeURIComponent(part.rule_id)}`} className={`italic ${INLINE_LINK_CLASS}`}>
          {part.text}
        </Link>
      );
    case "clearance":
      return (
        <Link to={tagUrl(part.tag_id, "clearance")} className={`italic ${INLINE_LINK_CLASS}`}>
          {part.text}
        </Link>
      );
    case "principal": {
      const link = principalLinkFor({ kind: part.principal_kind, ref: part.ref });
      // A workspace's members have no People page.
      if (!link) return <>{part.text}</>;
      return (
        <Link to={principalUrl(link)} className={INLINE_LINK_CLASS}>
          {part.text}
        </Link>
      );
    }
    case "policy":
      // A manual policy has no page in egret: it is named, not linked.
      return <em>{part.text}</em>;
    case "catalog":
      return (
        <Link to={pathToUrl({ catalog: part.text })} className={`italic ${INLINE_LINK_CLASS}`}>
          {part.text}
        </Link>
      );
  }
  // No wildcard: a part kind added to the wire and not here must break the build.
  const _never: never = part;
  return _never;
}

/**
 * One sentence of an explain answer, built by the server (spec C1): the
 * outcome in bold, a rule or a clearance as a link to its page, a principal as
 * a link to its People page, a column or an expression in mono. The client
 * renders the parts and never parses `text`. It wraps at any width.
 */
export function EffectLine({ line }: { line: ExplainEffectLine }) {
  return (
    <p data-testid="effect-line" data-outcome={line.outcome} className="text-sm break-words">
      {line.parts.map((part, i) => (
        <Part key={i} part={part} />
      ))}
    </p>
  );
}
