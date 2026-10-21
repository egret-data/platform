import type { Rule } from "@/api/generated/model";
import { MASK_TYPE_LABEL, TERM } from "@/routes/governance/glossary";

/**
 * A rule's kind as `formatRuleKind` words it (`Mask · Hash`, `Row filter · <expr>`),
 * in the page's own font; only an expression the reader types (a row filter's
 * or a custom mask's) is set as code.
 */
export function RuleKindLabel({ rule }: { rule: Rule }) {
  const code = (expr: string) => <code className="font-mono text-xs break-words">{expr}</code>;
  if (rule.kind.type === "row_filter") {
    return (
      <span>
        {`${TERM.rowFilter} · `}
        {code(rule.kind.filter_expr)}
      </span>
    );
  }
  if (rule.kind.mask_type === "CUSTOM") {
    return (
      <span>
        {`${TERM.mask} · ${MASK_TYPE_LABEL.CUSTOM} (`}
        {code(rule.kind.value_expr ?? "")}
        {")"}
      </span>
    );
  }
  return <span>{`${TERM.mask} · ${MASK_TYPE_LABEL[rule.kind.mask_type] ?? rule.kind.mask_type}`}</span>;
}
