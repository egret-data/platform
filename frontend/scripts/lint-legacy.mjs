// Honesty check for eslint-rules/legacy.js: every listed file must exist and
// must still break the rule it is listed under.
import { existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { ESLint } from "eslint";
import tseslint from "typescript-eslint";
import { plugins, a11ySettings, a11yRules } from "../eslint-rules/plugins.js";

export async function staleEntries(legacy, { cwd }) {
  const stale = [];
  for (const [rule, files] of Object.entries(legacy)) {
    const [prefix, name] = rule.split("/");
    if (!plugins[prefix]?.rules?.[name]) throw new Error(`legacy list for unknown rule '${rule}'`);
    const present = files.filter((f) => existsSync(resolve(cwd, f)));
    for (const f of files) if (!present.includes(f)) stale.push({ rule, file: f, reason: "missing" });
    if (present.length === 0) continue;
    const eslint = new ESLint({
      cwd,
      overrideConfigFile: true,
      overrideConfig: [{
        files: ["**/*.{ts,tsx}"],
        languageOptions: { parser: tseslint.parser, parserOptions: { ecmaFeatures: { jsx: true } } },
        plugins,
        settings: a11ySettings,
        rules: { [rule]: ["error", ...[].concat(a11yRules[rule] ?? []).slice(1)] },
      }],
    });
    for (const r of await eslint.lintFiles(present)) {
      if (!r.messages.some((m) => m.ruleId === rule)) {
        stale.push({ rule, file: r.filePath.slice(resolve(cwd).length + 1), reason: "clean" });
      }
    }
  }
  return stale;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const cwd = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const { legacy } = await import("../eslint-rules/legacy.js");
  const stale = await staleEntries(legacy, { cwd });
  for (const s of stale) {
    console.error(`${s.rule}: ${s.file} is ${s.reason === "clean" ? "clean now" : "gone"}; remove it from eslint-rules/legacy.js`);
  }
  process.exit(stale.length ? 1 : 0);
}
