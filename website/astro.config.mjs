import tailwindcss from "@tailwindcss/vite"
import { defineConfig } from "astro/config"
import { fileURLToPath } from "node:url"

// The WPT dashboard is a route, not a base path: the home page is the same
// site, and the raw data lives under /wpt/data so a run can be fetched with
// curl. No client framework: every page is rendered once at build time and the
// two interactive pieces are a few lines of inline script.
export default defineConfig({
  site: "https://tinybrowser.ericc.ch",
  vite: {
    plugins: [tailwindcss()],
    resolve: {
      alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
    },
  },
})
