import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Dev: proxy /api og /derived til FastAPI (fotomaxxer serve på :8000), så dev og
// prod deler samme URL-rom. Prod: `vite build` → web/dist, servert av FastAPI på /.
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8000",
      "/derived": "http://127.0.0.1:8000",
    },
  },
  build: {
    outDir: "dist",
  },
});
