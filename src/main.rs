//! bitti: plain-text web tests on real Chrome, run by one native binary
//! through navigera. See README.md.

mod allure;
mod axtree;
mod config;
mod expect;
mod fix;
mod from_session;
mod gherkin;
use navigera::http;
mod mcp;
mod merge;
mod nearest;
mod record;
mod report;
mod runner;
mod schema;
mod server;
mod shot;
mod suite;
mod trace;
mod watch;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The static Linux build's allocator: musl's own is several times slower
/// on the small allocations of a large page's snapshot, which navigera
/// takes in this process.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// The agent skill: the format and the commands, for an agent writing tests.
const SKILL_MD: &str = include_str!("../skills/bitti/SKILL.md");

const USAGE: &str = "\
bitti: plain-text web tests on real Chrome (through navigera)

usage: bitti <command> [...]

commands:
  run [<file|dir>...]    run scenarios (default: every .bt and .feature file
                         under the current directory)
  lint [<file|dir>...]   check files without a browser
  list [<file|dir>...]   the scenarios a run would take
  merge <dir|file>...    join the jsonl results of shards into one report
  trace [<dir>] [<line>] a trace a run recorded (--trace): every step, or one
  fix [<file>:<line>]    put a suggestion from the last run into a failing step
  record <url>           a browser to use by hand; what you do becomes a
                         scenario
  from-session <name>    what a navigera session did, as a scenario
  navigera <command…>    navigera's commands, built in
                         (bitti navigera -s work goto <url>)
  mcp                    an MCP server: navigera's browser tools and
                         bitti's own
  skill                  the agent skill: the .bt format, steps and expect forms
  --version

`bitti <command> --help` lists a command's options. Steps, locators and
expect forms: `bitti skill`.

exit codes: 0 all passed, 1 a scenario failed, 2 a file or an argument is
wrong, 3 the browser or the app server didn't start
";

/// `run`'s options, for `bitti run --help`.
const RUN_OPTIONS: &[(&str, &str)] = &[
    ("--base-url <url>", "where relative URLs (`goto /cart`) point (else `base` in the file, BITTI_BASE_URL)"),
    ("--var <name=value>", "a variable for ${name} (repeatable)"),
    ("--secrets <file>", "NAME=value lines for ${secret:NAME} (the environment comes first); masked in output"),
    ("--grep <text>", "only scenarios whose name contains it"),
    ("--grep-invert <text>", "leave out scenarios whose name contains it"),
    (
        "--tag <expr>",
        "only scenarios with this @tag, or a tag expression: \"smoke and not slow\", \"(a or b) and not wip\" (repeatable: any of them)",
    ),
    ("--last-failed", "only the scenarios that failed in the last run (also --lf)"),
    ("--max-failures <n>", "start no scenario once n have failed (-x: 1); the rest are reported as not run"),
    ("--forbid-only", "a scenario tagged @only is an error (on by itself under CI); without it, @only runs just those"),
    ("--workers <n|auto>", "scenarios at once (default auto: half the CPUs, at most 8); also -j <n>"),
    ("--isolation <how>", "context (default): one browser, a browser context per worker; browser: a browser each"),
    ("--retries <n>", "run a failing scenario again, up to n times; one that then passes is flaky"),
    ("--fail-on-flaky", "a flaky scenario fails the run"),
    ("--shard <i/n>", "run the i-th of n slices (for CI matrix jobs)"),
    ("--repeat <n>", "run every scenario n times (flake hunting: each run is reported, then a pass count)"),
    ("--shuffle [<seed>]", "run in a random order (the seed is printed; give it again to repeat the order)"),
    (
        "--reporter <name>",
        "console (default), jsonl (one JSON object per scenario on stdout), github (workflow annotations; on by itself under GitHub Actions); repeatable",
    ),
    ("--config <file>", "settings, a [server] and [env.<name>] profiles (default: ./bitti.toml)"),
    ("--env <name>", "the config's [env.<name>] profile on top of the rest"),
    (
        "--update-snapshots",
        "a screenshot or JSON snapshot that differs from its baseline (or has none) becomes it (also --update-screenshots)",
    ),
    ("--compare-report <k>", "what `expect download … compare` keeps: html, json, rows (comma-separated) or none (default html)"),
    (
        "--trace <when>",
        "on-failure or all: each step's page (ax tree, JPEG), console and requests beside the screenshot in trace.jsonl; read it with `bitti trace`, or in --html",
    ),
    (
        "--video <when>",
        "on-failure or all: what the page showed, frame by frame, in the scenario's evidence (video/index.html plays it; linked from --html)",
    ),
    ("--junit <file>", "also write JUnit XML"),
    ("--cucumber <file>", "also write Cucumber JSON for the .feature files (step by step)"),
    ("--allure <dir>", "also write Allure results (a JSON file per scenario run, screenshots attached)"),
    ("--html <file>", "also write the run as one HTML page (steps, times, failures with their screenshot)"),
    ("--json", "print what the scenarios saved, as one JSON object, at the end"),
    ("--timeout-ms <ms>", "how long an expect retries (default 5000)"),
    ("--scenario-timeout-ms <ms>", "the most one scenario may take (its steps; hooks around it aside); @slow triples it"),
    ("--global-timeout-ms <ms>", "the most the whole run may take: no scenario starts after it, running ones stop there, and the run fails"),
    ("--wait-ms <ms>", "how long a step waits for its element (default 5000)"),
    ("--headed", "show the browser"),
    ("--slowmo <ms>", "wait this long before each step that acts on the page (to watch a --headed run)"),
    ("--chromium <path>", "the browser to run"),
    ("--engine lightpanda", "run on Lightpanda (no layout: a fast smoke tier; screenshots and pixel checks fail)"),
    (
        "--electron <app>",
        "test an Electron app: its directory or main script (Electron from its node_modules) or an executable; a fresh app per scenario, one at a time",
    ),
    ("--electron-arg <arg>", "an argument for the app (repeatable; --electron-arg=<arg> for one starting with -)"),
    (
        "--attach <port|ws://…>",
        "run against a browser or an Electron app already running with a DevTools port (one worker; an app attached to goes on from one scenario to the next)",
    ),
    (
        "--evidence <dir>",
        "failure screenshots and downloads (default .bitti/runs/<time>; a directory you name is used as given, links included)",
    ),
    ("--watch", "after the run, run again each scenario file that changes (polled every 300 ms), until Ctrl-C"),
    ("--pause-on-failure", "keep the browser at a failure, open to `navigera --attach`, until Enter (one worker)"),
];

