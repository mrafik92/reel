import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  base: "./", // relative paths so the bundle works inside webOS/Tizen packages
  server: {
    proxy: { "/api": { target: "http://localhost:8420", changeOrigin: true, ws: true } },
  },
  build: { target: "es2019", cssTarget: "chrome68" },
});
