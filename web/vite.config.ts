import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 開發時（不在 Tauri 內，直接用瀏覽器開）把 /api 轉給本機 Runtime 的既定埠
const RUNTIME_DEV_TARGET = "http://127.0.0.1:47821";

export default defineConfig({
  plugins: [react()],
  server: { port: 5173, strictPort: true, proxy: { "/api": RUNTIME_DEV_TARGET } },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
  },
});
