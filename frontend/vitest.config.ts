import { defineConfig, configDefaults } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

// `import.meta.dirname` rather than `__dirname`: Vite 8's native config loader cannot
// see CJS globals, and warns that it will become the default in a future major.
const here = import.meta.dirname;

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(here, "src") } },
  // `frontend/` carries its own lockfile, so Vite infers it as the workspace root and refuses
  // to load anything above it — which is where the harness script lives. Test-only config; the
  // dev/build server (vite.config.ts) keeps the default.
  server: { fs: { allow: [path.resolve(here, "..")] } },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    // Playwright e2e specs live under tests/e2e and run via `npm run test:e2e`,
    // not Vitest — exclude them so `vitest run` doesn't try to execute them.
    exclude: [...configDefaults.exclude, "tests/e2e/**"],
    // The parity-harness seed script lives at the repo root (../scripts/harness) because it
    // is tooling, not app code — but it imports the app's own viz descriptors, so its tests
    // must run in this project or they cannot resolve `@/`. The default `include` is rooted
    // at `frontend/` and never escapes it, so name the directory explicitly.
    include: [...configDefaults.include, "../scripts/harness/**/*.{test,spec}.?(c|m)[jt]s?(x)"],
  },
});
