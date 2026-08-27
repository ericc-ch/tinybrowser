# 02: Event loop

Type: interview

Question: Does the page worker block on every network call, or does it run a job queue so timers and `fetch` do not freeze script? Which scheduler library, if any?

Answer:

Two layers. Mixing them up is how this looked like “hand-roll vs Tokio.”

1. **HTML jobs (ours).** Parse, run script, fire `setTimeout`, deliver a `fetch` callback. One at a time on the page thread. Tokio does not know those rules. Every browser writes this list. That is not a runtime.

2. **Parking the thread and waking on time or I/O (Tokio).** Sleeping until the next timer, running `ureq` without freezing that thread, later async sockets if we need them. Hand-rolling *this* (timer wheel, epoll, waker bugs) is the footgun.

So: **Tokio current-thread, features `rt` + `time` only** (~+66 KB tuned, measured 2026-08-27). The page thread is that runtime. `net` stays blocking ureq; call it through `spawn_blocking` (Tokio’s extra threads for “this call waits on the network”). Never call `Agent::send()` directly on the page thread; that *is* the Tokio footgun (“do not block the runtime”).

Forbidden: tokio `full`, smol, axum, hyper. Those are a fat scheduler or HTTP *server* stacks. CDP, when it exists, is still a thin `std::net` socket unless a milestone proves otherwise.

If we later need async TCP on the page runtime, add the `net` feature (~+116 KB total in the probe). Do not rewrite `net` as async until that milestone exists.

## Split (what is whose)

We do **not** hand-roll Rust async (`Future`, wakers, epoll). Tokio is that.

We **do** hand-roll the HTML to-do list: an enum of jobs and the spec order to run them. Tokio has no `setTimeout` or `fetch` callback.

| Ours | Tokio |
|---|---|
| Job kinds: parse, run classic script, timer task, `fetch` promise job, navigation | `Runtime::new_current_thread()`, `block_on` |
| “Which job runs next” (HTML tasks vs microtasks later) | `time::sleep` until the next timer should fire |
| Calling QuickJS until that script returns | `spawn_blocking` so ureq `send()` waits on a pool thread |
| Turning “HTTP finished” into a job on the list | Waking the page thread when that pool thread is done |
| Cookie / DOM / bindings | Not involved |

Walkthrough: script calls `fetch(url)` then `setTimeout(fn, 1000)` and returns.

1. Ours: push nothing that runs `fn` yet; start a fetch job; record “run `fn` in 1000 ms.”
2. Tokio: `spawn_blocking(|| agent.send(...))` and `sleep(1000ms)` both live on the runtime.
3. Page thread is idle inside Tokio (not stuck in `send()`), so whichever finishes first can run.
4. If the timer wins: ours runs `fn` as the next HTML job (QuickJS, still on the page thread).
5. If the download wins: ours queues “resolve the fetch promise”; that runs as an HTML job, still one at a time.

`net` stays ordinary blocking code. It does not become async. Tokio only *hosts* the wait.
