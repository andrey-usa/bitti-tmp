# The .bt format

## Files, steps and variables

- Top-level lines: `base <url>`, `set NAME = value`, `use other.bt`,
  `flow name(a, b)`, `step "<phrase>"` (a Gherkin step's steps, see
  below), `auth name [ttl 30m]`, `before all|each`, `after each|all`,
  `scenario "name" @tag…`. Steps are indented under them.
- A step is a navigera command without the `navigera -s <session>`
  prefix (`goto /cart`, `click --text Add`): `goto`, `click`, `fill`, `select`, `press`,
  `check`, `hover`, `scroll`, `upload`, `wait`, `eval`, `set`, `route`,
  `state load`, and the rest. Targets use navigera's locators (`--role`
  with `--name`, `--label`, `--text`, `--placeholder`, `--testid`,
  `--selector`, `--near <text>` for the one in the card or row that shows
  the text, `--nth`). Refs from `ax` (`click 12`) are an error: they
  change between runs. `check --label Terms` ticks a box; `check
  "<question>"` with no locator asks navigera a yes/no question about
  the page (a model call).
- `set` steps are navigera's: `set viewport 390 844`, `set device "iPhone 16 Pro"`
  (55 devices), `set geo 52.52 13.405`, `set locale de-DE`, and
  `set permissions grant|deny|reset geolocation notifications camera
  microphone clipboard` (also `midi`, `sensors`, `background-sync`), which a
  page then sees as granted, denied or still to be asked. (A `set NAME = value` at the top of a file is
  a variable.)
- A relative path a step reads (`upload`, `state load`, `eval --file`, `expect js --file`,
  `init-script --file`, `route --body-file`, `route --har`) is found next to the `.bt`
  file, as `use` and `for each` files are, so a suite runs the same from
  any directory. Files a step writes (`screenshot`, `state save`) land in
  the current directory.
- Network: `route "*/api/*" --body '{…}'` answers in the server's place,
  `route --har api.har` replays a recorded HAR (navigera's `network --har`
  writes one; requests it has no entry for fail, or reach the network with
  `--not-found continue`), and `route "*/api/me" --modify --replace free
  pro` changes the real response (`--set-header 'Name: value'` too).
  `tests/fixtures/har.bt` has both.
- Words split like a shell's, with no expansion; inside double quotes
  `\"` and `\\` stand for themselves and `\n`, `\r` for a line break
  (a textarea's value). `${NAME}` and
  `${NAME:-default}` come from `save`, a flow's arguments, the data row,
  `before all`, `--var`, `set`, then the environment; `$$` (or `\$`) is a
  literal `$`, and quotes don't stop `${…}` (`'${x}'` is filled in too). Also `${run.id}` (one per run: unique
  emails), `${scenario.name}`, and `${download.path}`/`${download.name}`
  after `expect download`.
- `${secret:NAME}`: from the environment, else `--secrets <file>`
  (`NAME=value` lines; the environment wins). Its value is masked as `***` in everything the run
  prints and saves (messages, the page's URL and title, saved values,
  JUnit and JSON lines).
- `${totp:NAME}`: the code an authenticator app shows right now, for a
  sign-in's second step (`fill --label "Authentication code"
  ${totp:SHOP_TOTP}`). NAME is found as a secret's is and holds the
  authenticator's base32 key or the `otpauth://` URI of the site's QR
  code; the key is masked like a secret. A code with under 3 s left
  waits for the next one, so it can't run out before the site checks it.
- A default with spaces is one word: quote it whole,
  `"${GREETING:-hello there}"` (words split before `${…}` is filled in).
- A `set` value can hold `${…}` too (a default, a `--var`, another `set`,
  a secret, the environment): it is filled in once, when a scenario starts.
