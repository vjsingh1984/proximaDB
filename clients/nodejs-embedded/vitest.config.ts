/**
 * Vitest configuration for the ProximaDB TypeScript SDK.
 *
 * Tests live under tests/ and are TypeScript-aware (esbuild via vitest).
 * No special setup files are required.
 */
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    environment: "node",
    globals: false,
  },
});
