import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri から読む（crates/znd-studio/tauri.conf.json の devUrl / frontendDist と揃える）
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5174, strictPort: true },
  build: { target: "safari16", outDir: "dist", emptyOutDir: true },
});
