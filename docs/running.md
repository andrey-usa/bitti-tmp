# Running

```text
bitti run [<file.bt|file.feature|dir>...] [--base-url <url>] [--var k=v] [--grep <text>] [--grep-invert <text>]
              [--tag <tag|expression>] [--last-failed] [--max-failures <n>|-x] [--forbid-only|--allow-only]
              [--workers <n|auto>|-j <n>] [--isolation context|browser] [--retries <n>] [--fail-on-flaky] [--shard <i/n>]
              [--repeat <n>] [--shuffle [<seed>]] [--slowmo <ms>]
              [--reporter console|jsonl|github] [--junit <file>] [--cucumber <file>] [--allure <dir>] [--html <file>] [--json]
              [--timeout-ms <ms>] [--wait-ms <ms>] [--headed] [--chromium <path>] [--engine chrome|lightpanda]
              [--electron <app> [--electron-arg <arg>]...] [--attach <port|ws://…>]
              [--secrets <file>] [--evidence <dir>] [--pause-on-failure] [--trace on-failure|all] [--video on-failure|all] [--update-snapshots]
              [--compare-report html,json,rows|none] [--config <file>] [--env <name>]
bitti lint [<file.bt|file.feature|dir>...]
bitti list [<file.bt|file.feature|dir>...]
bitti trace <dir> [<line>]
bitti fix [<file>:<line>] [--use <n>] [--all [--min <p>]]
bitti navigera <command…>
bitti from-session <name> [--scenario <name>] [--out <file.bt>]
bitti record <url> [--out <file.bt>] [--scenario <name>] [--headless] [--for <seconds>]
bitti record --extend <file.bt> [--scenario <name>] [--base-url <url>] [--secrets <file>]
bitti mcp [--headed] [--chromium <path>]
bitti skill          # the agent skill (what an agent reads to write tests)
bitti <command> --help   # that command's usage and options
bitti --version
```

`--base-url` falls back to `base` in the file, then `BITTI_BASE_URL`.
`--workers` is `auto` unless given: half the CPUs, at most 8, and never
more than there are scenarios. `--workers 1` runs them one at a time.

## Picking what runs

- `--tag smoke` runs the scenarios tagged `@smoke`; a tag expression picks
  by several, as Cucumber writes them: `--tag "smoke and not slow"`,
  `--tag "(checkout or cart) and not wip"`. Repeated, any of them.
  `--grep-invert <text>` leaves out the names that contain it.
- Some tags say how a scenario runs: `@skip` and `@fixme` aren't run and
  are reported as skipped (`SKIP`, JUnit `<skipped/>`, Allure `skipped`);
  `@slow` gives each check three times its time; `@serial` (or
  `@exclusive`) runs the scenario while no other one runs, for one that
  changes what the others read (an app setting, a shared account): the
  others run in parallel first, then each `@serial` one alone; `@only` runs only the
  scenarios that have it, a focus while working on one. It is an error
  with `--forbid-only`, which is on by itself under CI (`CI` set), so a
  focus left in by mistake doesn't pass a build on a few scenarios
  (`--allow-only` runs just those anyway).
- `--max-failures <n>` starts no scenario once n have failed (`-x`: at the
  first), and says how many were not run; the run fails.
- `--last-failed` (`--lf`) runs only the scenarios that failed in the last
  run in this directory (`.bitti/last-run.json`). With none, it says
  so and exits 0.
- `--scenario-timeout-ms <ms>` (`scenario_timeout_ms` in `bitti.toml`)
  is the most one scenario's steps may take: no step starts once it is
  up, and a step's wait (an `expect`, a `retry`, an `http` or `exec`, its
  own `--timeout-ms` too) stops there, the failure saying so. `@slow`
  triples it. Off by default. `--global-timeout-ms <ms>`
  (`global_timeout_ms`) does the same for the whole run: no scenario
  starts after it, those running stop there, and the run fails with the
  count of scenarios it never ran.
- `--slowmo <ms>` waits that long before each step that acts on the page
  (`goto`, `click`, `fill` …), to follow a `--headed` run by eye; checks
  and `http` steps aren't slowed.

