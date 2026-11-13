import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Vite serves/builds from ui/; the Tauri backend (src-tauri) loads ui/dist.
export default defineConfig({
  root: "ui",
  // Classic JSX: each JSX file imports React, which oxlint's react-in-jsx-scope checks.
  plugins: [react({ jsxRuntime: "classic" })],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  test: { include: ["src/**/*.test.{ts,tsx}"] },
});
