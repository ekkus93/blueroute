import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Vite serves/builds from ui/; the Tauri backend (src-tauri) loads ui/dist.
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  test: { include: ["src/**/*.test.{ts,tsx}"] },
});