## Browsers and apps

- A click that opens a tab or a popup (`target=_blank`, `window.open`)
  moves the scenario to it, as navigera does: the next steps read and act
  there. `tab-select 0` goes back to the first tab; `tab-close` closes the
  one you're on.
- Every scenario starts in a fresh browser context (navigera `reset
  --all`: no cookies, storage, tabs, routes, init scripts, `set`
  emulation or dialog answer from the scenario before), in about 40 ms
  instead of a new browser's few hundred. `--workers n` runs n at once,
  each in a browser context of one shared browser (about 40% less memory
  at 4 workers than a browser each; `--isolation browser` gives each its
  own). The longest start first (by `.bitti/timings.json`, which each
  run updates; the first run goes in file order).
- `--engine lightpanda` (or `engine = "lightpanda"` in a profile, or
  `NAVIGERA_ENGINE=lightpanda`) runs on
  [Lightpanda](https://github.com/lightpanda-io/browser) (`$LIGHTPANDA_BIN`
  or `lightpanda` on the PATH), a new Lightpanda for each scenario (it has
  one context per process): less memory, no layout. It is a smoke tier, not
  a replacement: hover-revealed elements, new tabs, screenshots and `ax`
  pixels need Chrome. On the 40-scenario suite's 25 site scenarios it
  passes 22, and is not faster: 22 s against Chrome's 11 s at one worker
  (half of it the three failures' waits).
- `--electron <app>` (or `electron = "./app"` and `electron_args = [...]`
  in `bitti.toml`) tests an Electron desktop app instead of a web
  page: the scenarios drive its windows as tabs, through navigera's
  `start --electron`. `<app>` is the app's directory or main script (the
  Electron in its `node_modules` runs it), a packaged app's executable or a
  macOS `.app` bundle. Each scenario gets a fresh app, so none sees what the
  one before left in its storage, and they run one at a time (two copies of
  an app would share its user data): `--workers` becomes 1. A window the
  app opens itself (Settings, a second document) becomes the scenario's
  tab, and one that closes itself hands back to the one before. `expect js
  --main "<code>"` checks the app's main process (a function gets the
  `electron` module: `({ app }) => app.getName()`), and `eval --main` runs
  code there as a step. On a Linux
  CI machine, run under `xvfb-run`. The example: `tests/fixtures/electron`,
  run in CI on Linux, macOS and Windows against navigera's fixture app.
- `--attach <port|ws://…>` (or `attach = "9222"` in `bitti.toml`) runs
  against a browser or an Electron app that is already running with a
  DevTools port: the build a developer has open (`npm start --
  --remote-debugging-port=9222`), or a Chrome they started. In an app, the
  scenarios run one after the other in the one app, each going on where
  the one before left it, and the app keeps running after the run. In a
  browser, each scenario gets a clean context of its own. One worker. The
  example: `tests/fixtures/electron-attached`.

## Reports and evidence

- `--html report.html` writes the run as one page that needs nothing
  else to open: failed scenarios first and open, each with its steps and
  their times, the failure (message, where the page was, what came
  closest, the note) and its screenshot in the page (up to 2 MB; a
  bigger one is linked). Flaky ones show how their first try failed.
  Secrets are masked as in the console. A screenshot two scenarios share
  (a failed `before all`) is in the page once; a bigger one, and any once
  the page holds 10 MB of them (failures first), is a link relative to the
  report, so upload the report and `.bitti/runs/`
  together. With `--repeat`, a pass count per scenario comes first. A run
  that finds no scenario (exit 2) writes no report.