- `save NAME = <command>` keeps a command's result (`get text …`).
- `print <command>` shows a read command's result in the run's output
  (`print get text --testid total`, `print eval "…"`, `print ax --selector
  main`): no check and no wait, for a look at the page or a value while a
  test is being written. `print http` shows the last `http` answer (its
  status, headers and body), `print body`, `print json <$.path>` and `print
  header <name>` a part of it (JSON indented), and `print "order ${id}"`
  a line with its variables filled in.
- `exec [--timeout-ms <ms>] <program> <args…>` runs a program (no shell:
  `sh -c "…"` or `pwsh -c "…"` for one) in the `.bt` file's directory,
  with `BITTI_BASE_URL` set, for what a browser can't reach: seed a
  database (`psql`, `sqlplus`), call a script. Its exit code and output
  become `${exec.exit}`, `${exec.stdout}` and `${exec.stderr}`, and
  `expect exit 0`, `expect stdout contains SHIPPED`, `expect stderr …`
  check them (the first 16 MB of each). A nonzero exit fails the step,
  with the last line of its stderr, unless a step indented under it checks
  `exit`. It is stopped after 60 s (`--timeout-ms`), and that fails the
  step whatever is under it.

  ```text
  exec ./scripts/order-status.sh ${order}
    expect exit 0
    expect stdout contains SHIPPED
  ```

## HTTP requests

- `http <METHOD> <url> [--json <text> | --body <text> | --json-file <f> |
  --body-file <f> | --form name=value… | --multipart name=value|name=@file…]
  [--query name=value]… [--user name:password | --bearer <token>]
  [--header "Name: value"]… [--no-follow] [--insecure] [--timeout-ms <ms>]`
  makes a
  request outside the page (an API to set up or check what the UI did, or
  an API test of its own: see [API tests](api-tests.md); a
  relative URL takes the base URL). `--form` sends an urlencoded form,
  `--multipart` a `multipart/form-data` one (`@file` uploads the file beside
  the `.bt`, `;type=image/png` after it sets its type), `--query` adds an
  encoded `name=value` to the URL, and `--user ada:${secret:PW}` or
  `--bearer ${secret:TOKEN}` sets the `Authorization` header (Basic or
  Bearer). The scenario's cookies go with it and
  the cookies it sets come back to the browser, so an API sign-in signs
  the page in. Its answer is `${http.status}`, `${http.body}` and
  `${http.ms}` (how long it took); `expect
  status 201`, `expect json $.role is shopper`, `expect header Location
  ends-with /u/1`, `expect body contains …`, `expect time < 300ms` check it,
  and `save id = json
  $.id` (or `header <name>`) keeps a value. A 4xx or 5xx fails the step,
  with the start of the body, unless a step indented under it checks
  `status`. A timeout fails it whatever is under it. Redirects are followed
  (up to 20, a cookie set on the way going to the next hop and to the
  browser). `--no-follow` gives the 3xx itself. `--insecure` accepts a
  certificate nothing vouches for (a local server's own). The client is
  built in (rustls, the operating system's certificates, the proxy from
  `https_proxy`/`http_proxy`/`no_proxy`): no process per request, and
  connections are kept alive between requests, so a call to a local
  server costs about a millisecond and a secret never reaches a command
  line.

  `--graphql <query>` (or `--graphql-file <f>`) with `--variables <json>`
  and `--operation <name>` sends a GraphQL request as its JSON body. An
  answer with `errors` (GraphQL sends them with a 200) fails the step,
  unless a step under it checks `json $.errors`.

  ```text
  http POST /api/users --json '{"email": "${email}", "role": "shopper"}'
    expect status 201
    expect json $.role is shopper
  save user = json $.id
  ```

## Retries, pauses and the clock

- `retry [--timeout-ms <ms>]` with steps indented under it runs them
  again until all of them pass, a little later each time (0.1, 0.25, 0.5,
  then every 1 s), for what happens in the background: a job that
  finishes, an order that ships, a mail that is sent. Each `expect` in it
  looks once (unless it says `--timeout-ms`): the block is the wait. It
  gives up after its `--timeout-ms` (the run's expect timeout, 5 s, by
  default) with the last try's failure.

  ```text
  retry --timeout-ms 20000
    http GET /api/orders/${order}
      expect json $.status is SHIPPED
  ```
- `pause` stops the run there, the browser as the steps before left it,
  open to a second navigera session (the command to attach is printed),
  until Enter. With no terminal it waits `BITTI_PAUSE_S` seconds, and
  under CI not at all unless that is set. A run with a `pause` in it runs
  one worker.
- `clock install [--time 2024-02-02T08:00:00] [--paused]` puts the page's
  time under the scenario's control (navigera's `clock`): `Date`,
  `performance.now` and timers start there, in every page the scenario
  opens; `clock run-for 15m` (or `mm:ss`) advances it, firing the timers
  due on the way in order; `clock fast-forward`, `clock set <time>`,
  `clock pause-at <time>` and `clock resume` do the rest. A session
  timeout, a countdown or a date-dependent page is tested in
  milliseconds, and the next scenario starts on real time.

  ```text
  clock install --time 2024-02-02T08:00:00 --paused
  goto /account
  clock run-for 14:59
  expect text "Signed in"
  clock run-for 1s
  expect text "Session expired"
  ```

