// Every plugin whose rules may carry a legacy list.
import jsxA11yX from "eslint-plugin-jsx-a11y-x";
import egret from "./index.js";

// The fork registers itself as `jsx-a11y-x`; egret keeps the familiar
// `jsx-a11y` prefix so rule ids read the same as in the rulebook.
export const plugins = { egret, "jsx-a11y": jsxA11yX };

// egret's primitives render these elements (src/components/ui), so the rules
// see through them (FE-A11Y-1, FE-FORM-2). The fork reads only the
// `jsx-a11y-x` key, not `jsx-a11y`. Shared with scripts/lint-legacy.mjs,
// which must lint with the same settings the main config does.
export const a11ySettings = {
  "jsx-a11y-x": {
    components: { Button: "button", Input: "input", Label: "label", Textarea: "textarea" },
  },
};

// The fork's recommended rules under the `jsx-a11y/` prefix, every enabled one
// at error. Rules the fork leaves off stay off.
const a11yRulesFromFork = Object.fromEntries(
  Object.entries(jsxA11yX.configs.recommended.rules).map(([id, entry]) => {
    const [level, ...options] = Array.isArray(entry) ? entry : [entry];
    const on = level !== "off" && level !== 0;
    return [id.replace(/^jsx-a11y-x\//, "jsx-a11y/"), on ? ["error", ...options] : "off"];
  }),
);

export const a11yRules = {
  ...a11yRulesFromFork,
  // Off by the user's ruling of 2026-10-10: a review rule, FE-A11Y-6.
  "jsx-a11y/no-autofocus": "off",
  // Still an error, but `role="list"` on a `<ul>`/`<ol>` is allowed: Safari
  // (VoiceOver) drops the implicit list role once CSS removes list styling,
  // which the Tailwind reset does to every list (FE-A11Y-1; W7a final review).
  "jsx-a11y/no-redundant-roles": ["error", { ul: ["list"], ol: ["list"] }],
  // Still an error, with the fork's own options, plus `log`: a scrollable
  // `role="log"` region takes `tabIndex={0}` so the keyboard can scroll it, as
  // axe's scrollable-region-focusable expects (FE-A11Y-3; the user's ruling of
  // 2026-10-11, W7b Task 5). Every other non-interactive role stays refused.
  "jsx-a11y/no-noninteractive-tabindex": [
    "error",
    {
      ...jsxA11yX.configs.recommended.rules["jsx-a11y-x/no-noninteractive-tabindex"][1],
      roles: [...jsxA11yX.configs.recommended.rules["jsx-a11y-x/no-noninteractive-tabindex"][1].roles, "log"],
    },
  ],
  // Still an error, with the fork's own options, plus `treegrid` on a
  // `<table>`: WAI-ARIA's treegrid pattern is a table whose rows form a tree,
  // and the namespaces list is one (FE-A11Y-1; spec
  // 2026-10-14-editor-completion-and-tree-a11y-design.md §3). The fork maps
  // `table` only to `grid`.
  "jsx-a11y/no-noninteractive-element-to-interactive-role": [
    "error",
    {
      ...jsxA11yX.configs.recommended.rules["jsx-a11y-x/no-noninteractive-element-to-interactive-role"][1],
      table: ["grid", "treegrid"],
    },
  ],
};
