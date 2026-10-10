# Frontend rules — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** egret gets one frontend rulebook (`docs/{en,vi}/frontend-rules.md`) that every agent working under `frontend/` is pointed to, and lint enforces its machine-checkable rules: new code cannot break them, and existing breaks are frozen in legacy lists that can only shrink.

**Architecture:**
- **A local ESLint plugin, `frontend/eslint-rules/`,** registered as `egret`. It holds three rules (`no-raw-palette`, `no-arbitrary-text-size`, `no-window-confirm`), a registry of the plugins whose rules may have legacy lists (`plugins.js`), and the legacy lists (`legacy.js`: one entry per rule id, listing the files that broke it when it was introduced).
- **`eslint.config.js`** turns each listed rule off for exactly its listed files. `scripts/lint-legacy.mjs` re-lints every listed file with its rule on and fails when a listed file no longer breaks it. `npm run lint` runs ESLint and then that script, so the existing gate runs both.
- **Accessibility lint** comes from `eslint-plugin-jsx-a11y-x`, the maintained fork of `eslint-plugin-jsx-a11y`. The fork has the same rules and declares ESLint 10 support; upstream 6.10.2 stops at ESLint 9.
- **The rulebook** is the spec's §4 in the spec's §3 format, followed by the reviewer checklist and a debt register whose numbers are measured.