## Flows, conditions, sign-ins and hooks

- `flow name(a, b)` at the top level holds steps; `run name x y` runs them
  with `${a}` and `${b}` set. `use other.bt` takes another file's flows
  and `set`s. A failure inside a flow names the flow's own line.
- `optional <step>`: a step that may not apply (a cookie banner that only
  sometimes shows); its failure is noted, not fatal.
- `if <expect clause>` with steps indented under it runs them when the
  clause holds now (or within its `--timeout-ms`): `if --selector
  "#banner" visible`, then `click --text Accept` indented below.
- `auth shopper [ttl 30m]` (`s`, `m`, `h`, `d`, or plain seconds) holds steps that sign in; `as shopper` in a
  scenario starts it signed in. The block runs once per run, in a browser
  context of its own, the first time a scenario needs it (the other
  workers wait for it), and its cookies and storage are saved under
  `.bitti/auth/` (keep that out of git). With a `ttl`, the saved state
  is reused by later runs until it is that old.
- `before all` runs once for the file, before its first scenario, and every
  scenario of the file sees what it saves. If it fails, none of them run.
  `after all` runs once after the last one (a failure there is reported as
  `(after all)`).
- `handoff until <expect clause>`: the steps only a person can do (2FA, a
  CAPTCHA). When the clause already holds, nothing happens; else the
  browser comes on screen, the run waits for the clause (5 minutes, or its
  `--timeout-ms`) or Enter, and the browser goes back off screen. Plain
  `handoff` waits for Enter. With no screen it fails at once and says so.

## Data rows

- Data rows: an indented `examples` line with a table under it, or
  `scenario "…" for each logins.csv`, makes one scenario per row, named
  from the row (`scenario "Ship to ${country}"`); the row's columns are
  variables.

## Checks: expect

- `expect` retries until it holds or its timeout (5 s, `--timeout-ms` on the
  line or the run) runs out; `exit`, `stdout`, `stderr`, `status`, `body`,
  `json` and `header` read the last `exec` or `http` answer once. `is`
  compares numbers as numbers (`1` is `1.0`). A glob (`request`,
  `download`) has `*` for any run of characters and matches the whole URL
  or file name:

