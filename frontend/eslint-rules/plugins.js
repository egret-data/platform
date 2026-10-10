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
};