/// `text` after `prefix`, in lines of at most 80 columns; lines after the
/// first start with `indent` spaces.
fn wrapped(prefix: &str, text: &str, indent: usize) -> String {
    let mut out = String::new();
    let mut line = prefix.to_string();
    let mut fresh = true;
    for word in text.split_whitespace() {
        if !fresh && line.chars().count() + 1 + word.chars().count() > 80 {
            out.push_str(line.trim_end());
            out.push('\n');
            line = " ".repeat(indent);
            fresh = true;
        }
        if !fresh {
            line.push(' ');
        }
        line.push_str(word);
        fresh = false;
    }
    out.push_str(line.trim_end());
    out.push('\n');
    out
}

/// `bitti <command> --help`: that command's usage and options.
fn help_for(cmd: &str) -> Option<String> {
    let text = match cmd {
        "run" => {
            let mut out = String::from(
                "usage: bitti run [<file.bt|file.feature|dir>...] [options]\n\n\
                 Runs the scenarios in the files, or in every .bt and .feature file under\n\
                 the directories (default: the current one).\n\noptions:\n",
            );
            for (flag, what) in RUN_OPTIONS {
                out.push_str(&wrapped(&format!("  {flag:<22} "), what, 25));
            }
            out
        }
        "lint" => "usage: bitti lint [<file|dir>...] [--var <name=value>] [--secrets <file>]\n\n\
                   Checks .bt and .feature files without a browser: syntax, unknown steps,\n\
                   and variables nothing sets (warnings: the run's environment may set them).\n"
            .to_string(),
        "list" => "usage: bitti list [<file|dir>...] [--grep <text>] [--tag <tag>]\n\
                   \x20                     [--shard <i/n>]\n\n\
                   The scenarios a run with the same options would take, one per line:\n\
                   file:line, name and tags.\n"
            .to_string(),
        "trace" => "usage: bitti trace [<dir>] [<line>]\n\n\
                    A trace that `run --trace on-failure|all` recorded: every step with its\n\
                    page, console and requests, or the step at <line> in full, with its\n\
                    screenshot file and ax tree. <dir> is a run's directory\n\
                    (.bitti/runs/<time>) or one scenario's in it; by default, the\n\
                    latest run's.\n"
            .to_string(),
        "merge" => merge::HELP.to_string(),
        "fix" => fix::HELP.to_string(),
        "from-session" => "usage: bitti from-session <navigera session> [--scenario <name>]\n\
                           \x20                             [--out <file.bt>]\n\n\
                           Writes what a navigera session did as a scenario, its refs turned into\n\
                           locators that hold between runs.\n"
            .to_string(),
        "record" => "usage: bitti record <url> [--out <file.bt>] [--scenario <name>] [--headless]\n\
                     \x20                       [--for <seconds>]\n\
                     \x20      bitti record --extend <file.bt> [--scenario <name>]\n\
                     \x20                       [--base-url <url>] [--secrets <file>]\n\n\
                     Opens a browser to use by hand; what you do becomes a scenario. With\n\
                     --extend, runs the scenario to its end first, then records more steps\n\
                     into it. Needs a display.\n"
            .to_string(),
        "mcp" => "usage: bitti mcp [--headed] [--chromium <path>]\n\n\
                  An MCP server on standard input and output: navigera's browser tools and\n\
                  bitti_from_session, bitti_run, bitti_failure and bitti_fix.\n"
            .to_string(),
        "skill" => {
            "usage: bitti skill\n\nPrints the agent skill: the .bt format, steps and expect forms.\n".to_string()
        }
        _ => return None,
    };
    Some(text)
}

#[derive(Clone)]
struct Args {
    cmd: String,
    paths: Vec<PathBuf>,
    opts: runner::Options,
    junit: Option<PathBuf>,
    cucumber: Option<PathBuf>,
    allure: Option<PathBuf>,
    html: Option<PathBuf>,
    /// bitti.toml's `[server]`.
    server: Option<config::Server>,
    workers: usize,
    /// `--workers` (or `workers` in bitti.toml) was given: a run that can
    /// only take one says it dropped them.
    workers_given: bool,
    retries: u32,
    fail_on_flaky: bool,
    shard: Option<(usize, usize)>,
    reporters: Vec<String>,
    json: bool,
    /// `context` (one browser, a context per worker) or `browser`.
    isolation: String,
    repeat: u32,
    shuffle: Option<u64>,
    /// `--watch`: run again the files that change.
    watch: bool,
    /// `--max-failures <n>` (`-x`: 1): no scenario starts once n have failed.
    max_failures: Option<usize>,
    /// `--last-failed`: only the scenarios that failed in the last run.
    last_failed: bool,
    /// `--forbid-only` (on by itself under CI): a scenario tagged `@only`
    /// is an error, not a filter.
    forbid_only: bool,
}

/// The default and `--workers auto`: half the CPUs this process may use
/// (cgroup quotas and affinity included), at most 8. A run never starts
/// more workers than it has scenarios.
fn auto_workers() -> usize {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    (cpus / 2).clamp(1, 8)
}

