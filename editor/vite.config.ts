import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Builds straight into the Leptos crate's public/ dir — cargo-leptos copies
// everything there into site-root at build time and serves it as a static
// asset. Single-file output on purpose: this is loaded via a plain
// <script type="module"> only on /wiki routes (see wiki_editor.rs), not
// bundled into the app's own wasm, so there's no reason to split chunks.
export default defineConfig({
  plugins: [react()],
  // A dependency somewhere in the BlockNote/Mantine chain references the
  // bare Node global `process.env.NODE_ENV` rather than Vite's own
  // `import.meta.env` — Vite only string-replaces that pattern in code it
  // transforms itself, which doesn't reliably catch every pre-bundled
  // dependency, so it's thrown at runtime in the browser ("process is not
  // defined") unless replaced explicitly here too.
  define: {
    "process.env.NODE_ENV": JSON.stringify("production"),
  },
  build: {
    outDir: "../crates/web/public",
    emptyOutDir: false,
    lib: {
      entry: "src/main.tsx",
      formats: ["es"],
      fileName: () => "wiki-editor.js",
    },
    rollupOptions: {
      output: {
        // Keep the CSS filename predictable too, for the <link> tag in
        // wiki_editor.rs — Vite's default is derived from the entry name,
        // which lib mode already makes "wiki-editor.css", but pinning it
        // here avoids that being an undocumented coincidence.
        assetFileNames: "wiki-editor[extname]",
      },
    },
  },
});