| expect | Checks |
|---|---|
| `url`/`title` `is` (or `==`), `contains`, `starts-with`, `ends-with`, `matches` `<value>` | the active tab |
| `text "<t>"`, `no text "<t>"` [`--within <target>`] | visible text, frames included, or inside an element |
| `<target> visible`, `hidden`, `enabled`, `disabled`, `checked`, `unchecked`, `focused`, `editable`, `empty` | an element's state |
| `<target> text`, `value`, `attr <name>`, `style <property>` + a comparison | an element's text, value, attribute or computed style |
| `--selector <css> count` `is`, `>`, `>=`, `<`, `<=` `<n>` | how many match (exactly one `--selector`) |
| `${var}` + a comparison | a saved value |
| `js "<expression>"` (or `eval`) [+ a comparison] | a script in the page: truthy, or its value compared |
| `js --file <script.js>` [+ a comparison] | the same, from a file next to the `.bt` (an expression, or a function body that `return`s its answer): for a check across rows or anything too long to quote |
| `request <METHOD> <url glob> [status <n>]`, `no request <METHOD> <url glob>` | a request the page made in this scenario (`*` in the URL matches anything) |
| `request … json <$.path>` + a comparison | a value in the last matching response's JSON: `$`, `.key`, `[0]`, `["key"]` (`$.items[0].sku`) |
| `request … body` + a comparison | the last matching response's body, as text (`body contains "added to cart"`) |
| `request … count` + a comparison | how many matching requests |
| `cookie <name>` [+ a comparison], `no cookie <name>`, `cookie <name> absent` | a cookie of the scenario's browser context |
| `storage local\|session <key>` [+ a comparison \| `absent`] | Local or Session Storage |
| `download <file glob>`, `no download …` | a finished download (then `${download.path}`, `${download.name}`: the latest match). Each scenario's downloads go to a directory of its own beside its evidence (`.bitti/runs/<time>/<scenario>/downloads`, kept only when something came), so a name never carries over from another scenario; a second file of the same name in one scenario arrives as `name (2).ext` |
| `download <file glob> compare <expected file>` [`--key <columns>`] [compara flags…] | the download is the same table as the expected file beside the `.bt` ([compara](https://github.com/andrey-usa/compara) on the PATH, or `BITTI_COMPARA`): CSV, NDJSON or Parquet, either side; compara's flags (`-i`, `--trim`, `--tolerance`) pass through. A difference fails the step with compara's counts and the reports it kept beside the scenario's downloads: `--report html,json,rows` (the HTML report, the JSON summary of counts and column statistics, CSVs of the changed, added and removed rows) or `none`; the run's `--compare-report` (`compare_report` in `bitti.toml`, default `html`) when the step doesn't say, and compara's own `-o`, `--json` or `--export-dir` win |
| `console` + a comparison, `no console "<t>"` | a console message (any level) |
| `no errors` | no page exceptions or console errors |
| `accessible` [`--selector <css>`] [`--tags wcag2a,wcag2aa,…`] [`--ignore <rule>,…`] | [axe-core](https://github.com/dequelabs/axe-core) (built in, through navigera's `a11y`, in a world of its own: the page never sees a `window.axe`) finds no violation in the page, or under the selector: WCAG 2.0 and 2.1 A and AA by default; `--tags` replaces that set (`--tags wcag2a,wcag2aa,best-practice` to add axe's best practices); a failure names each rule, its impact, the first elements and what to fix |
| `lcp`, `fcp`, `ttfb`, `load`, `inp` `<`, `<=`, `>`, `>=` `<time>` (`2.5s`, `800ms`; a bare number is ms); `cls` + a comparison and a score (`0.1`) | a performance budget for the current page, as the browser measured it (navigera's `vitals`): largest and first contentful paint, time to first byte, the load event (ms from the navigation's start); the layout shift score (its largest session window, shifts after input left out); interaction to next paint, the slowest of the scenario's clicks and key presses on this page so far (put it after them). A miss names the element behind it (`3120 (#hero)`). Not measured yet (the load event still to come, no interaction yet) is retried. Chrome keeps only interactions of 104 ms or more, so `inp < 200ms` passes when every one was quicker, and a budget under 104 ms is refused. Run under `set cpu 4` and `set network fast-3g` to measure as a mid-tier phone |
| `exit` [`is`] `<n>`, `stdout`/`stderr` + a comparison | what the last `exec` gave |
| `status` [`is`] `<n>`, `body` + a comparison, `json <$.path>` [+ a comparison], `header <name>` [+ a comparison] | the last `http` answer; a JSON object or array compared with `is` is compared as JSON (key order and `1` vs `1.0` don't matter), with `contains` as a part of it (`json $ contains '{"role": "shopper"}'`: the keys it names, at any depth; for an array, items that some item matches) |
| `json <$.path> count` + a comparison | how many items the array (or keys the object) at the path has |
| `json [<$.path>] schema <file.json>[#<pointer>]` | the answer (or the value at the path) is valid against a JSON Schema beside the `.bt`, or one inside a larger document (`openapi.json#/components/schemas/Item`: an OpenAPI 3 document in JSON, its `$ref`s read in the whole document); a failure lists where it isn't (`$[1].price: expected number, got string "5"`). See [API tests](api-tests.md) for what it reads |
| `json [<$.path>] snapshot <name>` [`--ignore <$.path>,…`] | the answer (or the value at the path) is what its baseline beside the `.bt` holds, `<file>.bt.snapshots/<name>.json`, compared as JSON; `--ignore $.id,$.items[*].created` leaves out what changes from run to run. With no baseline yet the run writes it and the step fails, so a person reads it first; `--update-snapshots` writes or replaces it. A difference names each path (`$.total: 38, was 36`) |
| `time` `<`, `<=`, `>`, `>=` `<time>` (`300ms`, `1.5s`) | how long the last `http` exchange took, redirects included |
| `ax [<target>] matches` + the tree indented under it | the page's (or the target's) accessibility tree, as `navigera ax` prints it, holds those lines: partially (`button` matches `button "Place order"`) and in order, a line indented under another inside what that one matched, refs ignored, `/regex/` (spaces allowed) for a name or a value |
| `screenshot <name> [<target>]` [`--full-page`] [`--threshold 0.2`] [`--max-diff-pixels <n>` \| `--max-diff-ratio <r>`] [`--mask <css>`]… | the viewport (or the element, or the whole page) as its baseline shows it, pixel by pixel; see below |
| `not …` | any of them, negated |

## Checks: soft, ax and screenshots

`--soft` on any `expect` notes a failure and goes on; the scenario fails
at its end with all of them. A `${var}` can stand for any compared value
(a count, a regex), checked once it is filled in; `--timeout-ms` takes a
number as written.

```text
expect ax --role form --name Checkout matches
  - textbox "Email" value=/.+@example\.com/
  - combobox "Country" value="Canada"
  - button "Place order"
```

Each template line matches a tree line holding its words in order,
indented lines inside the line above's subtree. Sibling `- text:` lines
count as one text on both sides, so a template keeps matching however
navigera splits adjacent text into lines.

A screenshot's baseline is beside the `.bt` file, in
`<file>.bt.screenshots/<name>-<os>.png` (fonts differ between systems,
so each OS keeps its own). Keep it with the tests. With none yet the run
writes it and the step fails, so a person looks at it first.
`--update-snapshots` (or `--update-screenshots`) writes or replaces
baselines and passes. While it
is taken, animations and transitions stop, the caret hides and each
`--mask` element is painted over in magenta (a mask that matches nothing
fails the step). An element is scrolled into view first, inside its
scroll containers and frames, and cropped where it is on the page, in an
iframe too. Two pixels differ when their
colour distance is over `--threshold` (0 to 1). `--max-diff-pixels` and
`--max-diff-ratio` (a share of the pixels, 0 to 1) set how many may
differ. When both are given, both must hold, and with neither, none may. A
mismatch writes this run's image and a diff (the differing pixels red) to
the evidence directory and names both.

Copy the lines from `navigera ax` and keep the ones that matter. A miss
names the first line that has no place and the nearest line of its role
(`no line matches \`- combobox "Country" value="Mexico"\`; nearest:
\`- combobox "Country" value="Canada" options: …\``, the line as `ax`
prints it).
