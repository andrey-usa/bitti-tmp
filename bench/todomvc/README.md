# todomvc

bitti against Playwright Test, on Node and on Bun, on Playwright's own
TodoMVC suite: the 24 tests of `demo-todo-app.spec.ts` from the
create-playwright package, and their one-to-one bitti port in `bt/`, on the
same local app and the same browser binary. `.github/workflows/bench.yml`
runs it weekly and on demand; the docs' Benchmarks pages are written from
that run's results by `gen_docs.py`.

| file | what |
|---|---|
| `setup.py` | the app (tastejs/todomvc's React example at a pinned commit, with `app.patch`), the spec (checked against its SHA-256) and @playwright/test |
| `playwright.config.ts` | create-playwright's template config, chromium only, trace off, list reporter, the browser from `BENCH_CHROMIUM` |
| `bt/` | the bitti port: a `.bt` file per `describe`, the spec's helpers as flows in `common.bt` |
| `bench.py` | the runs: warm-up, then the tools in turns; every run must pass all 24 tests |
| `memsample.py` | one run's wall time, CPU, and peak RSS (and PSS) of the runner and of the browser, apart |
| `gen_docs.py` | README.md's Benchmarks, `docs/benchmarks.md` and `docs/losses.md` from a run |

```sh
python3 bench/todomvc/setup.py --work /tmp/todomvc
python3 -m http.server 8765 --bind 127.0.0.1 --directory /tmp/todomvc/www &
python3 bench/todomvc/bench.py --bitti target/release/bitti --work /tmp/todomvc \
  --browser chrome=/path/to/chrome --browser-dir /path/to --bun "$(command -v bun)" --json results.json
python3 bench/todomvc/gen_docs.py results.json --page bench.md
```
