# Benchmarks

Numbers: [bench run 42779688203](https://github.com/andrey-usa/bitti/actions/runs/42779688203), 2026-11-10.

Playwright's own TodoMVC suite (`demo-todo-app.spec.ts`, 24 tests) and its one-to-one bitti port, on the same local app and the same browser binary, headless, every run passing all 24 tests. Run on GitHub's `ubuntu-24.04`: AMD EPYC 9V45 96-Core Processor, 4 cores, 15 GB, Linux-7.0.0-1017-azure-x86_64-with-glibc2.39.

Time is the whole command, the median of 5 runs taken in turns; CPU is user and system time of every process. Runner memory is bitti's own process, or Playwright's node or bun processes; browser memory is every browser process; each is its peak RSS, median of the runs.
PSS, in brackets, splits each shared page between the processes that share it: the memory a tool really takes. It comes from 2 more runs whose times are not used.

## Chrome for Testing 158.0.8127.18

| tool | workers | time, median (range) | CPU | runner memory | browser memory |
|---|---|---|---|---|---|
| **each tool's default workers** | | | | | |
| bitti 0.1.0 | 2 | **4.01 s** (3.89-4.13) | 9.1 s | 14 MiB (11 PSS) | 1,163 MiB (414 PSS) |
| Playwright Test 1.65.0, Node 24.22.2 | 2 | 7.54 s (7.44-8.79) | 19.1 s | 758 MiB (654 PSS) | 2,116 MiB (564 PSS) |
| Playwright Test 1.65.0, Bun 1.4.4 | 2 | 7.16 s (7.09-7.42) | 18.2 s | 348 MiB (260 PSS) | 2,113 MiB (578 PSS) |
| **one worker each** | | | | | |
| bitti 0.1.0 | 1 | **5.10 s** (5.05-5.30) | 8.1 s | 13 MiB (11 PSS) | 856 MiB (355 PSS) |
| Playwright Test 1.65.0, Node 24.22.2 | 1 | 8.91 s (8.78-9.46) | 15.8 s | 476 MiB (416 PSS) | 1,071 MiB (384 PSS) |
| Playwright Test 1.65.0, Bun 1.4.4 | 1 | 8.55 s (8.44-8.60) | 15.2 s | 239 MiB (194 PSS) | 1,070 MiB (385 PSS) |

## chrome-headless-shell 158.0.8127.18

| tool | workers | time, median (range) | CPU | runner memory | browser memory |
|---|---|---|---|---|---|
| **each tool's default workers** | | | | | |
| bitti 0.1.0 | 2 | **3.08 s** (2.95-3.45) | 6.5 s | 14 MiB (11 PSS) | 741 MiB (270 PSS) |
| Playwright Test 1.65.0, Node 24.22.2 | 2 | 4.89 s (4.64-6.98) | 10.9 s | 756 MiB (652 PSS) | 1,009 MiB (326 PSS) |
| Playwright Test 1.65.0, Bun 1.4.4 | 2 | 4.37 s (4.31-5.61) | 10.0 s | 357 MiB (271 PSS) | 1,012 MiB (306 PSS) |
| **one worker each** | | | | | |
| bitti 0.1.0 | 1 | **4.14 s** (4.10-4.18) | 5.9 s | 13 MiB (11 PSS) | 510 MiB (219 PSS) |
| Playwright Test 1.65.0, Node 24.22.2 | 1 | 6.09 s (5.96-6.11) | 8.7 s | 466 MiB (416 PSS) | 510 MiB (218 PSS) |
| Playwright Test 1.65.0, Bun 1.4.4 | 1 | 5.67 s (5.61-6.03) | 8.1 s | 247 MiB (198 PSS) | 509 MiB (223 PSS) |

## Every run

| browser | workers | tool | each run's time |
|---|---|---|---|
| Chrome for Testing 158.0.8127.18 | one worker each | bitti 0.1.0 | 5.30 s, 5.10 s, 5.05 s, 5.14 s, 5.08 s |
| Chrome for Testing 158.0.8127.18 | one worker each | Playwright Test 1.65.0, Node 24.22.2 | 9.46 s, 8.91 s, 8.98 s, 8.78 s, 8.81 s |
| Chrome for Testing 158.0.8127.18 | one worker each | Playwright Test 1.65.0, Bun 1.4.4 | 8.44 s, 8.56 s, 8.55 s, 8.60 s, 8.50 s |
| Chrome for Testing 158.0.8127.18 | each tool's default workers | bitti 0.1.0 | 4.13 s, 4.07 s, 3.89 s, 4.01 s, 3.97 s |
| Chrome for Testing 158.0.8127.18 | each tool's default workers | Playwright Test 1.65.0, Node 24.22.2 | 7.54 s, 8.79 s, 7.44 s, 7.54 s, 7.56 s |
| Chrome for Testing 158.0.8127.18 | each tool's default workers | Playwright Test 1.65.0, Bun 1.4.4 | 7.16 s, 7.42 s, 7.09 s, 7.14 s, 7.19 s |
| chrome-headless-shell 158.0.8127.18 | one worker each | bitti 0.1.0 | 4.14 s, 4.10 s, 4.11 s, 4.16 s, 4.18 s |
| chrome-headless-shell 158.0.8127.18 | one worker each | Playwright Test 1.65.0, Node 24.22.2 | 6.03 s, 6.11 s, 6.11 s, 6.09 s, 5.96 s |
| chrome-headless-shell 158.0.8127.18 | one worker each | Playwright Test 1.65.0, Bun 1.4.4 | 5.70 s, 5.61 s, 5.66 s, 6.03 s, 5.67 s |
| chrome-headless-shell 158.0.8127.18 | each tool's default workers | bitti 0.1.0 | 3.00 s, 3.08 s, 3.45 s, 3.34 s, 2.95 s |
| chrome-headless-shell 158.0.8127.18 | each tool's default workers | Playwright Test 1.65.0, Node 24.22.2 | 6.98 s, 5.74 s, 4.64 s, 4.89 s, 4.70 s |
| chrome-headless-shell 158.0.8127.18 | each tool's default workers | Playwright Test 1.65.0, Bun 1.4.4 | 5.61 s, 4.37 s, 4.44 s, 4.37 s, 4.31 s |

## Versions and method

bitti built from the run's commit (bitti 0.1.0); @playwright/test 1.65.0 from npm, its latest at the run; Node v24.22.2; Bun 1.4.4, Playwright's CLI run with `bun --bun`, so its workers are Bun too.

The app is tastejs/todomvc's React example at the last commit before it was replaced, with the test ids, the edit field's label and the toggle text demo.playwright.dev has, and a newer todomvc-app-css (`bench/todomvc/app.patch`). The spec is create-playwright's, checked against its SHA-256, with only its URL changed. Playwright runs create-playwright's template config, chromium only, trace off, the list reporter. The port has a scenario for every test and an `expect` for every assertion; where bitti has no list form or chained locator, the check is written out item by item (`bench/todomvc/bt/`).

Every tool starts from a warm-up round that is not counted; then the tools take turns, the order flipped each round. Each run is sampled from `/proc` every 20 ms (`bench/todomvc/memsample.py`), the whole process tree, with orphaned browser processes kept in it.

To run it yourself, with bitti built, Node, Bun, and a Chrome for Playwright and bitti:

```sh
python3 bench/todomvc/setup.py --work /tmp/todomvc
python3 -m http.server 8765 --bind 127.0.0.1 --directory /tmp/todomvc/www &
python3 bench/todomvc/bench.py --bitti target/release/bitti --work /tmp/todomvc \
  --browser chrome=/path/to/chrome --browser-dir /path/to --bun "$(command -v bun)" --json results.json
python3 bench/todomvc/gen_docs.py results.json --page bench.md
```
