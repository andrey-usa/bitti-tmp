# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.1] - 2026-11-12

### Security

- A secret with a `"`, a `\` or another character JSON or a URL writes
  differently was masked in the printed report but stood escaped in
  `trace.jsonl`, and percent-encoded in a failure's URL. Each written
  form of a `${secret:…}` value is now replaced by `***`.
- From navigera: a cookie an `http` step's answer sets is taken only for
  the answering host or a parent of it (another site's `Domain` was
  stored as given), and a `Domain` cookie now reaches the subdomains it
  names.
- The screenshot taken for each traced step passed through the shared
  temp directory under a name another account on the machine could guess
  (`bitti-step-<pid>-<n>.jpg`), readable by all. It now goes through this
  user's own temp directory (`navigera-<uid>`, mode 0700).

### Fixed

- The GitHub action's source build (a runner with no release binary, or
  `version: source`) checked navigera out at a branch that no longer
  exists and stopped on git's `pathspec` error. `navigera-ref` defaults
  to `next`, and a ref navigera does not have is named in the error.

### Removed

- A failing step no longer asks a decision model for the element it
  meant (`TYPESAFE_API_KEY` or `NAVIGERA_DECIDER_URL`): navigera dropped
  its decision model, which saved agents no turn and cost more in its
  measurements. "nearest on the page" lists bitti's own ranking, as it
  always did without a model.

## [0.2.0] - 2026-11-10

### Fixed

- A time limit that ran out is named exactly: `--global-timeout-ms 150`
  read "the run's 0.1 s" in the failing step and "0.15 s" in the line
  about the scenarios not run, and a 40 ms limit read "0.0 s". Both say
  `0.15 s` and `0.04 s` now.
- A `step "…"` pattern containing `"` or `\` never matched its
  `.feature` step: the run stored the pattern Debug-escaped and matched
  against the escaped text. It matches as written now.
- A step pattern may hold at most 32 placeholders. Each one is three
  capture groups in the compiled regex, so a pattern with hundreds
  compiled into gigabytes (800 took 2.7 GB and 16 s) before it could be
  refused; it is refused at the definition now.

### Changed

- `--workers` defaults to `auto` (half the CPUs, at most 8, never more
  than there are scenarios) instead of 1, as Playwright Test does. Each
  worker is a browser context in the one shared browser. Scenarios that
  must run alone keep `@serial`; `--workers 1` (or `workers = 1` in
  `bitti.toml`) gives the old order. `--electron`, `--attach` and a
  `pause` step still run one at a time, and say so only when
  `--workers` was given.
