import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Default env stays "node" so the 19 existing pure tests are unchanged.
    // Component tests opt into jsdom with a `// @vitest-environment jsdom` pragma on line 1.
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    setupFiles: ["src/test/setup.ts"],
  },
});