fn parse_args() -> Result<Args, String> {
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        return Err(USAGE.to_string());
    }
    let cmd = raw.remove(0);
    if matches!(cmd.as_str(), "help" | "--help" | "-h") {
        return Err(USAGE.to_string());
    }
    // bitti.toml (or --config), and its --env profile: flags before the
    // command line's, which win.
    let mut take = |flag: &str| -> Result<Option<String>, String> {
        match raw.iter().position(|w| w == flag) {
            Some(i) if i + 1 < raw.len() => {
                let v = raw.remove(i + 1);
                raw.remove(i);
                Ok(Some(v))
            }
            Some(_) => Err(format!("{flag} needs a value")),
            None => Ok(None),
        }
    };
    let config_file = take("--config")?;
    let env = take("--env")?;
    let config = config::load(config_file.as_deref().map(std::path::Path::new), env.as_deref())?;
    let mut words = config.as_ref().map(|c| c.profile.words()).unwrap_or_default();
    words.extend(raw);
    let it = words.into_iter();
    let mut paths = Vec::new();
    let stamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let mut opts = runner::Options::new(stamp);
    let mut junit = None;
    let mut cucumber = None;
    let mut allure = None;
    let mut html = None;
    let mut base_flag = None;
    let mut workers = None;
    let mut retries = 0u32;
    let mut fail_on_flaky = false;
    let mut shard = None;
    let mut reporters: Vec<String> = Vec::new();
    let mut json = false;
    let mut isolation = "context".to_string();
    let mut repeat = 1u32;
    let mut shuffle = None;
    let mut watch = false;
    let mut max_failures = None;
    let mut last_failed = false;
    let mut forbid_only = runner::under_ci();
    let mut it = it.peekable();
    while let Some(a) = it.next() {
        if a == "--shuffle" {
            // An optional seed: the next word when it is a number.
            let seed = it.peek().and_then(|w| w.parse::<u64>().ok());
            if seed.is_some() {
                it.next();
            }
            shuffle = Some(seed.unwrap_or(stamp ^ (std::process::id() as u64).rotate_left(32)));
            continue;
        }
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--base-url" => base_flag = Some(val("--base-url")?),
            "--var" => {
                let kv = val("--var")?;
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("--var takes name=value, not {kv:?}"))?;
                opts.vars.insert(k.trim().to_string(), v.to_string());
            }
            "--secrets" => {
                let path = val("--secrets")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("--secrets {path}: {e}"))?;
                opts.secrets = secrets_file(&text).map_err(|e| format!("--secrets {path}: {e}"))?;
            }
            "--pause-on-failure" => opts.pause = true,
            "--watch" => watch = true,
            "--grep" => opts.grep = Some(val("--grep")?),
            "--grep-invert" => opts.grep_invert = Some(val("--grep-invert")?),
            "--tag" => {
                let t = val("--tag")?;
                runner::tag_expr(&t, &[])?;
                opts.tags.push(t);
            }
            "--last-failed" | "--lf" => last_failed = true,
            "--forbid-only" => forbid_only = true,
            "--allow-only" => forbid_only = false,
            "-x" => max_failures = Some(1),
            "--max-failures" => {
                let v = val("--max-failures")?;
                max_failures = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| format!("--max-failures takes a number above 0, not {v:?}"))?,
                );
            }
            "--slowmo" => {
                let v = val("--slowmo")?;
                opts.slowmo = Some(v.parse().map_err(|_| format!("--slowmo takes milliseconds, not {v:?}"))?);
            }
            "--junit" => junit = Some(PathBuf::from(val("--junit")?)),
            "--cucumber" => cucumber = Some(PathBuf::from(val("--cucumber")?)),
            "--allure" => allure = Some(PathBuf::from(val("--allure")?)),
            "--html" => html = Some(PathBuf::from(val("--html")?)),
            "--timeout-ms" => {
                let v = val("--timeout-ms")?;
                opts.expect_ms = v.parse().map_err(|_| format!("--timeout-ms takes milliseconds, not {v:?}"))?;
            }
            "--global-timeout-ms" => {
                let v = val("--global-timeout-ms")?;
                let ms: u64 = v.parse().map_err(|_| format!("--global-timeout-ms takes milliseconds, not {v:?}"))?;
                opts.global_ms = (ms > 0).then_some(ms);
            }
            "--scenario-timeout-ms" => {
                let v = val("--scenario-timeout-ms")?;
                let ms: u64 = v.parse().map_err(|_| format!("--scenario-timeout-ms takes milliseconds, not {v:?}"))?;
                opts.scenario_ms = (ms > 0).then_some(ms);
            }
            "--wait-ms" => {
                let v = val("--wait-ms")?;
                opts.wait_ms = Some(v.parse().map_err(|_| format!("--wait-ms takes milliseconds, not {v:?}"))?);
            }
            "--headed" => opts.headed = true,
            "--update-screenshots" | "--update-snapshots" => opts.update_shots = true,
            "--trace" => opts.trace = trace::Trace::parse(&val("--trace")?)?,
            "--video" => opts.video = trace::Trace::parse_video(&val("--video")?)?,
            "--compare-report" => opts.compare_report = expect::report_kinds(&val("--compare-report")?)?,
            "--chromium" => opts.chromium = Some(val("--chromium")?),
            "--electron" => opts.electron = Some(val("--electron")?),
            "--attach" => opts.attach = Some(val("--attach")?),
            "--electron-arg" => opts.electron_args.push(val("--electron-arg")?),
            a if a.starts_with("--electron-arg=") => opts.electron_args.push(a["--electron-arg=".len()..].to_string()),
            "--engine" => {
                let e = val("--engine")?;
                if e != "chrome" && e != "lightpanda" {
                    return Err(format!("--engine takes chrome or lightpanda, not {e:?}"));
                }
                opts.engine = e;
            }
            "--evidence" => opts.evidence = PathBuf::from(val("--evidence")?),
            "--workers" | "-j" => {
                let v = val("--workers")?;
                workers = Some(if v == "auto" {
                    auto_workers()
                } else {
                    v.parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| format!("--workers takes a number or auto, not {v:?}"))?
                });
            }
            "--retries" => {
                let v = val("--retries")?;
                retries = v.parse().map_err(|_| format!("--retries takes a number, not {v:?}"))?;
            }
            "--fail-on-flaky" => fail_on_flaky = true,
            "--repeat" => {
                let v = val("--repeat")?;
                repeat =
                    v.parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("--repeat takes a number, not {v:?}"))?;
            }
            "--shard" => {
                let v = val("--shard")?;
                let parsed = v
                    .split_once('/')
                    .and_then(|(i, n)| Some((i.parse::<usize>().ok()?, n.parse::<usize>().ok()?)))
                    .filter(|(i, n)| *n > 0 && *i >= 1 && i <= n);
                shard = Some(parsed.ok_or_else(|| format!("--shard takes i/n with 1 <= i <= n, not {v:?}"))?);
            }
            "--reporter" => {
                let v = val("--reporter")?;
                for r in v.split(',') {
                    if !matches!(r, "console" | "jsonl" | "github") {
                        return Err(format!("unknown reporter {r:?} (console, jsonl, github)"));
                    }
                    reporters.push(r.to_string());
                }
            }
            "--json" => json = true,
            "--isolation" => {
                isolation = val("--isolation")?;
                if isolation != "context" && isolation != "browser" {
                    return Err(format!("--isolation takes context or browser, not {isolation:?}"));
                }
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            // One line: the whole usage under it would scroll the error away.
            f if f.starts_with('-') => return Err(format!("unknown option {f} (bitti {cmd} --help lists them)")),
            p => paths.push(PathBuf::from(p)),
        }
    }
    if base_flag.is_some() {
        opts.base = base_flag;
    }
    if paths.is_empty() {
        let from_config = config.as_ref().and_then(|c| c.profile.paths.clone()).unwrap_or_default();
        paths.extend(from_config.into_iter().map(PathBuf::from));
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    if opts.electron.is_some() && opts.engine == "lightpanda" {
        return Err("--electron runs an Electron app; --engine lightpanda is a browser of its own".into());
    }
    if !opts.electron_args.is_empty() && opts.electron.is_none() {
        return Err("--electron-arg passes an argument to an Electron app: add --electron <app>".into());
    }
    if opts.attach.is_some() && opts.electron.is_some() {
        return Err("--attach drives an app that is already running; --electron launches one: pick one".into());
    }
    if opts.attach.is_some() && opts.engine == "lightpanda" {
        return Err("--attach connects to a running browser; --engine lightpanda launches one of its own".into());
    }
    // Annotations alone say nothing about a passing run: the console stays
    // on unless the JSON lines take its place.
    if !reporters.iter().any(|r| r == "console" || r == "jsonl") {
        reporters.push("console".into());
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") && !reporters.iter().any(|r| r == "github") {
        reporters.push("github".into());
    }
    Ok(Args {
        cmd,
        paths,
        opts,
        junit,
        cucumber,
        allure,
        html,
        server: config.and_then(|c| c.profile.server),
        workers: workers.unwrap_or_else(auto_workers),
        workers_given: workers.is_some(),
        retries,
        fail_on_flaky,
        shard,
        reporters,
        json,
        isolation,
        repeat,
        shuffle,
        watch,
        max_failures,
        last_failed,
        forbid_only,
    })
}

