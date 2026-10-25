import type { EffectAbout, EffectOutcome, EffectPart, ExplainEffectLine } from "@/api/generated/model";

/**
 * Effect lines as the server sends them (spec §5.1): `text` is the parts'
 * texts joined, which the server asserts for every line it builds. Shared by
 * the explain tests in `components/perm` and the governance routes.
 */
export function effectLine(about: EffectAbout, outcome: EffectOutcome, parts: EffectPart[]): ExplainEffectLine {
  return { about, outcome, parts, text: parts.map((p) => p.text).join("") };
}

export const text = (t: string): EffectPart => ({ kind: "text", text: t });
export const strong = (t: string): EffectPart => ({ kind: "strong", text: t });
export const code = (t: string): EffectPart => ({ kind: "code", text: t });
export const subject = (t: string): EffectPart => ({ kind: "subject", text: t });
export const rule = (t: string, ruleId: string): EffectPart => ({ kind: "rule", text: t, rule_id: ruleId });

/** "`u` reads `column` as **word** (rule *ruleName*)", the line for a rule mask. */
export function ruleMaskLine(u: string, column: string, word: string, ruleName: string, ruleId: string): ExplainEffectLine {
  return effectLine("column", "masked", [
    subject(u),
    text(" reads "),
    code(column),
    text(" as "),
    strong(word),
    text(" (rule "),
    rule(ruleName, ruleId),
    text(")"),
  ]);
}

/** "`u` sees **every row**", the rows line with no filter. */
export function everyRowLine(u: string): ExplainEffectLine {
  return effectLine("rows", "all_rows", [subject(u), text(" sees "), strong("every row")]);
}
