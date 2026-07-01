import { defineConfig, configDefaults } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    // Playwright e2e specs live under tests/e2e and run via `npm run test:e2e`,
    // not Vitest — exclude them so `vitest run` doesn't try to execute them.
    exclude: [...configDefaults.exclude, "tests/e2e/**"],
  },
});
