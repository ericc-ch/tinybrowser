"""WPT timeout policy — tweak these numbers, nothing else.

The WebDriver lane reads them through `tinybrowser_wpt.py` and the in-process
`run in-process` lane gets them as its `--normal-budget`/`--long-budget` flags
from `cli.py`. Both lanes therefore abandon a test at the same point; a change
here reaches both or neither.

The runner abandons a test at `(budget + EXTRA_TIMEOUT)` seconds and reports
TIMEOUT. Normal files get DEFAULT_TIMEOUT; files declaring
`<meta name=timeout content=long>` get LONG_TIMEOUT in its place. Slow and
hanging are the same failure here, so both budgets sit far below the page's own
clock (testharness.js: 10 s normal / 60 s long) — the runner kill has to land
first or that clock stays the binding one.
"""

DEFAULT_TIMEOUT = 3  # seconds; upstream 10
LONG_TIMEOUT = 5  # seconds; upstream 60
EXTRA_TIMEOUT = 2  # seconds; upstream 5
