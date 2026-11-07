/**
 * Copies the published record into public/wpt/data, which is the site's API:
 * every file the page reads is also fetchable at the same path.
 *
 * The history repo is checked out beside the site by the deploy workflow. When
 * there is no run yet the site still builds; the dashboard renders its empty
 * state and the data directory holds only what exists.
 */
import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs"
import { join } from "node:path"

const source = process.env.WPT_DATA_DIR ?? "../data"
const target = "public/wpt/data"

const FILES = ["index.json", "latest.json", "regressions.json"]

// The failure list is the one file the page does not render; it ships so the
// data a run produced is complete, and it is what the next publish diffs.
const SIDECARS = ["failures.jsonl.gz"]

const ALL = [...FILES, ...SIDECARS]

rmSync(target, { recursive: true, force: true })
mkdirSync(target, { recursive: true })

let copied = 0
for (const name of ALL) {
  const from = join(source, name)
  if (!existsSync(from)) continue
  copyFileSync(from, join(target, name))
  copied += 1
}

// The history is public; keep the attribution and the layout next to it.
const readme = join(source, "README.md")
if (existsSync(readme)) copyFileSync(readme, join(target, "README.md"))

if (copied === 0) {
  console.warn(`sync-data: no record in ${source}; the dashboard will render its empty state`)
} else {
  console.log(`sync-data: ${copied} files from ${source}`)
}
