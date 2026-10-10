// Splits a class string into utilities with their variant prefixes, `!` and a
// leading `-` removed: "dark:hover:!-mt-2" -> "mt-2". Brackets may hold `:`.
export function classTokens(text) {
  return text.split(/\s+/).filter(Boolean).map((raw) => {
    let depth = 0, cut = 0;
    for (let i = 0; i < raw.length; i++) {
      if (raw[i] === "[") depth++;
      else if (raw[i] === "]") depth--;
      else if (raw[i] === ":" && depth === 0) cut = i + 1;
    }
    return { raw, utility: raw.slice(cut).replace(/^!/, "").replace(/^-/, "") };
  });
}

// Every string a rule should read: string literals and template chunks.
export function stringVisitors(check) {
  return {
    Literal(node) { if (typeof node.value === "string") check(node, node.value); },
    TemplateElement(node) { check(node, node.value.cooked ?? node.value.raw); },
  };
}