**Tech Stack:** ESLint 10 flat config (`frontend/eslint.config.js`, ESM), typescript-eslint 8, vitest 4 (with `globals: true`, so `RuleTester` uses vitest's `describe`/`it`), Node ESM scripts, Markdown.

**Spec:** `docs/{en,vi}/specs/2026-10-10-frontend-rules-design.md`. **§n refers to the spec.**

## Global Constraints

- **User decisions (spec header, 2026-10-10), verbatim in effect:**
  - Enforcement is the rulebook plus lint. Rules a machine can check reliably become ESLint errors. Code that already breaks them is listed as named debt, and new code cannot add to it. There is no bulk migration.
  - `docs/en/frontend-rules.md` is normative. `docs/en/ui-conventions.md` stays as the rationale and is not otherwise edited.
  - UI copy is English, with no i18n.
  - Visual system is egret's; structure and flow copy the area's source product (BI: Metabase, Workflow: Kestra), checked against a running instance.
- **Rule ids** are `FE-<GROUP>-<n>`. The groups and numbering are exactly those of §4: PAGE 1–5, STATE 1–4, ACTION 1–5, VISUAL 1–6, FORM 1–4, DATA 1–3, ROUTE 1–3, A11Y 1–5, COPY 1–4, REF 1–2, TEST 1–4.
- **Every egret lint rule and every jsx-a11y rule is `error`.** The React Compiler rules that `eslint.config.js` sets to `warn` are not touched.
- **Exemptions:**
  - The egret rules apply to `src/**/*.{ts,tsx}` except tests: `**/__tests__/**` and `**/*.test.{ts,tsx}`.
  - `egret/no-raw-palette` also skips `src/components/chart/echarts/**` (FE-VISUAL-6).
- **Legacy lists.**
  - A legacy list is created once, from a measured lint run, in the task that introduces its rule. A file is never added to a list afterwards.
  - Paths are relative to `frontend/` (for example `src/components/sqllab/SqlEditor.tsx`).
- **No production code changes** except deleting `src/theme/egret-theme.ts`. Do not "fix" a violation to shorten a list; that is the debt policy's job, file by file.
- **The frontend has no prettier.** Never run `npx prettier --write`. Never run bare `orval`; this plan does not regenerate the client.
- **Gate for every task** (from `frontend/`):
  - `npm run lint` (ESLint plus `lint:legacy` once Task 1 lands)
  - `npx tsc -b`
  - `npm run test`
  - `npm run build`
  - A `ChartExplore` 5 s timeout under parallel load is a known flake. Re-run that file alone before calling it a failure, and report both runs.
- **Git:**
  - Conventional Commits; never commit `.superpowers/`; never override the git author.
  - End every commit with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - Do not push.
- **Docs are bilingual.** The English file is canonical. The Vietnamese file sits at the mirrored path under `docs/vi/`, opens with a note that it is a reference and that English wins on conflict, and carries the same rule ids and numbers.

---

### Task 1: The `egret` ESLint plugin, legacy lists and their honesty check

**Files:**
- Create:
  - `frontend/eslint-rules/index.js`
  - `frontend/eslint-rules/classTokens.js`
  - `frontend/eslint-rules/rules/no-raw-palette.js`
  - `frontend/eslint-rules/rules/no-arbitrary-text-size.js`
  - `frontend/eslint-rules/rules/no-window-confirm.js`
  - `frontend/eslint-rules/plugins.js`
  - `frontend/eslint-rules/legacy.js`
  - `frontend/scripts/lint-legacy.mjs`
- Create (tests):
  - `frontend/eslint-rules/__tests__/rules.test.js`
  - `frontend/eslint-rules/__tests__/lint-legacy.test.js`
  - `frontend/eslint-rules/__tests__/fixtures/breaks.tsx`
  - `frontend/eslint-rules/__tests__/fixtures/clean.tsx`
- Modify: `frontend/eslint.config.js`, `frontend/package.json` (`lint`, `lint:legacy` scripts)
- Delete: `frontend/src/theme/egret-theme.ts`. Check first: `grep -rn "theme/egret-theme\|egretTheme" frontend/src` must print only lines from the file itself.

**Interfaces:**
- Produces:
  - `export default plugin` from `eslint-rules/index.js`, shaped `{ meta: { name: "eslint-plugin-egret" }, rules: { "no-raw-palette", "no-arbitrary-text-size", "no-window-confirm" } }`.
  - `export const plugins` from `eslint-rules/plugins.js`, shaped `{ egret }`. Task 2 adds `"jsx-a11y"`.
  - `export const legacy` from `eslint-rules/legacy.js`, shaped `Record<ruleId, string[]>`.
  - `export async function staleEntries(legacy, { cwd })` from `scripts/lint-legacy.mjs`. It returns `Array<{ rule, file, reason: "missing" | "clean" }>`.

- [ ] **Step 1: Write the rule tests (red).** In `eslint-rules/__tests__/rules.test.js`, start the file with `// @vitest-environment node`. Use `RuleTester` from `eslint` with `languageOptions: { parser: tseslint.parser, parserOptions: { ecmaFeatures: { jsx: true } } }`, where `tseslint` comes from `typescript-eslint`. Cases, at least:
  - `no-raw-palette`:
    - **valid:**
      - `<div className="bg-primary text-muted-foreground" />`
      - `cn("text-destructive", x && "bg-highlight")`
      - `"text-sm"`
      - `const c = "#ff0000"` (a hex literal that is not a class token)
      - `"border-border"`
    - **invalid** (one error each, the message naming the token):
      - `"bg-green-600"`
      - `"dark:bg-red-500"`
      - `"hover:text-sky-500/20"`
      - `` `p-2 ${a} border-amber-200` ``
      - `"bg-[#0af]"`
      - `"text-[oklch(0.5_0.1_200)]"`
      - `"!bg-rose-950"`
      - `"-from-teal-50"`
  - `no-arbitrary-text-size`:
    - **valid:** `"text-sm"`, `"text-[var(--x)]"`, `"text-[#fff]"` (that is a colour; another rule handles it).
    - **invalid:** `"text-[11px]"`, `"md:text-[0.75rem]"`, `` `text-[13.5px] ${x}` ``.
  - `no-window-confirm`:
    - **valid:**
      - `const confirm = () => true; confirm();`
      - `function f(confirm) { confirm(); }`
      - `dialog.confirm()`
    - **invalid:**
      - `window.confirm("x")`
      - `globalThis.confirm("x")`
      - `confirm("x")`, where it is the global
      - `if (!window.confirm("Discard?")) return;`

  Run `npx vitest run eslint-rules`. Expected: FAIL, because the rule modules do not exist.

- [ ] **Step 2: Implement the token helper and the three rules.** `classTokens.js`:

```js
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
```

`rules/no-raw-palette.js`:

```js
import { classTokens, stringVisitors } from "../classTokens.js";

const UTIL = "bg|text|border(?:-[trblxyse])?|ring|ring-offset|outline|fill|stroke|from|via|to|divide|placeholder|caret|accent|decoration|shadow";
const HUE = "slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose";
const PALETTE = new RegExp(`^(?:${UTIL})-(?:${HUE})-(?:50|[1-9]00|950)(?:/\\d{1,3})?$`);
const ARBITRARY = /^[a-z-]+-\[(?:#[0-9a-fA-F]{3,8}|(?:rgba?|hsla?|oklch|oklab|color-mix)\()/;

export default {
  meta: {
    type: "problem",
    docs: { description: "FE-VISUAL-1: colours come from tokens (docs/en/frontend-rules.md)" },
    messages: {
      raw: "'{{token}}' is a raw colour; use a token class such as bg-primary or text-muted-foreground (FE-VISUAL-1, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    return stringVisitors((node, text) => {
      for (const { raw, utility } of classTokens(text)) {
        if (PALETTE.test(utility) || ARBITRARY.test(utility)) {
          context.report({ node, messageId: "raw", data: { token: raw } });
        }
      }
    });
  },
};
```

`rules/no-arbitrary-text-size.js`: the same shape, with `/^text-\[\d+(?:\.\d+)?(?:px|rem|em)\]$/` and the message `"'{{token}}' is an arbitrary font size; use the type scale: text-xs, text-sm, text-base, text-lg, text-xl, text-2xl (FE-VISUAL-3, docs/en/frontend-rules.md)."`

`rules/no-window-confirm.js`:

```js
export default {
  meta: {
    type: "problem",
    docs: { description: "FE-ACTION-1: confirm destructive actions with a dialog" },
    messages: {
      confirm: "window.confirm is not egret's confirmation; use ConfirmDeleteDialog (src/components/ui/confirm-delete-dialog.tsx) or AlertDialog (FE-ACTION-1, docs/en/frontend-rules.md).",
    },
    schema: [],
  },
  create(context) {
    const isGlobalConfirm = (node) => {
      let scope = context.sourceCode.getScope(node);
      for (; scope; scope = scope.upper) {
        const v = scope.set.get("confirm");
        if (v && v.defs.length > 0) return false;
      }
      return true;
    };
    return {
      CallExpression(node) {
        const c = node.callee;
        if (c.type === "Identifier" && c.name === "confirm" && isGlobalConfirm(c)) {
          context.report({ node, messageId: "confirm" });
        } else if (
          c.type === "MemberExpression" && !c.computed &&
          c.property.type === "Identifier" && c.property.name === "confirm" &&
          c.object.type === "Identifier" && ["window", "globalThis", "self"].includes(c.object.name)
        ) {
          context.report({ node, messageId: "confirm" });
        }
      },
    };
  },
};
```

`index.js` exports the plugin object of **Interfaces**. Run `npx vitest run eslint-rules`. Expected: the rule tests pass. If a case fails, fix the rule, not the case. Each case is a claim from §7.1.

- [ ] **Step 3: Measure the legacy lists.** Put the egret rules into `eslint.config.js` with no legacy yet. Use two blocks, so the echarts exemption applies to `no-raw-palette` only:

```js
import egret from "./eslint-rules/index.js";
import { legacy } from "./eslint-rules/legacy.js";

const EGRET_FILES = ["src/**/*.{ts,tsx}"];
const TESTS = ["**/__tests__/**", "**/*.test.{ts,tsx}"];
// … inside tseslint.config(…), after the existing block:
{
  files: EGRET_FILES,
  ignores: [...TESTS, "src/components/chart/echarts/**"],
  plugins: { egret },
  rules: { "egret/no-raw-palette": "error" },
},
{
  files: EGRET_FILES,
  ignores: TESTS,
  plugins: { egret },
  rules: { "egret/no-arbitrary-text-size": "error", "egret/no-window-confirm": "error" },
},
// The legacy lists: each rule off for exactly the files that broke it when it
// was introduced (docs/en/frontend-rules.md, the debt register).
...Object.entries(legacy).map(([rule, files]) => ({ files, rules: { [rule]: "off" } })),
```

Start `legacy.js` as `export const legacy = {};`. Then measure:

```bash
cd frontend && npx eslint src --format json -o /tmp/fe-rules-lint.json; node -e '
const r = require("/tmp/fe-rules-lint.json"); const by = {};
for (const f of r) for (const m of f.messages) if (m.ruleId?.startsWith("egret/"))
  (by[m.ruleId] ??= new Map()).set(f.filePath.replace(process.cwd() + "/", ""), ((by[m.ruleId].get(f.filePath.replace(process.cwd()+"/",""))) ?? 0) + 1);
for (const [k, v] of Object.entries(by)) console.log(k, v.size, "files", [...v.values()].reduce((a, b) => a + b, 0), "reports\n ", [...v.keys()].sort().join("\n  "));'
```

Use a scratch path under the session scratchpad instead of `/tmp` if one is given. Record the counts in the report. Expected order of magnitude, from the survey: about 12 files for `no-raw-palette`, about 33 reports for `no-arbitrary-text-size`, and 2 files for `no-window-confirm`. If a report is not a class token (for example a comment-like string), say so in the report and do not weaken the rule silently. Write the measured files into `legacy.js`, sorted, one array per rule id, with a header comment:

```js
// Files that broke a rule when it was introduced (docs/en/frontend-rules.md,
// "Debt register"). Lists only shrink: a file is never added after the list is
// created, and `npm run lint:legacy` fails when a listed file no longer breaks
// its rule, so a fixed file must leave its list in the same commit.
export const legacy = {
  "egret/no-raw-palette": [ /* measured, sorted */ ],
  "egret/no-arbitrary-text-size": [ /* … */ ],
  "egret/no-window-confirm": [ /* … */ ],
};
```

Run `npx eslint .`. Expected: no `egret/*` error.

- [ ] **Step 4: Write the honesty-check tests (red).** Fixtures:
  - `eslint-rules/__tests__/fixtures/breaks.tsx` holds `export const A = () => <div className="bg-red-500" />;`
  - `clean.tsx` holds the same component with `bg-primary`.

`lint-legacy.test.js`, starting with `// @vitest-environment node`, asserts:
  - `staleEntries({ "egret/no-raw-palette": ["eslint-rules/__tests__/fixtures/breaks.tsx"] }, { cwd })` returns `[]`;
  - with `clean.tsx` it returns `[{ rule: "egret/no-raw-palette", file: "eslint-rules/__tests__/fixtures/clean.tsx", reason: "clean" }]`;
  - with a non-existent path it returns `reason: "missing"`;
  - with an unknown rule id it **throws** an error naming the id.

`cwd` is the `frontend/` directory, resolved from `import.meta.url`. Run the tests. Expected: FAIL, because the module does not exist.

- [ ] **Step 5: Implement `scripts/lint-legacy.mjs` and the `plugins.js` registry.**

```js
// eslint-rules/plugins.js — every plugin whose rules may carry a legacy list.
import egret from "./index.js";
export const plugins = { egret };
```

```js
// scripts/lint-legacy.mjs
import { existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { ESLint } from "eslint";
import tseslint from "typescript-eslint";
import { plugins } from "../eslint-rules/plugins.js";

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
        rules: { [rule]: "error" },
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
```

In `package.json`:
- `"lint:legacy": "node scripts/lint-legacy.mjs"`
- `"lint": "eslint . && node scripts/lint-legacy.mjs"`

Run the tests, then `npm run lint`. Expected: both green.

- [ ] **Step 6: Prove the lists only shrink, and that new code is blocked.** Do these two checks on a scratch edit and revert both. Run `git diff --stat` afterwards to confirm the revert. Record the output of each in the report.
  1. Add `<span className="text-red-600" />` to a file that is not on the `no-raw-palette` list, for example `src/components/explore/EmptyState.tsx`. Run `npx eslint src/components/explore/EmptyState.tsx`. Expected: one `egret/no-raw-palette` error.
  2. Take one file on the `no-raw-palette` list and change every reported token to a token class (temporarily). Run `npm run lint:legacy`. Expected: it fails, naming that file as "clean now".

- [ ] **Step 7: Delete `src/theme/egret-theme.ts`.** Run the grep from **Files** first. Then run the whole gate. Expected: green, with the known ChartExplore flake rule applied.

- [ ] **Step 8: Commit.**

```bash
git add frontend/eslint-rules frontend/scripts/lint-legacy.mjs frontend/eslint.config.js frontend/package.json frontend/src/theme/egret-theme.ts
git commit -m "feat(frontend): egret lint rules for raw colours, arbitrary text sizes and window.confirm, with shrink-only legacy lists

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: Accessibility lint (`eslint-plugin-jsx-a11y-x`)

**Files:**
- Modify:
  - `frontend/package.json`, `frontend/package-lock.json`
  - `frontend/eslint.config.js`
  - `frontend/eslint-rules/plugins.js`
  - `frontend/eslint-rules/legacy.js`
- Test: `frontend/eslint-rules/__tests__/a11y-config.test.js`

**Interfaces:**
- Consumes: `plugins`, `legacy`, `staleEntries` and the `lint` script from Task 1.
- Produces: `plugins["jsx-a11y"]`, and legacy entries keyed `jsx-a11y/<rule>`.

- [ ] **Step 1: Install, pinned exactly.**
  - Run `npm i -D --save-exact eslint-plugin-jsx-a11y-x@0.2.0`. Expected: no ERESLOVE error; the package's peer range includes ESLint 10.
  - Read the package's `README` and `package.json` `exports` in `node_modules/eslint-plugin-jsx-a11y-x`, and record the name of the flat recommended config (for example `flatConfigs.recommended` or `configs.recommended`) in the report. Do not guess it.

- [ ] **Step 2: Write the config test (red).** In `a11y-config.test.js` (`// @vitest-environment node`):
  - use `new ESLint({ cwd })` with the real `eslint.config.js`;
  - call `calculateConfigForFile("src/components/explore/EmptyState.tsx")`;
  - assert that `rules["jsx-a11y/alt-text"]` and `rules["jsx-a11y/label-has-associated-control"]` are at severity 2;
  - assert that `settings["jsx-a11y"].components` maps `Button` to `"button"`, `Input` to `"input"`, `Label` to `"label"` and `Textarea` to `"textarea"`;
  - assert that for `src/components/explore/__tests__/x.test.tsx` the jsx-a11y rules are off.

  Expected: FAIL.

- [ ] **Step 3: Configure.** Add a block for `src/**/*.tsx`, ignoring the test globs. It spreads the fork's recommended rules, sets every rule it enables to `"error"`, and adds:

```js
settings: {
  "jsx-a11y": {
    // egret's primitives render these elements (src/components/ui), so the
    // rules see through them (FE-A11Y-1, FE-FORM-2).
    components: { Button: "button", Input: "input", Label: "label", Textarea: "textarea" },
  },
},
```

Add `"jsx-a11y": jsxA11y` to `eslint-rules/plugins.js`. Run the config test. Expected: pass.

- [ ] **Step 4: Measure and write the legacy entries.** Use the same measuring command as Task 1 Step 3, filtering `m.ruleId?.startsWith("jsx-a11y/")`.
  - Record every rule's file and report counts in the report.
  - Add one sorted entry per `jsx-a11y/<rule>` that fired.
  - If a rule fires on more than 40 files, stop and report it (NEEDS_CONTEXT) with the count and three sample messages. Do not turn the rule off: whether it belongs at error is the user's call.

- [ ] **Step 5: Verify.**
  - Run `npm run lint`. Expected: green, including `lint:legacy` over the new entries.
  - Scratch-add `<img src="x" />` to `src/components/explore/EmptyState.tsx`, run ESLint on it, and expect `jsx-a11y/alt-text`. Then revert, and record the output.
  - Run the whole gate.

- [ ] **Step 6: Commit.**

```bash
git add frontend/package.json frontend/package-lock.json frontend/eslint.config.js frontend/eslint-rules
git commit -m "feat(frontend): accessibility lint with eslint-plugin-jsx-a11y-x, existing reports frozen as legacy

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: The rulebook, the pointer files and the debt register

**Files:**
- Create:
  - `docs/en/frontend-rules.md`
  - `docs/vi/frontend-rules.md`
  - `frontend/CLAUDE.md`
  - `frontend/AGENTS.md`
- Modify: `docs/en/ui-conventions.md` and `docs/vi/ui-conventions.md` (one line each, at the top, under the title)
- Test: `frontend/eslint-rules/__tests__/rulebook.test.js`

**Interfaces:**
- Consumes:
  - `legacy` (Task 1, 2), for the register's lint rows;
  - the rule ids `egret/no-raw-palette`, `egret/no-arbitrary-text-size`, `egret/no-window-confirm`, and every `jsx-a11y/*` key in `legacy`.

- [ ] **Step 1: Write the rulebook test (red).** In `rulebook.test.js` (`// @vitest-environment node`), read the files from disk and assert:
  1. `docs/en/frontend-rules.md` contains every id of the Global Constraints numbering, each exactly once as a heading or rule line: PAGE 1–5, STATE 1–4, ACTION 1–5, VISUAL 1–6, FORM 1–4, DATA 1–3, ROUTE 1–3, A11Y 1–5, COPY 1–4, REF 1–2, TEST 1–4. It contains no other `FE-…-n`.
  2. `docs/vi/frontend-rules.md` contains the same set of ids.
  3. Every repo path in backticks in the English rulebook that starts with `src/`, `frontend/`, `docs/` or `scripts/` exists. A `src/` path resolves from `frontend/`.
  4. Every rule id in `legacy` appears in the rulebook's "Debt register" section, with the number of files in its list.
  5. Every lint rule named in the rulebook (`egret/…`) exists in `plugins.egret.rules`.
  6. `frontend/CLAUDE.md` and `frontend/AGENTS.md` each contain the path `../docs/en/frontend-rules.md`, and that path exists.
  7. The first 5 lines of each `ui-conventions.md` mention `frontend-rules.md`.

  Expected: FAIL.

- [ ] **Step 2: Recount the review-checked debt.** Run these from `frontend/` and record each count in the report:

```bash
grep -rln "<h1" src/routes --include=*.tsx | grep -v __tests__ | wc -l           # FE-PAGE-1 (route files with their own h1)
grep -rLn "ListPageShell" $(grep -rln "<h1" src/routes --include=*.tsx | grep -v __tests__) | wc -l
grep -rn "<table" src --include=*.tsx | grep -v __tests__ | grep -v "components/ui/table" | wc -l   # FE-PAGE-4
grep -rn "Loading…\|Loading\.\.\." src --include=*.tsx | grep -v __tests__ | wc -l                 # FE-STATE-2
grep -rn "\"No [a-z]\+ \(yet\|found\)" src --include=*.tsx | grep -v __tests__ | wc -l             # FE-STATE-3
grep -rn "\[\"cp\"\|\['cp'" src --include=*.ts* | grep -v __tests__ | wc -l                       # FE-DATA-2
grep -c "lazy(" src/App.tsx                                                                         # FE-ROUTE-2
```

These greps are rough instruments. Name each as a grep count, with its command, in the register, and do not present it as an exact inventory.

- [ ] **Step 3: Write `docs/en/frontend-rules.md`.** Structure:
  1. **Title, purpose, how to read.** Three or four lines: MUST/SHOULD, ids are stable, and lint-checked rules fail `npm run lint`. Add a link to `ui-conventions.md` as the rationale.
  2. **The rules.** Spec §4.1–§4.11 in the spec's §3 format. Each rule is a short block:

```markdown
#### FE-VISUAL-1 — Colours come from tokens · MUST
Use the token classes defined in `frontend/src/index.css` (`bg-primary`, `text-muted-foreground`, `bg-highlight`, `text-success`, `text-destructive`, …). No raw Tailwind palette class, no arbitrary colour value.
**Do this:** `className="bg-primary text-primary-foreground"`. **Checked by:** lint (`egret/no-raw-palette`). **Why:** the token system carries dark mode and contrast; raw colours broke both in #239.
```

  The rule text comes from §4 with no change of meaning. Paths are written `frontend/src/…` so the link test can resolve them. FE-ROUTE-2 says that the bundle test arrives with the first lazy area (W7).
  3. **Reviewer checklist.** The ids of spec §5, each a checkbox line with its one-line rule.
  4. **Debt register.**
     - Lint rows: one per legacy rule id, with its file count from `legacy.js`, pointing to that file.
     - Review rows: the spec §6 table with Step 2's grep counts and commands.
     - The register opens with the policy: fix when touched, and lists only shrink.
     - A row for `docs/vi/ui-conventions.md` drift.

  Keep it under about 400 lines.

- [ ] **Step 4: Write `docs/vi/frontend-rules.md`.** It translates the English file with the same ids, numbers, paths and counts, and opens with:

```markdown
> Bản tham khảo tiếng Việt. Bản chính là `docs/en/frontend-rules.md`; khi hai bản khác nhau, bản tiếng Anh thắng.
```

- [ ] **Step 5: Write the pointer files and the `ui-conventions.md` lines.** `frontend/CLAUDE.md`:

```markdown
# Frontend

Before changing anything under `frontend/`, read the rulebook: [`../docs/en/frontend-rules.md`](../docs/en/frontend-rules.md) (Vietnamese reference: `../docs/vi/frontend-rules.md`). Its MUST rules are requirements; lint enforces the machine-checkable ones (`npm run lint`), and its reviewer checklist covers the rest.

- Gate: `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`.
- Regenerate the API client with `npm run gen:api`, never bare `orval`.
- No prettier in this tree.
- Never add a file to `eslint-rules/legacy.js`; a file you fix leaves it.
```

`frontend/AGENTS.md`: the same text. In `docs/en/ui-conventions.md`, directly under the `# egret UI conventions` title, add:

```markdown
> **The rules now live in [`frontend-rules.md`](frontend-rules.md), which is normative.** This document is the rationale and the record of reversed decisions that the rulebook links to.
```

The Vietnamese twin gets the same line in Vietnamese, linking `frontend-rules.md`.

- [ ] **Step 6: Run the rulebook test, then the whole gate.** Expected: green. Read the English rulebook once top to bottom against spec §4. Every rule's meaning must match, and any wording change must be one of form only. List any doubt in the report.

- [ ] **Step 7: Commit.**

```bash
git add docs/en/frontend-rules.md docs/vi/frontend-rules.md frontend/CLAUDE.md frontend/AGENTS.md docs/en/ui-conventions.md docs/vi/ui-conventions.md frontend/eslint-rules/__tests__/rulebook.test.js
git commit -m "docs(frontend): the frontend rulebook, its reviewer checklist and debt register, and pointers for agents

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
