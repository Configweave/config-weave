import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// The @forge/* deps are `link:`s into the sibling ../forge monorepo (same
// convention as the Rust path deps on ../WCL and ../wscript) — build forge
// once (`just build` there) before installing here. Dev server proxies the
// API + WebSockets to a locally-running weave-server.
export default defineConfig({
  plugins: [solid()],
  resolve: {
    // One copy each of solid (reactivity breaks silently otherwise) and the
    // CodeMirror core (duplicate @codemirror/state throws at editor mount).
    dedupe: ["solid-js", "@codemirror/state", "@codemirror/view", "@codemirror/language"],
  },
  optimizeDeps: {
    // Forge packages ship preserved-JSX source under the `solid` export
    // condition; keep them out of esbuild pre-bundling so vite-plugin-solid
    // compiles them.
    exclude: ["@forge/ui", "@forge/tokens", "@forge/code", "@forge/desktop", "@forge/term"],
  },
  server: {
    proxy: {
      "/api": { target: "http://127.0.0.1:8765", ws: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