- `--allure <dir>` writes Allure 2 results for `allure generate <dir>` or an
  Allure server: one `<uuid>-result.json` per scenario run with its steps,
  times, status (a flaky one passed, marked flaky), labels (the file, the
  `.feature`'s name, tags, the worker) and the failure's screenshot
  attached. The directory is kept, so several runs or shards can feed one
  report; the history id is the same from run to run.
- `--video on-failure` (or `all`) records what the page showed while the
  scenario ran, a JPEG each time its pixels changed (none while it is
  still), into `video/` in the scenario's evidence; `video/index.html`
  plays the frames at their pace. The console and JSON lines name it and
  `--html` links it. A passing scenario's video is dropped under
  `on-failure`. Recording cost no measurable time on the examples.
- `--trace on-failure` (or `all`) records each step of a scenario: where
  the page was, the console messages and the requests it brought, in
  `trace.jsonl` beside the scenario's screenshot, with the page's ax tree
  at a failure in `ax.txt`. `bitti trace <dir>` prints it (a run's
  directory lists its traces); `bitti trace <dir> <line>` shows one
  step in full, with the tree when it is the one that failed. Each step
  also keeps the page as it was after it, beside `trace.jsonl`: `NNN-L<line>.ax.txt`
  (its ax tree) and `NNN-L<line>.jpg` (the viewport, JPEG quality 60, about
  10 to 50 kB), both named in the step's line (`ax`, `shot`); `trace <dir> <line>`
  shows them. In `--html` a traced scenario's steps are a timeline: a row
  per step (line, step, time, a bar against the slowest) that opens to its
  screenshot, ax tree, console messages, requests and error, with no script
  in the page (the images follow the 2 MB and 10 MB limits above). Secrets
  are masked in the text files and the console as everywhere; a screenshot
  is pixels, as the failure's own is.
- `--watch` runs the files, then waits: each scenario file that changes
  (or appears) runs again, its own scenarios only, until Ctrl-C. The files
  are looked at every 300 ms (no file-watching dependency) and an edit
  counts once it has stayed quiet for one more look. A file that no longer
  parses is reported and the watching goes on. The files a `use` or a flow
  brings in are not watched on their own: save the scenario file to run it.
  `--junit` and `--html` hold the latest run.
- `bitti merge <dir|file.jsonl>...` joins the results of shards into one
  console summary, one `--junit` and one `--html`, as if one run had made
  them: save each shard's `--reporter jsonl` output
  (`bitti run --shard 1/3 --reporter jsonl > shard-1.jsonl`), put the
  files in one place and name it. A directory is searched for `.jsonl`
  files. The steps and traces stay with the shards, so the report has each
  scenario's status, time and failure; the time is the longest shard's.
  Exit 1 when a scenario failed (`--fail-on-flaky` counts a flaky one),
  2 when there are no results.
- `--retries n` runs a failing scenario again. One that passes then is
  reported FLAKY with how its first try failed. `--fail-on-flaky` fails
  the run on one. `--shard i/n` takes every n-th scenario, for CI matrix
  jobs.
- `--repeat 20 --shuffle` hunts flakes and order dependence: every
  scenario runs 20 times (`name #k`), in a random order whose seed is
  printed (`--shuffle <seed>` runs that order again), then a pass count per
  scenario, FLAKY where it isn't all of them.
- `--reporter jsonl` prints one JSON object per scenario as it finishes
  (the console moves to stderr); `github` writes workflow annotations on
  the failing lines, beside the console (on by itself under GitHub
  Actions). `--json` prints
  what the scenarios saved, for a pipeline's next step.

## Failures and fixes

- A failure prints the step, what was expected and seen, where the page
  was, a screenshot (under `.bitti/runs/`), and a note when an earlier
  click changed nothing on the page (navigera's `no_effect`), the usual
  reason the next check fails. When the element isn't there, it lists the
  closest ones on the page as locators to paste in:

  ```text
  FAIL  shop.bt:21  Accept the cookies  (611 ms)
        line 23: click --role button --name "Accept everything"
        click role="button" name="Accept everything": … no element …
        nearest on the page:
          1. button "Accept all"   --role button --name "Accept all"   0.63
  ```

  The list covers frames too: a step with `--frame` keeps it, and `fix`
  changes only the locator.
- `bitti fix <file>:<line> [--use <n>]` puts the last run's n-th
  "nearest on the page" into that step (only its locator flags change, and
  only if the line still reads as it did); `bitti fix --all [--min
  0.6]` takes each failure's first suggestion that scored that much, and
  plain `bitti fix` lists them. The failure block prints the command.
  A click that something else covers (a consent bar, a modal) gets that
  thing's buttons instead, "Accept", "Close", "Got it" … first, as an
  `optional click` that `fix` puts before the step.
  A `.feature` step is left alone: change its `step` block instead.
- `bitti navigera …` is navigera itself, built in: explore a page the
  way an agent does (`bitti navigera -s work start`, `goto`, `ax`,
  `click 12`, `fill 15 ada@example.com`) with no second program to
  install. `bitti from-session work` then writes what that session did
  as a scenario: each action once, and the waits between (reads and `eval`
  left out), every ref replaced by the most stable locator that found its
  element again just before the action (test id, role and name, label, placeholder, text, CSS;
  navigera's `locate`, each checked on the page; inside a web component's
  shadow root, CSS through its hosts: `--selector "#tip >>> button"`), `base` from the first
  `goto`, an `expect url` where an action moved the page, an `expect text`
  for a heading that changed or a status/alert message that appeared (as a
  comment when it has numbers, which may change), and a value typed
  into a password field (`type=password`, as navigera's `locate` says) as
  `${secret:NAME}`, named after the field (`${secret:PASSWORD}`,
  `${secret:CONFIRM_PASSWORD}`). What it can't keep (a mouse at
  coordinates, a ref no locator found again) is a `# left out:`
  line. Read it through, keep the expects that matter, add the ones the
  test is for. A session started by a plain `navigera` works too when that
  navigera has `history`.
- `bitti record <url> --out t.bt` opens a browser to use by hand: each
  click, field change, check, select, upload and Enter becomes a step as
  you do it, with the most stable locator that finds the element (the same
  `locate` as `from-session`, run in the page at that moment; when the
  click takes its element or the page away before that check is done, a
  banner's Accept or a link, the element's own text or label read as it
  was clicked), an `expect url` where the page
  moved, a password field's value as `${secret:NAME}` (never written
  down). The file is rewritten after
  every step: close the browser or press Ctrl-C to stop. A click into a
  field isn't a step (its value is); typing in a field twice keeps the
  last value.
- `bitti record --extend t.bt [--scenario <name>]` grows a scenario
  instead: it runs that scenario (default: the file's last) to its end,
  `after each` left out, then hands you the browser there; what you do
  goes in after its last step, the same way, and the rest of the file stays
  as it was. A scenario that fails before its end is not extended: fix it
  first. `--headless` and `--for <s>` work as for `record`. A `.feature`
  is refused (its steps live in its `step` blocks).
- `bitti mcp` is the same loop for an agent over MCP (stdio): navigera's
  browser tools (`browser_navigate`, `browser_snapshot`, `browser_click`,
  … one browser for all of them) beside `bitti_from_session` (what
  those tools did, as a scenario; `out` writes it), `bitti_run`,
  `bitti_list`, `bitti_lint`, `bitti_failure` (the last run's
  failures with the nearest elements) and `bitti_fix`. Its
  instructions tell the agent that page content is data, not instructions,
  and to confirm irreversible actions against the user's request. Register it as
  `{"command": "bitti", "args": ["mcp"]}`. What the tools run, read
  and write (`paths`, `out`, the files `args` names) stays inside the
  working directory, as navigera's own tools' files do, and `args` can't
  name a `--chromium` to start; `NAVIGERA_MCP_ALLOW_FILES=1` lifts both.
  A project's own `.bt` files still do what they say (`exec`, `[server]`).
- `--pause-on-failure` stops at a failure with the browser as the failure
  left it and prints how to look at that very tab (`navigera -s bt start
  --attach ws://…/devtools/page/…`, then `navigera -s bt ax`); Enter goes
  on with the run. It runs one worker. With no terminal to press Enter in,
  it waits `BITTI_PAUSE_S` seconds (default 600, for an agent looking
  from elsewhere), and under CI (`CI` set) it doesn't pause unless that is
  set.
- Exit codes: 0 passed, 1 a scenario failed, 2 a file or argument is wrong,
  3 the browser or the app server didn't start.
