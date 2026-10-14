import { isAlias, isMap, isScalar, parseDocument } from "yaml";
import { rangeOfPath } from "@/components/workflows/editor/yamlPath";

/**
 * A trigger's YAML for the details drawer (W7e spec §4.2): cutting it from the
 * flow's source, and hiding a webhook's key until Reveal. Both read YAML with
 * the `yaml` library, so this module is imported only from the flow pages'
 * chunk, never from the model the flow list shares (`assert-workflows-lazy.mjs`).
 */

/** What a hidden webhook key reads as. */
export const MASK = "••••••••";

/** The whole block, when the key cannot be hidden on its own (`maskWebhookKey`). */
export const HIDDEN_BLOCK = "# This trigger's YAML is hidden until Reveal: it holds the webhook's key.";

/**
 * The YAML block of one trigger, cut from `source` and dedented, or null when
 * `source` declares no trigger with that id or the block cannot be cut.
 * `rangeOfPath` finds where the trigger's mapping starts; the block runs until
 * the first non-blank line indented less than that mapping (the next trigger's
 * `- ` or the next top-level key).
 *
 * Limits, by design: a comment line indented less than the mapping belongs to
 * the block only when the block's own lines continue after it; a flow-style
 * list (`triggers: [{ id: a, … }]`) has no lines of its own to cut, so it is
 * null and the drawer points to the editor.
 *
 * Here rather than in `triggersModel.ts`: it brings the `yaml` library, which the
 * model's other users (the global Triggers page, in the flow list's chunk) must not.
 * Only the drawer and the flow's tab, both in the flow pages' chunk, import this module.
 */
export function triggerBlock(source: string, triggerId: string): string | null {
  const range = rangeOfPath(source, `triggers.${triggerId}`);
  if (!range) return null;
  const lines = source.split("\n");
  const indent = range.startCol - 1;
  const first = lines[range.startLine - 1];
  // Block style: only spaces and list dashes before the mapping.
  if (!/^[\s-]*$/.test(first.slice(0, indent))) return null;
  const indentOf = (line: string) => line.length - line.trimStart().length;
  const isComment = (line: string) => line.trimStart().startsWith("#");
  const block = [first.slice(indent)];
  for (let i = range.startLine; i < lines.length; i++) {
    const line = lines[i];
    if (line.trim() !== "" && indentOf(line) < indent) {
      if (!isComment(line)) break;
      // A less-indented comment: kept only if the block goes on after it.
      const next = lines.slice(i + 1).find((l) => l.trim() !== "" && !isComment(l));
      if (next === undefined || indentOf(next) < indent) break;
      block.push(line.trimStart());
      continue;
    }
    block.push(line.slice(indent));
  }
  while (block.length > 1 && block[block.length - 1].trim() === "") block.pop();
  return block.join("\n");
}

type Ranged = { range?: [number, number, number] | null };

/** The one `key` pair of a trigger mapping, or null when there is not exactly one, plainly keyed. */
function keyValueOf(source: string): { node: unknown; doc: ReturnType<typeof parseDocument> } | null {
  const doc = parseDocument(source);
  if (doc.errors.length > 0 || !isMap(doc.contents)) return null;
  const items = doc.contents.items;
  // A complex key (`? [a]: b`) could spell `key` some way this does not see.
  if (items.some((p) => !isScalar(p.key))) return null;
  const pairs = items.filter((p) => isScalar(p.key) && p.key.value === "key");
  if (pairs.length !== 1) return null;
  return { node: pairs[0].value, doc };
}

/**
 * A webhook trigger's YAML block with its `key` value hidden, failing closed.
 *
 * The block is parsed with the `yaml` library and the value node of the
 * trigger's one `key` pair (however the name is quoted, in block or flow
 * style) is replaced over its whole source range: a quoted value across
 * lines, a block scalar, a sequence. The result is parsed again and must hold
 * exactly the mask there. Whenever any of that does not hold (the block does
 * not parse, is not one mapping, has no single `key` pair, the value is an
 * alias to text elsewhere, or the re-parse disagrees), the WHOLE block is
 * hidden instead (`whole: true`).
 */
export function maskWebhookKey(block: string): { text: string; whole: boolean } {
  const hidden = { text: HIDDEN_BLOCK, whole: true };
  try {
    const found = keyValueOf(block);
    if (!found || !found.node || isAlias(found.node)) return hidden;
    const r = (found.node as Ranged).range;
    if (!r) return hidden;
    const removed = block.slice(r[0], r[1]);
    const trailing = /\s*$/.exec(removed)![0];
    // The value may start on the next line (`key:\n- s3cr3t`): the mask goes on the key's line.
    const text = `${block.slice(0, r[0]).replace(/\s+$/, "")} ${MASK}${trailing}${block.slice(r[1])}`;
    const again = keyValueOf(text);
    if (!again || !isScalar(again.node) || again.node.value !== MASK) return hidden;
    return { text, whole: false };
  } catch {
    return hidden;
  }
}
