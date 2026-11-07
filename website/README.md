# Website

The public site: the home page and the WPT dashboard at `/wpt`. Astro, Tailwind
v4, no client framework. Every page is rendered at build time.

    bun install
    WPT_DATA_DIR=../data bun run dev

`WPT_DATA_DIR` points at a checkout of
[`tinybrowser-dev/tinybrowser-wpt-results`](https://github.com/tinybrowser-dev/tinybrowser-wpt-results).
Without it the dashboard renders its empty state, which is what the first
deploy looks like.

## The data the page reads

The dashboard is a static snapshot. `scripts/sync-data.ts` copies the record
from the results repo into `public/wpt/data` before `astro build`, so the files
the page reads are also the files a script can fetch:

| file                  | contents                                              |
| --------------------- | ----------------------------------------------------- |
| `latest.json`         | the newest run: totals, per-folder counters, pin, SHA |
| `index.json`          | one entry per run: date, SHA, totals                  |
| `regressions.json`    | failure-list diff against the previous run            |
| `failures.jsonl.gz`   | every failing test and subtest, one JSON object a line |

`latest.json`, `index.json`, and `regressions.json` come from
`python3 tools/wpt/cli.py publish` in the `WPT` workflow. There is no API
version: the shapes above are what this site consumes, and a change to one is a
change to the page in the same commit.

A run produced by a different WPT revision is not comparable, so
`regressions.json` reports `comparable: false` and an empty diff across a pin
bump. New `TIMEOUT`s are listed separately from new failures: they flip with
runner load.

## Deploy

`.github/workflows/website.yml` builds and deploys to GitHub Pages on two
triggers: a push that touches `website/**`, and the completion of a `WPT` run
(whose publish step may have moved the data). The custom domain is
`tinybrowser.ericc.ch`; `public/CNAME` carries it into the artifact.

## Theme

The palette is the shadcn `registry:theme` published at
`https://ericc.ch/r/registry.json`: six monochrome tokens mapped onto shadcn's
CSS variables in `src/styles/global.css`. Pass/fail/warn/crash are the one
addition, because a conformance table needs them; they are `light-dark()` pairs
so both schemes work from one declaration.
