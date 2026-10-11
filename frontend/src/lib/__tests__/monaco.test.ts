import { describe, expect, it, vi } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
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
 * namespace (which short-circuits `init()` before it ever injects a script tag) and that no
 * `paths.vs` URL is passed alongside it.
 *
 * The second property this file guards is that the bundle stays LAZY. `App.tsx` has no lazy
 * routes, so anything statically reachable from the app entry lands in the one eager chunk
 * the login screen downloads — and Monaco is roughly the size of the rest of the app. A
 * single stray static `import "@/lib/monaco"` silently doubles first load, and nothing about
 * the app would look or behave any differently. So the import graph is asserted, not trusted.
 *
 * The heavy imports are mocked so this file does not drag ~7MB of Monaco ESM through the
 * jsdom transform. `?worker` is deliberately left unmocked: it is a Vite build-time
 * transform, and this test is the canary that Vitest can still resolve it.
 */

const configSpy = vi.fn();

vi.mock("@monaco-editor/react", () => ({
  default: function EditorStub() {
    return null;
  },
  loader: { config: configSpy },
}));
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
vi.mock("monaco-editor/languages/definitions/yaml/register", () => ({}));

const SRC_DIR = resolve(__dirname, "../..");
const SRC = resolve(SRC_DIR, "lib/monaco.ts");
const WRAPPER = resolve(SRC_DIR, "components/monaco/MonacoEditor.tsx");

/** Every .ts/.tsx under `src`, so the import-graph assertions below cannot be dodged by
 *  putting the offending import in a directory this test did not think to look in. */
function tsFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const full = resolve(dir, e.name);
    if (e.isDirectory()) return tsFiles(full);
    return /\.tsx?$/.test(e.name) ? [full] : [];
  });
}

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

  it("registers exactly the three languages the app edits", () => {
    // The full `monaco-editor` entry registers ~80 languages plus the TypeScript, CSS, HTML
    // and JSON language services, which cost ~14MB of dist here for nothing. Dropping a
    // language that IS used is the silent half of that trade: SQL Lab and the Data API Query
    // editor need `sql`, notebook code cells need `python`, the flow editor needs `yaml`.
    const src = readFileSync(SRC, "utf8");
    const registered = [...src.matchAll(/^import "monaco-editor\/languages\/definitions\/(\w+)\/register";$/gm)]
      .map((m) => m[1]);
    expect(registered).toEqual(["sql", "python", "yaml"]);
  });

  it("hands monaco-yaml its own worker and every other label the editor worker", async () => {
    // jsdom has no Worker; record what each `?worker` constructor asks for instead.
    const urls: string[] = [];
    vi.stubGlobal("Worker", class {
      constructor(url: URL | string) { urls.push(String(url)); }
    });
    try {
      await import("../monaco");
      const env = (self as unknown as { MonacoEnvironment: { getWorker: (id: string, label: string) => unknown } })
        .MonacoEnvironment;
      env.getWorker("x", "yaml");
      env.getWorker("x", "editorWorkerService");
      env.getWorker("x", "json");
    } finally {
      vi.unstubAllGlobals();
    }
    expect(urls).toHaveLength(3);
    expect(urls[0]).toMatch(/monaco-yaml\/yaml\.worker/);
    expect(urls[1]).toMatch(/monaco-editor\/esm\/vs\/editor\/editor\.worker/);
    expect(urls[2]).toBe(urls[1]);
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

  it("is reachable only through one dynamic import, so it stays out of the eager chunk", () => {
    const referrers = tsFiles(SRC_DIR)
      // Any `__tests__` directory, not just this one: a test elsewhere that stubs the module
      // (`vi.mock("@/lib/monaco")`) is a referrer in the text but not in the bundle, and
      // excluding only `resolve(__dirname)` made this guard fail on a test added under
      // `components/monaco/__tests__` — a false positive that says nothing about laziness.
      .filter((f) => f !== SRC && !f.includes("__tests__"))
      .filter((f) => /["']@\/lib\/monaco["']|["']\.\.?\/(?:lib\/)?monaco["']/.test(readFileSync(f, "utf8")))
      .map((f) => f.slice(SRC_DIR.length + 1));

    // Exactly one referrer. Two would mean the second one might be static; zero would mean
    // the bundle is dead code and no editor can load at all.
    expect(referrers).toEqual(["components/monaco/MonacoEditor.tsx"]);

    // …and that one referrer must reach it with `import(`, not a static import. This is the
    // whole mechanism: a static import here puts ~3.8MB into the chunk every page loads.
    const wrapper = readFileSync(WRAPPER, "utf8");
    expect(wrapper).toContain('lazy(() => import("@/lib/monaco"))');
    expect(wrapper).not.toMatch(/^import .*"@\/lib\/monaco"/m);
  });

  it("is the only door to @monaco-editor/react — editor components go through the wrapper", () => {
    // Bypassing the wrapper is the other way to lose laziness: `Editor` imported straight
    // from `@monaco-editor/react` renders fine and pulls the loader (and then Monaco) in
    // eagerly. Type-only imports are erased at build time and are fine.
    const offenders = tsFiles(SRC_DIR)
      .filter((f) => f !== SRC)
      .filter((f) =>
        readFileSync(f, "utf8")
          .split("\n")
          .some((l) => /from "@monaco-editor\/react"/.test(l) && !/^\s*import type /.test(l)),
      )
      .map((f) => f.slice(SRC_DIR.length + 1));
    expect(offenders).toEqual([]);
  });
});