- A file named by `use` is read and parsed once per run, however many
  `.bt` files use it; each step pattern is compiled once per run, and
  navigera reads each shared step once. `bitti lint` on generated suites
  takes 56.7% fewer instructions than 0.1.0 on the small one and 81.2%
  fewer on one 4 times its size, which costs 4.50 times the small one's
  work instead of 10.34 (cachegrind, [gate run
  42723794673](https://github.com/andrey-usa/bitti/actions/runs/42723794673)).

### Added

- `expect inp < 200ms`: interaction to next paint, the slowest of the
  scenario's interactions on the page so far, beside `lcp`, `fcp`,
  `ttfb`, `load` and `cls`. Every budget now reads navigera's `vitals`,
  and a miss names the element behind it (`expect lcp < 2.5s`: `3120
  (#hero)`). Chrome keeps only interactions of 104 ms or more, so a
  budget of 104 ms or more is decided either way and one under it is
  refused.
- `expect accessible` runs navigera's `a11y`: axe-core in a world of
  its own in the page, which never sees a `window.axe`, and one copy of
  it, stored deflated in navigera, where bitti carried its own 652 KB.
  Messages are as before.
- `@serial` (or `@exclusive`): a scenario that runs while no other one
  does, for one that changes what the others read. With `--workers` the
  others run in parallel first, then each `@serial` one alone.
- `${totp:NAME}`: the code an authenticator app shows now, for a
  sign-in's second step, from a secret (the environment or `--secrets`)
  holding its base32 key or `otpauth://` URI, masked like one.
- From navigera: `passkey on` (a virtual authenticator: passkey sign-up
  and sign-in with no person or phone), kept by `state save` and so by
  `auth`; `set cpu 4` (a slower phone's main thread); `screenshot` of one
  element.

## [0.1.0] - 2026-11-08

The first release.

### Added

- `record`: a click that takes its page or its element away (a link, a
  banner's Accept) is written as a link's or button's role and name, and
  a field's step by its role and its label's own words (not a select's
  options), when `locate` can't finish first; before, such a click was
  sometimes recorded as `--text`.
- `--video on-failure|all` (also `video` in bitti.toml): what the page
  showed, frame by frame with the time of each, in the scenario's
  evidence (`video/index.html` plays it), named in the console and JSON
  lines and linked from `--html`.
- API tests with no browser: a scenario of `http`, `exec` and checks of
  their answers runs without Chrome, and a run of only such scenarios
  starts none. `http` sends `--form`, `--multipart`, `--query`, `--user`
  `--bearer` and `--graphql` (an answer with `errors` fails), keeps a cookie jar per scenario (an API sign-in can be
  an `auth` that page scenarios start from), and is answered by `expect
  time`, `json … count`, `json … schema` (a schema file, or one inside an
  OpenAPI document: `openapi.json#/components/schemas/Item`), `json … snapshot`
  (`--update-snapshots`) and `print`. The HTTP client is built in
  (navigera's, over rustls), with kept-alive connections: about a
  millisecond a request to a local server, where a `curl` process took
  6-10.
- `clock` steps (navigera's): a page's dates and timers start at a given
  time, paused or flowing, and run forward on command, so a session
  timeout or a countdown is tested in milliseconds.
- `--scenario-timeout-ms`: the most a scenario's steps may take, each
  step's wait cut at what is left (`@slow` triples it), and
  `--global-timeout-ms` for the whole run.
- `retry` blocks, a `pause` step, `--slowmo <ms>`, tag expressions,
  `--grep-invert`, `--max-failures`/`-x`, `--last-failed`, and `@skip`,
  `@fixme`, `@only` (`--forbid-only`, on under CI) and `@slow`.
- `bitti run --watch` runs the scenario files, then runs again each
  one that changes, until Ctrl-C.
- `bitti merge <dir|file.jsonl>...` joins the `--reporter jsonl`
  output of shards into one console summary, one `--junit` and one
  `--html`.
- A `set permissions grant|deny|reset <name>…` step (navigera's) is
  documented, and `set device` knows 55 devices.
- `--trace` keeps each step's page: an ax tree and a JPEG of the viewport
  per step beside `trace.jsonl`, shown by `bitti trace <dir> <line>`,
  and `--html` shows a traced scenario's steps as a timeline that opens
  to them.

- Plain-text web tests (`.bt`) and Gherkin `.feature` files that run on real
  Chrome through navigera, in one native binary with no Node.
- `bitti run` (workers, a browser context per scenario, retries,
  `--repeat` and `--shuffle`, traces), `lint` and `list`.
- Writing tests from what was done in a browser: `record`, and
  `from-session` from a navigera session's history.
- `fix`: a failing step's nearest matches on the page, written into the
  file with `--use`.
- `trace` for a recorded run, `skill` (the agent skill), `mcp` (an MCP
  server with navigera's browser tools and bitti's own), and
  `bitti navigera …`, navigera itself, built in.
- A GitHub Action (`action.yml`) and a container image (`Dockerfile`).
- `--electron <app>` (`electron` and `electron_args` in `bitti.toml`):
  scenarios test an Electron desktop app's windows, a fresh app per
  scenario, one at a time. `expect js --main "<code>"` checks the app's main
  process; `eval --main` runs code there as a step.
- `--attach <port|ws://…>` (`attach` in `bitti.toml`): run against a
  browser or an Electron app already running with a DevTools port. An app
  attached to isn't restarted between scenarios (they go on where the one
  before left it) and keeps running after the run; one worker.
- `help` and `--help` print the usage, as running it bare does.
- `expect request … body` + a comparison: the last matching response's
  body as text, for a response that isn't JSON or a check on its wording.
- `expect js --file <script.js>` [+ a comparison]: a check from a file next
  to the `.bt` (an expression, or a function body that `return`s its
  answer), for one across rows or too long to quote on a line.
- `bitti <command> --help` prints that command's usage and options.
- `print <command>`: a step that shows a read command's result (a value,
  a locator's text, the page's tree) under its scenario in the run's
  output, checking nothing and waiting for nothing.
- `lint` refuses what would fail only in the run: `set NAME = value` under
  a scenario (a file's variable sits at the top; a step `set` is the
  browser's), flags that can't go together (`--text` with `--nth`), and a
  `set` of a setting the browser doesn't have.
- The agent skill (`bitti skill`) leads with the loop that costs an
  agent least: write the scenario from the task, `lint`, run, and fix from
  the failure, which names the closest elements as locators; then `print`,
  one browser session with `--raw` for looking, and `from-session`. It is
  shorter, every example in it runs as shown, and it says nothing of MCP.
- A failure's note that a click changed nothing is left out once a check
  after the click has held (the page did answer it).
- `lint` also refuses a second `base` line (the last one used to win
  silently) and a `js --file` script that isn't there, under `if` and
  `optional` too, where it read as a check that didn't hold. A repeated
  scenario name says where the first is; `use` of a missing file says
  where it is looked for, without `(os error 2)`.
- `if expect text "…"` is the same as `if text "…"`: the word is optional.
- An `if` on `js --file` finds its script next to the `.bt`, as `expect`
  does, from any directory.
- An `expect` that cuts a long value says how many characters it left out
  and that a `print` step shows the value whole; a quote left open says
  how to write one inside quotes and shows the line.
- `expect js` is bounded by its step's time: a script that never settles
  fails at 5 s (or `--timeout-ms`), saying so, not after 35 s with a
  DevTools method name. An `optional` step that did not hold shows as
  `skip` in a trace and its HTML; `bitti trace <dir> <line>` says when
  there is no such line, and a directory that isn't there says so.
- With a `[server]`, the browser starts while the app does instead of after
  it answers, so its launch is off the run's critical path (a bench-site
  run with a server that takes 1.5 s: 6.8 s -> 6.5 s here; 7.3 s -> 5.9 s
  on a slower machine). A server that fails to start also closes the
  browser. `BITTI_PHASES=1` prints where a run's time goes.
