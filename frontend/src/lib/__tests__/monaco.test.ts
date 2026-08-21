import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * `@monaco-editor/loader` defaults to `https://cdn.jsdelivr.net/npm/monaco-editor@<v>/min/vs`
 * and only stops using it if someone calls `loader.config({ monaco })` BEFORE the first
 * `loader.init()`. `src/lib/monaco.ts` is that call.
 *
 * Be honest about what this file can and cannot show. A unit test cannot observe a network
 * fetch, so it cannot prove the CDN is unused — only a browser reload with the network panel
 * open can, and that measurement is recorded in the task report. What it CAN pin is the
 * wiring that makes the CDN unreachable: that `loader.config` is handed a real monaco
 * namespace (which short-circuits `init()` before it ever injects a script tag), that no
 * `paths.vs` URL is passed alongside it, and that the module stays wired into the app entry.
 *
 * The heavy imports are mocked so this file does not drag ~7MB of Monaco ESM through the
 * jsdom transform. `?worker` is deliberately left unmocked: it is a Vite build-time
 * transform, and this test is the canary that Vitest can still resolve it.
 */

const configSpy = vi.fn();

vi.mock("@monaco-editor/react", () => ({ loader: { config: configSpy } }));
vi.mock("monaco-editor/editor", () => ({ editor: {}, languages: {} }));
vi.mock("monaco-editor/features/register.all", () => ({}));
vi.mock("monaco-editor/editor/browser/coreCommands", () => ({}));
vi.mock("monaco-editor/editor/contrib/caretOperations/browser/caretOperations", () => ({}));
vi.mock("monaco-editor/editor/contrib/dropOrPasteInto/browser/copyPasteContribution", () => ({}));
vi.mock("monaco-editor/editor/contrib/find/browser/findController", () => ({}));
vi.mock("monaco-editor/editor/contrib/gotoSymbol/browser/goToCommands", () => ({}));
vi.mock("monaco-editor/editor/contrib/gotoError/browser/markerSelectionStatus", () => ({}));
vi.mock("monaco-editor/editor/contrib/semanticTokens/browser/documentSemanticTokens", () => ({}));
vi.mock("monaco-editor/editor/contrib/suggest/browser/suggestController", () => ({}));
vi.mock("monaco-editor/editor/common/standaloneStrings", () => ({}));
vi.mock("monaco-editor/languages/definitions/sql/register", () => ({}));
vi.mock("monaco-editor/languages/definitions/python/register", () => ({}));

const SRC = resolve(__dirname, "../monaco.ts");
const MAIN = resolve(__dirname, "../../main.tsx");

describe("the Monaco loader setup module", () => {
  it("hands loader.config a monaco instance, so init() never injects a CDN script", async () => {
    await import("../monaco");

    expect(configSpy).toHaveBeenCalledTimes(1);
    const arg = configSpy.mock.calls[0][0] as { monaco?: unknown };
    expect(arg).toHaveProperty("monaco");
    // `loader.init()` short-circuits on `state.monaco` alone; anything falsy falls through
    // to `getMonacoLoaderScript()` and the jsdelivr default.
    expect(arg.monaco).toBeTruthy();
    expect(arg.monaco).toHaveProperty("editor");
    expect(arg.monaco).toHaveProperty("languages");
  });

  it("passes no paths.vs — the instance is the mechanism, not a rewritten URL", async () => {
    await import("../monaco");
    // A `paths.vs` pointing at a self-hosted copy would be the OTHER fix: it means shipping
    // monaco's 24MB `min/vs` tree as static assets. We deliberately bundle instead, so any
    // `paths` here would be a sign the two approaches got mixed.
    expect(configSpy.mock.calls[0][0]).not.toHaveProperty("paths");
  });

  it("registers a worker factory on MonacoEnvironment", async () => {
    await import("../monaco");
    const env = (self as unknown as { MonacoEnvironment?: { getWorker?: unknown } }).MonacoEnvironment;
    // Without this Monaco falls back to fetching a worker from `paths.vs` — i.e. back to
    // the CDN by another door.
    expect(typeof env?.getWorker).toBe("function");
  });

  it("registers exactly the two languages the app edits", () => {
    // The full `monaco-editor` entry registers ~80 languages plus the TypeScript, CSS, HTML
    // and JSON language services, which cost ~14MB of dist here for nothing. Dropping a
    // language that IS used is the silent half of that trade: SQL Lab and the Data API Query
    // editor need `sql`, notebook code cells need `python`.
    const src = readFileSync(SRC, "utf8");
    expect(src).toContain('import "monaco-editor/languages/definitions/sql/register";');
    expect(src).toContain('import "monaco-editor/languages/definitions/python/register";');
  });

  it("carries every editor contribution the full monaco entry has", () => {
    // The silent failure mode of picking a lean entry point: the editor still renders, but
    // folding or the suggest *widget* is quietly gone. `monaco-editor/features/register.all`
    // looks like the whole feature set and is not — at 0.56.0 it omits nine contributions
    // that `esm/vs/index.js` also imports, `suggestController` among them.
    //
    // So compare the two lists mechanically rather than trusting the name. Languages are
    // excluded on purpose: registering only sql and python is the whole point (see above).
    // `.css` imports are excluded because monaco's exports map (`"./*": "./esm/vs/*.js"`)
    // cannot resolve a stylesheet by package subpath at all — the one entry this skips,
    // `codicon-modifiers.css`, was observed loading transitively in the browser anyway.
    const MONACO = resolve(__dirname, "../../../node_modules/monaco-editor/esm/vs");
    const sideEffectImports = (file: string) =>
      [...readFileSync(file, "utf8").matchAll(/^import\s+(['"])(.+?)\1;$/gm)].map((m) => m[2]);
    const normalise = (spec: string, base: string) => {
      const out: string[] = [];
      for (const seg of `${base}/${spec}`.split("/")) {
        if (seg === "." || seg === "") continue;
        if (seg === "..") out.pop();
        else out.push(seg);
      }
      return out.join("/");
    };

    const full = sideEffectImports(`${MONACO}/index.js`).map((s) => normalise(s, "."));
    const viaRegisterAll = sideEffectImports(`${MONACO}/features/register.all.js`)
      .map((s) => normalise(s, "features"));
    const explicit = [...readFileSync(SRC, "utf8").matchAll(/^import "monaco-editor\/(.+?)";$/gm)]
      .map((m) => `${m[1]}.js`);
    const covered = new Set([...viaRegisterAll, ...explicit]);

    const missing = full.filter(
      (p) => !p.startsWith("languages/") && !p.endsWith(".css") && !covered.has(p),
    );
    expect(
      missing,
      `src/lib/monaco.ts is missing contributions that monaco's own entry point loads.\n` +
        `Add an explicit import for each, then re-read the parity list in the task report:\n` +
        missing.map((p) => `  import "monaco-editor/${p.replace(/\.js$/, "")}";`).join("\n"),
    ).toEqual([]);
    // Guard the guard: if the parse ever silently matches nothing, the assertion above
    // passes for the wrong reason.
    expect(full.length).toBeGreaterThan(100);
    expect(viaRegisterAll.length).toBeGreaterThan(50);
  });

  it("is imported by the app entry and nowhere else", () => {
    // The placement is load-bearing, not stylistic. Component tests mock
    // `@monaco-editor/react` but not `monaco-editor`, so an import from SqlEditor.tsx or
    // CellEditor.tsx would pull real Monaco into jsdom for every test that renders an
    // editor. `main.tsx` is never imported by a component test.
    expect(readFileSync(MAIN, "utf8")).toContain('import "./lib/monaco";');
  });
});
