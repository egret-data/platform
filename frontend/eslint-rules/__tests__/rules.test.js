// @vitest-environment node
import { RuleTester } from "eslint";
import tseslint from "typescript-eslint";
import noRawPalette from "../rules/no-raw-palette.js";
import noArbitraryTextSize from "../rules/no-arbitrary-text-size.js";
import noWindowConfirm from "../rules/no-window-confirm.js";
import noNativeSelect from "../rules/no-native-select.js";

const tester = new RuleTester({
  languageOptions: { parser: tseslint.parser, parserOptions: { ecmaFeatures: { jsx: true } } },
});

const bad = (code, token) => ({ code, errors: [{ messageId: "raw", data: { token } }] });

tester.run("no-raw-palette", noRawPalette, {
  valid: [
    '<div className="bg-primary text-muted-foreground" />',
    'cn("text-destructive", x && "bg-highlight")',
    '"text-sm"',
    'const c = "#ff0000"',
    '"border-border"',
  ],
  invalid: [
    bad('"bg-green-600"', "bg-green-600"),
    bad('"dark:bg-red-500"', "dark:bg-red-500"),
    bad('"hover:text-sky-500/20"', "hover:text-sky-500/20"),
    bad("`p-2 ${a} border-amber-200`", "border-amber-200"),
    bad('"bg-[#0af]"', "bg-[#0af]"),
    bad('"text-[oklch(0.5_0.1_200)]"', "text-[oklch(0.5_0.1_200)]"),
    bad('"!bg-rose-950"', "!bg-rose-950"),
    bad('"-from-teal-50"', "-from-teal-50"),
    bad('"bg-red-500!"', "bg-red-500!"),
    bad('"bg-red-500/[0.3]"', "bg-red-500/[0.3]"),
    bad('"bg-red-500/(--o)"', "bg-red-500/(--o)"),
  ],
});

const size = (code, token) => ({ code, errors: [{ messageId: "size", data: { token } }] });

tester.run("no-arbitrary-text-size", noArbitraryTextSize, {
  valid: ['"text-sm"', '"text-[var(--x)]"', '"text-[#fff]"'],
  invalid: [
    size('"text-[11px]"', "text-[11px]"),
    size('"md:text-[0.75rem]"', "md:text-[0.75rem]"),
    size("`text-[13.5px] ${x}`", "text-[13.5px]"),
    size('"text-[11px]!"', "text-[11px]!"),
    size('"text-[11px]/5"', "text-[11px]/5"),
  ],
});

const conf = (code) => ({ code, errors: [{ messageId: "confirm" }] });

tester.run("no-window-confirm", noWindowConfirm, {
  valid: [
    "const confirm = () => true; confirm();",
    "function f(confirm) { confirm(); }",
    "dialog.confirm()",
  ],
  invalid: [
    conf('window.confirm("x")'),
    conf('globalThis.confirm("x")'),
    conf('confirm("x")'),
    conf('self.confirm("x")'),
    conf('window.confirm?.("x")'),
    conf('function g() { if (!window.confirm("Discard?")) return; }'),
  ],
});

const sel = (code) => ({ code, errors: [{ messageId: "select" }] });

tester.run("no-native-select", noNativeSelect, {
  valid: [
    "<Select value={v} onValueChange={f}><SelectTrigger /></Select>",
    "<SelectPrimitive.Root />",
    '<input type="text" />',
    'document.createElement("div")',
  ],
  invalid: [
    sel('<select value={v} onChange={f}><option value="a">A</option></select>'),
    sel("<select />"),
    sel("const x = cond ? <select aria-label=\"x\" /> : null;"),
  ],
});
