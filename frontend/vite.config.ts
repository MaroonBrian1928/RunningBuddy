import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:3000",
      "/strava": "http://127.0.0.1:3000"
    }
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test.setup.ts"
  }
});
