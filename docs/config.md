# Config: `bitti.toml`

A `bitti.toml` in the current directory (or `--config <file>`) holds
what a run takes by default. Flags on the command line win over it.
Paths in it are from its own directory.

```toml
base_url = "http://127.0.0.1:3000"
paths = ["tests"]            # what to run when the command line names nothing
workers = 4                  # or "auto"
retries = 1
reporters = ["github"]       # added to --reporter
junit = "reports/junit.xml"  # also html, cucumber, allure, secrets, evidence, chromium
trace = "on-failure"         # or "all", "off"
compare_report = ["html", "json"]  # what `expect download … compare` keeps
[vars]                       # as --var; --var on the command line wins
region = "us"

[server]                     # the app under test
command = "npm run dev"      # sh -c (cmd /C on Windows)
cwd = "app"                  # where it runs (default: the config's directory)
url = "http://127.0.0.1:3000/health"
timeout_s = 60
# reuse = true               # one already answering at url is used (default, except under CI: CI set and not empty, false or 0)
# env = { PORT = "3000" }

[env.staging]                # --env staging: on top of the rest
base_url = "https://staging.example.com"
[env.staging.vars]
region = "eu"
```

The `[server]` starts before the browser and is ready when its `url`
answers (any status under 500). Its output goes to `.bitti/server.log`,
and it is stopped with everything it started when the run ends. One that
exits or doesn't answer in time ends the run with exit 3 and the end of its
log. Other keys: `timeout_ms`, `wait_ms`, `isolation`, `fail_on_flaky`,
`headed`, `slowmo`, `max_failures`, `forbid_only`. An unknown key is an
error.