/// `NAME=value` lines (`#` comments, optional quotes around the value).
fn secrets_file(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (k, v) = line.split_once('=').ok_or_else(|| format!("line {}: expected NAME=value", i + 1))?;
        let v = v.trim();
        let v = match (v.chars().next(), v.chars().last()) {
            (Some(q @ ('"' | '\'')), Some(e)) if q == e && v.len() >= 2 => &v[1..v.len() - 1],
            _ => v,
        };
        out.insert(k.trim().to_string(), v.to_string());
    }
    Ok(out)
}

/// An I/O error without its `(os error N)`.
fn plain_io(e: &std::io::Error) -> String {
    let text = e.to_string();
    match text.find(" (os error ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

fn load(paths: &[PathBuf]) -> Result<Vec<suite::Suite>, String> {
    let files = suite::discover(paths).map_err(|e| {
        let shown: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        format!("reading {}: {}", shown.join(", "), plain_io(&e))
    })?;
    if files.is_empty() {
        return Err("no .bt or .feature files found".into());
    }
    let mut suites = Vec::new();
    let mut errors = Vec::new();
    for f in files {
        match std::fs::read_to_string(&f) {
            Ok(text) if f.extension().is_some_and(|e| e == "feature") => match gherkin::parse(&f, &text) {
                Ok(s) => suites.push(s),
                Err(e) => errors.push(e.to_string()),
            },
            Ok(text) => match suite::parse(&f, &text) {
                Ok(s) => suites.push(s),
                Err(e) => errors.push(e.to_string()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => errors.push(format!("{}: no such file", f.display())),
            Err(e) => errors.push(format!("{}: {}", f.display(), plain_io(&e))),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    suite::resolve(&mut suites).map_err(|e| e.to_string())?;
    Ok(suites)
}

fn main() -> ExitCode {
    // navigera's own commands, for exploring a page before writing the
    // test (`bitti navigera -s work goto …`); a session they start is
    // served by this executable again (`--session <name> …` first).
    let first = std::env::args().nth(1);
    if first.as_deref() == Some("navigera") {
        // Its help names the program it runs as: `bitti navigera -s s
        // start`, not `bitti -s s start`, and says which navigera this is.
        let rest: Vec<String> = std::env::args().skip(2).collect();
        if rest.is_empty() || rest.iter().any(|a| matches!(a.as_str(), "--help" | "-h" | "help")) {
            println!("(built into bitti {}; `bitti navigera` runs it)\n", env!("CARGO_PKG_VERSION"));
        }
        return navigera::cli::main(std::iter::once("bitti navigera".to_string()).chain(rest));
    }
    if first.as_deref().is_some_and(|a| a == "--session" || a.starts_with("--session=")) {
        return navigera::cli::main(std::env::args());
    }
    // `bitti <command> --help`: that command's own usage and options.
    if let Some(text) = first.as_deref().and_then(help_for) {
        if std::env::args().skip(2).any(|a| a == "--help" || a == "-h") {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
    }
    // `.bitti` holds run evidence, timings and sign-in state. A project
    // can bring its own as a link (a cloned repository): don't write through
    // one.
    if !matches!(first.as_deref(), None | Some("-h" | "--help" | "help" | "skill" | "--version" | "-V" | "version")) {
        if let Err(e) = own_state_dir() {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    }
    // The test loop over MCP: navigera's browser tools and bitti's own.
    // Flags after `mcp` are navigera's (`--headed`, `--chromium <path>`).
    if first.as_deref() == Some("mcp") {
        let args = std::iter::once("navigera".to_string()).chain(std::env::args().skip(2)).chain(["mcp".to_string()]);
        let config = match navigera::protocol::parse_args(args) {
            Ok(parsed) => parsed.config,
            Err(e) => {
                eprintln!(
                    "bitti mcp: {}",
                    // navigera's text names navigera's help; these flags are ours.
                    match e {
                        navigera::protocol::ArgsError::Help(t) | navigera::protocol::ArgsError::Invalid(t) =>
                            t.replace("navigera --help", "bitti mcp --help"),
                    }
                );
                return ExitCode::from(2);
            }
        };
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        return mcp::serve(&config, &mut stdin.lock(), &mut stdout.lock());
    }
    // `merge` reads the jsonl results of shards.
    if first.as_deref() == Some("merge") {
        let words: Vec<String> = std::env::args().skip(2).collect();
        return match merge::main(&words) {
            Ok((text, code)) => {
                print!("{text}");
                ExitCode::from(code)
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        };
    }
    // `fix` and `from-session` take their own words.
    type Own = fn(&[String]) -> Result<String, String>;
    let own: Option<Own> = match std::env::args().nth(1).as_deref() {
        Some("fix") => Some(fix::main),
        Some("from-session") => Some(from_session::main),
        Some("record") => Some(record::main),
        _ => None,
    };
    if let Some(main) = own {
        let words: Vec<String> = std::env::args().skip(2).collect();
        return match main(&words) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        };
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) if msg == USAGE => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    match args.cmd.as_str() {
        "skill" => {
            print!("{SKILL_MD}");
            ExitCode::SUCCESS
        }
        "--version" | "-V" | "version" => {
            println!("bitti {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "trace" => {
            let dir = args.paths.first().cloned().unwrap_or_else(|| PathBuf::from(".bitti/runs"));
            let only = match args.paths.get(1) {
                None => None,
                Some(p) => match p.to_str().and_then(|w| w.parse::<usize>().ok()) {
                    Some(n) => Some(n),
                    None => {
                        eprintln!("{}: not a line number (bitti trace <dir> <line>)", p.display());
                        return ExitCode::from(2);
                    }
                },
            };
            match trace::show(&dir, only) {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(2)
                }
            }
        }
        "lint" => match load(&args.paths) {
            Ok(suites) => {
                // Variables nothing sets: warnings, since the environment of
                // the run (a CI secret) may still set them.
                let mut run_vars: Vec<String> = args.opts.vars.keys().cloned().collect();
                run_vars.extend(args.opts.secrets.keys().map(|k| format!("secret:{k}")));
                let mut warnings = 0;
                for s in &suites {
                    for sc in &s.scenarios {
                        for (line, name) in suite::unset_vars(s, sc, &run_vars) {
                            warnings += 1;
                            println!(
                                "warning: {}:{line}: ${{{name}}} isn't set by the file, --var, a data row or a save before it, nor in this environment",
                                s.file.display()
                            );
                        }
                    }
                }
                let n: usize = suites.iter().map(|s| s.scenarios.len()).sum();
                let w = if warnings > 0 { format!(", {}", counted(warnings, "warning")) } else { String::new() };
                println!("ok: {}, {}{w}", counted(suites.len(), "file"), counted(n, "scenario"));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        "list" => match load(&args.paths).and_then(|suites| planned(&suites, &args).map(|jobs| (suites, jobs))) {
            Ok((suites, jobs)) => {
                for job in jobs {
                    let (suite, sc) = job.get(&suites);
                    let tags: String = sc.tags.iter().map(|t| format!(" @{t}")).collect();
                    println!("{}:{}  {}{tags}", suite.file.display(), sc.line, sc.name);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        "run" if args.watch => run_watching(args),
        "run" => run(args),
        other => {
            eprintln!(
                "unknown command {other:?}: the commands are run, lint, list, trace, merge, fix, record, \
                 from-session, navigera, mcp and skill (bitti --help)"
            );
            ExitCode::from(2)
        }
    }
}

/// An error if `.bitti`, or a directory in it bitti writes to, is a
/// link: it would send evidence, timings and sign-in state wherever the link
/// points.
fn own_state_dir() -> Result<(), String> {
    for path in [".bitti", ".bitti/runs", ".bitti/auth"] {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            let to = std::fs::read_link(path).map(|t| format!(" (to {})", t.display())).unwrap_or_default();
            return Err(format!(
                "{path} is a link{to}. bitti keeps run evidence, timings and sign-in state there and \
                 doesn't write through a link a project brought with it: remove the link (a real \
                 directory is fine)"
            ));
        }
    }
    Ok(())
}

/// `BITTI_PHASES=1`: each phase of the run's wall time on stderr, in ms
/// since the first call (a measurement aid; nothing else changes).
pub fn phase(label: &str) {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let t0 = T0.get_or_init(Instant::now);
    if std::env::var_os("BITTI_PHASES").is_some() {
        eprintln!("phase {:>8.1} ms  {label}", t0.elapsed().as_secs_f64() * 1000.0);
    }
}

/// The browser, launching on its own thread while the app server starts.
/// Dropped without being taken (a failed server, no scenario), it is joined
/// and closed, so no browser outlives the run.
struct EarlyBrowser(Option<std::thread::JoinHandle<anyhow::Result<runner::Runner>>>);

impl EarlyBrowser {
    fn take(&mut self) -> Option<runner::Runner> {
        self.0.take().and_then(|h| h.join().ok()).and_then(Result::ok)
    }
}

impl Drop for EarlyBrowser {
    fn drop(&mut self) {
        let _ = self.take();
    }
}

/// `run --watch`: the run, then each change to a scenario file runs that
/// file again, until Ctrl-C. A run that cannot start (a file that does not
/// parse) is reported and the watching goes on.
fn run_watching(args: Args) -> ExitCode {
    let mut seen = watch::stamps(&args.paths);
    // Watching goes on whatever a run ends with.
    let _ = run(args.clone());
    loop {
        eprintln!("watching {} (Ctrl-C to stop)", counted(seen.len(), "file"));
        let files = watch::wait(&args.paths, &mut seen);
        let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        eprintln!("changed: {}", names.join(", "));
        let mut next = args.clone();
        next.paths = files;
        // Each run keeps its evidence apart, as separate runs do.
        if next.opts.evidence.starts_with(".bitti/runs") {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            next.opts.evidence = PathBuf::from(format!(".bitti/runs/{stamp}"));
        }
        let _ = run(next);
    }
}

fn run(args: Args) -> ExitCode {
    phase("run() entered");
    let started = Instant::now();
    // This run's own evidence directory, under `.bitti/runs`, must be
    // new: one already there as a link was put there for this moment.
    if args.opts.evidence.starts_with(".bitti")
        && std::fs::symlink_metadata(&args.opts.evidence).is_ok_and(|m| m.file_type().is_symlink())
    {
        eprintln!(
            "{} is a link: bitti doesn't write evidence through a link a project brought with it; remove it",
            args.opts.evidence.display()
        );
        return ExitCode::from(2);
    }
    let suites = match load(&args.paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut jobs = match planned(&suites, &args) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if jobs.is_empty() && args.last_failed {
        eprintln!("no scenario failed in the last run (of these files): nothing to run again");
        return ExitCode::SUCCESS;
    }
    // `@skip` and `@fixme` are reported, not run.
    let skipped: Vec<runner::ScenarioResult> = jobs
        .iter()
        .filter_map(|j| {
            let (s, sc) = j.get(&suites);
            let why = sc.tags.iter().find(|t| matches!(t.as_str(), "skip" | "fixme"))?;
            Some(runner::ScenarioResult::skip(s, sc, &format!("@{why}")))
        })
        .collect();
    jobs.retain(|j| {
        let (_, sc) = j.get(&suites);
        !sc.tags.iter().any(|t| matches!(t.as_str(), "skip" | "fixme"))
    });
    // A run of `http` and `exec` scenarios alone starts no browser.
    let browser = jobs.iter().any(|j| {
        let (s, sc) = j.get(&suites);
        s.job_needs_browser(sc)
    });
    let mut args = args;
    // A `pause` step stops the run for a look: one worker, and a browser a
    // second navigera session can attach to.
    let pausing = jobs.iter().any(|j| {
        let (s, sc) = j.get(&suites);
        s.has_pause(sc)
    });
    args.opts.pause_step = pausing;
    // The browser starts while the app does (a `[server]` can take seconds to
    // answer; the browser needs about one): the same browser, launched sooner.
    // Without a `[server]` there is nothing to overlap, and the worker
    // launches it itself.
    let mut early = EarlyBrowser((browser && args.server.is_some() && args.opts.engine != "lightpanda").then(|| {
        let opts = args.opts.clone();
        std::thread::spawn(move || {
            phase("early browser: launching");
            let r = runner::Runner::launch(opts);
            phase("early browser: up");
            r
        })
    }));
    // The app under test, up before the first scenario and down after the run.
    let _server = match args.server.as_ref().map(server::start) {
        Some(Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(3);
        }
        Some(Ok(running)) => Some(running),
        None => None,
    };
    phase("server up");
    let mut opts = args.opts.clone();
    opts.deadline = opts.global_ms.map(|ms| started + Duration::from_millis(ms));
    // The first file's `base` serves when the run names none.
    if opts.base.is_none() {
        opts.base = suites.iter().find_map(|s| s.base.clone());
    }
    if jobs.is_empty() && skipped.is_empty() {
        eprintln!("no scenario matches");
        return ExitCode::from(2);
    }
    // Longest first (from the last runs), so the slowest one doesn't start
    // last; ones never timed go first, as they may be long.
    let timings = Timings::load();
    if args.repeat > 1 {
        jobs = jobs.iter().flat_map(|j| (1..=args.repeat).map(move |rep| runner::Job { rep, ..*j })).collect();
    }
    match args.shuffle {
        Some(seed) => {
            shuffle(&mut jobs, seed);
            eprintln!("shuffled with seed {seed} (--shuffle {seed} runs this order again)");
        }
        None => jobs.sort_by(|a, b| {
            let t = |j: &runner::Job| timings.get(&j.key(&suites)).copied().unwrap_or(f64::INFINITY);
            t(b).partial_cmp(&t(a)).unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    let jsonl = args.reporters.iter().any(|r| r == "jsonl");
    let console = args.reporters.iter().any(|r| r == "console");
    let github = args.reporters.iter().any(|r| r == "github");
    // A pause holds the run at one failure: one worker. An Electron app
    // runs one copy at a time (two would share its user data).
    if opts.electron.is_some() && args.workers_given && args.workers > 1 {
        eprintln!("bitti: --electron runs one app at a time: --workers {} becomes 1", args.workers);
    }
    if opts.attach.is_some() && args.workers_given && args.workers > 1 {
        eprintln!("bitti: --attach runs in the one browser it attached to: --workers {} becomes 1", args.workers);
    }
    if pausing && args.workers_given && args.workers > 1 {
        eprintln!("bitti: a `pause` step waits for Enter: --workers {} becomes 1", args.workers);
    }
    let one = opts.pause || pausing || opts.electron.is_some() || opts.attach.is_some();
    let workers = if one { 1 } else { args.workers.min(jobs.len()).max(1) };
    let hooks = Hooks::new(&suites, &jobs);
    // One browser, a context per worker (`--isolation context`, the
    // default), or a browser per worker. The first browser is kept here
    // and closes last.
    let mut owner = None;
    let mut forks: Vec<Option<runner::Runner>> = (0..workers).map(|_| None).collect();
    let mut early_runner = early.take().map(|mut r| {
        r.set_opts(opts.clone());
        r
    });
    // (One worker needs no sharing: its own browser is the one browser.)
    if browser && args.isolation == "context" && workers > 1 && opts.engine != "lightpanda" {
        match early_runner.take().map(Ok).unwrap_or_else(|| runner::Runner::launch(opts.clone())) {
            Ok(r) => {
                for slot in forks.iter_mut() {
                    *slot = r.fork().ok();
                }
                owner = Some(r);
            }
            Err(e) => {
                // Said as the one-worker path says it: none of them ran.
                eprintln!("{}", runner::browser_failed(&format!("{e:#}")));
                eprintln!("{} not run", counted(jobs.len(), "scenario"));
                return ExitCode::from(3);
            }
        }
    }
    // One worker, or a browser each: the first worker starts on the early browser.
    if forks.first().is_some_and(Option::is_none) {
        forks[0] = early_runner.take();
    }
    phase("jobs planned");
    // `@serial` ones last: each waits until no other scenario runs, and a
    // worker waiting for that while parallel work is left would sit idle.
    let (mut jobs, serial): (Vec<_>, Vec<_>) =
        jobs.into_iter().partition(|j: &runner::Job| !runner::is_serial(&j.get(&suites).1.tags));
    jobs.extend(serial);
    let queue = Mutex::new(std::collections::VecDeque::from(jobs));
    // `@serial` scenarios take it to write, every other one to read: one
    // of them runs while no other scenario does.
    let alone = std::sync::RwLock::new(());
    let (tx, rx) = mpsc::channel::<Event>();
    let mut results: Vec<runner::ScenarioResult> = Vec::new();
    let mut launch_errors = Vec::new();
    let failed = std::sync::atomic::AtomicUsize::new(0);
    for r in skipped {
        let _ = tx.send(Event::Done(Box::new(r)));
    }
    std::thread::scope(|scope| {
        for (w, fork) in forks.into_iter().enumerate() {
            let tx = tx.clone();
            let (queue, suites, opts, hooks, alone) = (&queue, &suites, &opts, &hooks, &alone);
            let stop = Stop { max: args.max_failures, failed: &failed, until: opts.deadline };
            let retries = args.retries;
            scope.spawn(move || worker(w, fork, queue, suites, opts, hooks, alone, retries, stop, tx));
        }
        drop(tx);
        let mut out = std::io::stdout();
        for ev in rx {
            match ev {
                Event::Done(r) => {
                    let mut text = String::new();
                    if github {
                        text.push_str(&report::github(&r));
                    }
                    if jsonl {
                        text.push_str(&report::jsonl(&r));
                    }
                    if console {
                        // With jsonl on stdout, the console goes to stderr.
                        if jsonl {
                            eprint!("{}", report::console_line(&r));
                        } else {
                            text.push_str(&report::console_line(&r));
                        }
                    }
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                    results.push(*r);
                }
                Event::LaunchFailed(e) => launch_errors.push(e),
            }
        }
    });
    phase("all scenarios done");
    drop(owner);
    let left = queue.lock().map(|q| q.len()).unwrap_or(0);
    let stopped = args.max_failures.is_some_and(|m| failed.load(std::sync::atomic::Ordering::Relaxed) >= m);
    let late = opts.deadline.is_some_and(|u| Instant::now() >= u);
    if left > 0 && late && !stopped && launch_errors.is_empty() {
        let ms = opts.global_ms.unwrap_or(0);
        eprintln!(
            "{} not run: the run's {} s ran out (--global-timeout-ms {ms})",
            counted(left, "scenario"),
            runner::secs(Duration::from_millis(ms))
        );
    } else if left > 0 && stopped && launch_errors.is_empty() {
        // The count, not the limit: scenarios already running when the
        // limit was reached finish, and may fail too.
        let m = args.max_failures.unwrap_or(1);
        let n = failed.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("{} not run: {} failed (--max-failures {m})", counted(left, "scenario"), counted(n, "scenario"));
    } else if left > 0 {
        eprintln!("{}", runner::browser_failed(launch_errors.first().map(String::as_str).unwrap_or("?")));
        eprintln!("{} not run", counted(left, "scenario"));
        if results.is_empty() {
            return ExitCode::from(3);
        }
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if console && args.repeat > 1 {
        // How often each scenario passed across its runs.
        let mut text = String::from("repeats:\n");
        for (name, ok, n) in &report::tally(&results) {
            let mark = if ok == n { "    " } else { "FLAKY " };
            text.push_str(&format!("  {mark}{ok}/{n}  {name}\n"));
        }
        if jsonl {
            eprint!("{text}");
        } else {
            print!("{text}");
        }
    }
    if console {
        let mut line = report::summary(&results, ms);
        if !browser && !results.is_empty() {
            line.push_str(" (no browser)");
        }
        if jsonl {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
    if jsonl {
        print!("{}", report::jsonl_summary(&results, ms));
    }
    if args.json {
        let saved: serde_json::Map<String, serde_json::Value> =
            results.iter().map(|r| (r.name.clone(), serde_json::json!(r.saved))).collect();
        println!("{}", serde_json::Value::Object(saved));
    }
    Timings::save(&results);
    fix::save(&results);
    if let Some(dir) = &args.allure {
        if let Err(e) = allure::write(&results, &suites, dir) {
            eprintln!("could not write Allure results to {}: {e}", dir.display());
        }
    }
    if let Some(path) = &args.cucumber {
        if let Err(e) = std::fs::write(path, gherkin::cucumber(&results, &suites)) {
            eprintln!("could not write {}: {e}", path.display());
        }
    }
    if let Some(path) = &args.junit {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(path, report::junit(&results, ms)) {
            eprintln!("could not write {}: {e}", path.display());
        }
    }
    if let Some(path) = &args.html {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        match std::fs::write(path, report::html(&results, ms, dir)) {
            Ok(()) => eprintln!("report {}", path.display()),
            Err(e) => eprintln!("could not write {}: {e}", path.display()),
        }
    }
    let failed = results.iter().any(|r| !r.passed()) || (args.fail_on_flaky && results.iter().any(|r| r.flaky()));
    // Scenarios a `--max-failures` stop left out fail the run with the failures.
    if failed || left > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// A seeded Fisher-Yates shuffle (xorshift64*): the same seed, the same
/// order, on every machine.
fn shuffle(jobs: &mut [runner::Job], seed: u64) {
    let mut x = seed.max(1);
    for i in (1..jobs.len()).rev() {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let r = x.wrapping_mul(0x2545F4914F6CDD1D);
        jobs.swap(i, (r % (i as u64 + 1)) as usize);
    }
}

/// The scenarios this run takes: `--grep`, `--grep-invert`, `--tag`,
/// `--last-failed`, `@only`, then `--shard` (every n-th, in file order, so
/// the slices are the same on every machine).
fn planned(suites: &[suite::Suite], args: &Args) -> Result<Vec<runner::Job>, String> {
    let mut jobs = runner::jobs(suites, &args.opts);
    if args.last_failed {
        let failed = fix::last_failed();
        jobs.retain(|j| {
            let (s, sc) = j.get(suites);
            let file = std::path::absolute(&s.file).unwrap_or_else(|_| s.file.clone());
            failed.iter().any(|(f, name)| *f == file && *name == sc.name)
        });
    }
    let only: Vec<&runner::Job> = jobs.iter().filter(|j| j.get(suites).1.tags.iter().any(|t| t == "only")).collect();
    if !only.is_empty() {
        if args.forbid_only {
            let list: Vec<String> = only
                .iter()
                .map(|j| {
                    let (s, sc) = j.get(suites);
                    format!("  {}:{}  {}", s.file.display(), sc.line, sc.name)
                })
                .collect();
            return Err(format!(
                "@only is on {} (a focus left in by mistake?); --forbid-only, on by itself under CI, refuses it (--allow-only to run just those):\n{}",
                counted(list.len(), "scenario"),
                list.join("\n")
            ));
        }
        jobs.retain(|j| j.get(suites).1.tags.iter().any(|t| t == "only"));
    }
    Ok(match args.shard {
        Some((i, n)) => jobs.into_iter().enumerate().filter(|(k, _)| k % n == i - 1).map(|(_, j)| j).collect(),
        None => jobs,
    })
}

/// `--max-failures`: how many may fail before no scenario starts, and how
/// many have.
#[derive(Clone, Copy)]
struct Stop<'a> {
    max: Option<usize>,
    failed: &'a std::sync::atomic::AtomicUsize,
    /// `--global-timeout-ms`: when the run is out of time.
    until: Option<Instant>,
}

impl Stop<'_> {
    fn reached(&self) -> bool {
        self.max.is_some_and(|m| self.failed.load(std::sync::atomic::Ordering::Relaxed) >= m)
            || self.until.is_some_and(|u| Instant::now() >= u)
    }
}

enum Event {
    Done(Box<runner::ScenarioResult>),
    LaunchFailed(String),
}

/// Each file's `before all` (run by the first worker that takes one of its
/// scenarios, while the others that do wait) and `after all` (run by the
/// worker that finishes its last).
struct Hooks {
    before: Mutex<Vec<Hook>>,
    ready: Condvar,
    left: Mutex<Vec<usize>>,
}

type HookResult = Result<BTreeMap<String, String>, Box<runner::Failure>>;

enum Hook {
    Idle,
    Running,
    Done(HookResult),
}

impl Hooks {
    fn new(suites: &[suite::Suite], jobs: &[runner::Job]) -> Self {
        let mut left = vec![0; suites.len()];
        for j in jobs {
            left[j.suite] += 1;
        }
        Hooks {
            before: Mutex::new(suites.iter().map(|_| Hook::Idle).collect()),
            ready: Condvar::new(),
            left: Mutex::new(left),
        }
    }

    /// File `i`'s `before all`, run once: what it saved, or how it failed.
    fn before(&self, i: usize, run: impl FnOnce() -> HookResult) -> HookResult {
        let Ok(mut st) = self.before.lock() else { return Ok(BTreeMap::new()) };
        loop {
            match &st[i] {
                Hook::Done(r) => return r.clone(),
                Hook::Running => match self.ready.wait(st) {
                    Ok(g) => st = g,
                    Err(_) => return Ok(BTreeMap::new()),
                },
                Hook::Idle => {
                    st[i] = Hook::Running;
                    drop(st);
                    let r = run();
                    if let Ok(mut st) = self.before.lock() {
                        st[i] = Hook::Done(r.clone());
                    }
                    self.ready.notify_all();
                    return r;
                }
            }
        }
    }

    /// One of file `i`'s scenarios is done: whether it was the last.
    fn finished(&self, i: usize) -> bool {
        self.left
            .lock()
            .map(|mut l| {
                l[i] = l[i].saturating_sub(1);
                l[i] == 0
            })
            .unwrap_or(false)
    }

    fn vars(&self, i: usize) -> BTreeMap<String, String> {
        match self.before.lock().as_deref().map(|b| &b[i]) {
            Ok(Hook::Done(Ok(v))) => v.clone(),
            _ => BTreeMap::new(),
        }
    }
}

/// One worker: its own browser, started when it takes its first scenario,
/// then scenarios from the shared queue until it is empty.
#[allow(clippy::too_many_arguments)]
fn worker(
    id: usize,
    fork: Option<runner::Runner>,
    queue: &Mutex<std::collections::VecDeque<runner::Job>>,
    suites: &[suite::Suite],
    opts: &runner::Options,
    hooks: &Hooks,
    alone: &std::sync::RwLock<()>,
    retries: u32,
    stop: Stop,
    tx: mpsc::Sender<Event>,
) {
    // A context on the shared browser, else a browser of its own, started
    // when the first scenario that needs one comes.
    let mut browser: Option<runner::Runner> = fork;
    loop {
        if stop.reached() {
            return;
        }
        let Some(job) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { return };
        let (suite, sc) = job.get(suites);
        let r = browser.get_or_insert_with(|| runner::Runner::bare(opts.clone()));
        if suite.job_needs_browser(sc) {
            if let Err(e) = r.ensure_browser() {
                // Give the scenario back to a worker whose browser runs.
                if let Ok(mut q) = queue.lock() {
                    q.push_front(job);
                }
                let _ = tx.send(Event::LaunchFailed(format!("{e:#}")));
                return;
            }
        }
        let job_rep = job.rep;
        r.rep = job_rep;
        // Held to the end of its tries (a poisoned lock only means a worker
        // panicked: the scenarios still run).
        let serial = runner::is_serial(&sc.tags);
        let (_shared, _alone) = if serial {
            (None, Some(alone.write().unwrap_or_else(|p| p.into_inner())))
        } else {
            (Some(alone.read().unwrap_or_else(|p| p.into_inner())), None)
        };
        // A serial one may have waited long for the others: if the run was
        // stopped meanwhile, it is not run. (Any other scenario a worker
        // took runs, and stops at the deadline itself, as before.)
        if serial && stop.reached() {
            if let Ok(mut q) = queue.lock() {
                q.push_front(job);
            }
            return;
        }
        let before = if suite.before_all.is_empty() {
            Ok(BTreeMap::new())
        } else {
            hooks.before(job.suite, || r.hook(suite, &suite.before_all, "before all", &BTreeMap::new()))
        };
        let mut result = match &before {
            Ok(vars) => {
                let mut result = r.run(suite, sc, 1, vars);
                let mut earlier = Vec::new();
                while !result.passed() && result.attempt <= retries {
                    earlier.extend(result.failure.take());
                    let next = result.attempt + 1;
                    result = r.run(suite, sc, next, vars);
                }
                result.retried = earlier;
                result
            }
            // Not run: the file's `before all` failed.
            Err(f) => runner::ScenarioResult::failed(suite, sc.name.clone(), sc.line, sc.tags.clone(), (**f).clone()),
        };
        result.worker = id;
        if job_rep > 0 {
            result.rep = job_rep;
            result.name = format!("{} #{job_rep}", result.name);
        }
        if !result.passed() {
            stop.failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        drop((_shared, _alone));
        if tx.send(Event::Done(Box::new(result))).is_err() {
            return;
        }
        if hooks.finished(job.suite) && !suite.after_all.is_empty() {
            if let Err(f) = r.hook(suite, &suite.after_all, "after all", &hooks.vars(job.suite)) {
                let line = suite.after_all[0].line;
                let mut done = runner::ScenarioResult::failed(suite, "(after all)".into(), line, Vec::new(), *f);
                done.worker = id;
                if tx.send(Event::Done(Box::new(done))).is_err() {
                    return;
                }
            }
        }
    }
}

/// How long each scenario took last time (`.bitti/timings.json`), to
/// start the slow ones first.
struct Timings;

impl Timings {
    const PATH: &'static str = ".bitti/timings.json";

    fn load() -> BTreeMap<String, f64> {
        std::fs::read_to_string(Self::PATH).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(results: &[runner::ScenarioResult]) {
        let mut all = Self::load();
        for r in results.iter().filter(|r| r.skipped.is_none()) {
            all.insert(r.key(), (r.ms * 10.0).round() / 10.0);
        }
        if std::fs::create_dir_all(".bitti").is_ok() {
            let _ = std::fs::write(Self::PATH, serde_json::to_string_pretty(&all).unwrap_or_default());
        }
    }
}

/// `1 file`, `2 files`.
fn counted(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_fits_80_columns_and_every_command_has_its_own() {
        let mut all = vec![USAGE.to_string()];
        for cmd in ["run", "lint", "list", "trace", "merge", "fix", "from-session", "record", "mcp", "skill"] {
            all.push(help_for(cmd).unwrap_or_else(|| panic!("no help for {cmd}")));
        }
        for text in &all {
            for line in text.lines() {
                assert!(line.chars().count() <= 80, "{} columns: {line}", line.chars().count());
            }
        }
        assert!(help_for("run").unwrap().contains("--pause-on-failure"));
    }

    /// An agent copies the skill's examples as they are: each must lint.
    #[test]
    fn the_skills_examples_lint_as_shown() {
        // A Windows checkout has the file with CRLF line ends.
        let skill = SKILL_MD.replace("\r\n", "\n");
        let blocks: Vec<&str> = skill.split("```text\n").skip(1).map(|b| b.split("```").next().unwrap()).collect();
        // The scripts it names sit next to the .bt, as the skill says.
        let dir = std::env::temp_dir().join(format!("bitti-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("check.js"), "return true").unwrap();
        let lints = |text: &str| {
            let path = dir.join("skill.bt");
            let mut suites = vec![suite::parse(&path, text).map_err(|e| e.to_string())?];
            suite::resolve(&mut suites).map_err(|e| e.to_string())
        };
        assert!(blocks[0].starts_with("base "), "{}", blocks[0]);
        lints(blocks[0]).unwrap_or_else(|e| panic!("{e}\n{}", blocks[0]));
        assert!(blocks[1].starts_with("expect "), "{}", blocks[1]);
        let steps: String = blocks[1].lines().map(|l| format!("  {l}\n")).collect();
        let file = format!("scenario \"x\"\n  save total = get text --testid total\n{steps}");
        lints(&file).unwrap_or_else(|e| panic!("{e}\n{file}"));
        // The API test, its schema beside it; it needs no browser.
        assert!(blocks[2].starts_with("auth "), "{}", blocks[2]);
        std::fs::write(dir.join("user.schema.json"), "{}").unwrap();
        lints(blocks[2]).unwrap_or_else(|e| panic!("{e}\n{}", blocks[2]));
        let api = suite::parse(&dir.join("skill.bt"), blocks[2]).unwrap();
        assert!(!api.scenario_needs_browser(&api.scenarios[0]));
        // A `#` after a step is a word of it, not a comment.
        for line in blocks[..3].iter().flat_map(|b| b.lines()) {
            assert!(line.trim_start().starts_with('#') || !line.contains(" # "), "a comment after a step: {line}");
        }
    }
}
