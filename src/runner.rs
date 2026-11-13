//! Runs scenarios on one browser, in-process through the navigera library:
//! no session server, no socket, no process per step. Each scenario starts
//! from a clean browser context (`reset`: no cookies, storage or tabs from
//! the one before).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use navigera::protocol::{parse_args, Command, Driver, SessionConfig};
use serde_json::Value;

use crate::expect::{compare, Expect, HttpRead, Matcher, Prop, RequestRead, Shot, Subject};
use crate::suite::{interpolate, Action, Scenario, Step, Suite};

/// The state each `auth` block saved this run (or an earlier one within
/// its TTL), shared by the workers: the first scenario that needs one runs
/// it, the others wait for it.
#[derive(Debug, Default)]
pub struct AuthCache {
    slots: Mutex<HashMap<String, AuthSlot>>,
}

/// One auth's state file once it is known, or why signing in failed.
type AuthSlot = Arc<Mutex<Option<Result<PathBuf, String>>>>;

impl AuthCache {
    fn slot(&self, name: &str) -> AuthSlot {
        self.slots.lock().map(|mut m| m.entry(name.to_string()).or_default().clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub base: Option<String>,
    pub vars: BTreeMap<String, String>,
    pub headed: bool,
    pub chromium: Option<String>,
    /// navigera's engine: `chrome` or `lightpanda`.
    pub engine: String,
    /// How long an `expect` retries (ms) unless it says otherwise.
    pub expect_ms: u64,
    /// navigera's element wait for steps (`--wait-ms`).
    pub wait_ms: Option<f64>,
    pub grep: Option<String>,
    /// `--grep-invert`: names that contain it are left out.
    pub grep_invert: Option<String>,
    /// `--tag`s: tags or tag expressions; a scenario runs when any holds.
    pub tags: Vec<String>,
    /// Where failure evidence (a screenshot) goes.
    pub evidence: PathBuf,
    /// `${run.id}`: the same for every scenario of a run, different from
    /// run to run.
    pub run_id: String,
    /// `${secret:NAME}`s from `--secrets <file>` (the environment comes
    /// first).
    pub secrets: BTreeMap<String, String>,
    /// `--pause-on-failure`: keep the browser at a failure for a look.
    pub pause: bool,
    pub auth: Arc<AuthCache>,
    /// `--update-screenshots`: a screenshot that differs becomes the baseline.
    pub update_shots: bool,
    /// `--trace`: record what each step saw.
    pub trace: crate::trace::Trace,
    /// `--video`: record what the page showed (navigera `screencast`).
    pub video: crate::trace::Trace,
    /// `--compare-report`: what compara keeps when a step doesn't say.
    pub compare_report: Vec<String>,
    /// `--electron <app>`: the scenarios drive this Electron app's windows,
    /// a fresh app for each scenario.
    pub electron: Option<String>,
    /// `--electron-arg <arg>`: the app's own arguments.
    pub electron_args: Vec<String>,
    /// `--attach <port|ws://…>`: a browser or an Electron app that is
    /// already running with a DevTools port, instead of one launched here.
    pub attach: Option<String>,
    /// `--slowmo <ms>`: wait this long before each step that acts on the
    /// page, to watch a headed run.
    pub slowmo: Option<u64>,
    /// `--scenario-timeout-ms`: the most a scenario's steps may take. A
    /// step starts only with time left, and waits no longer than what is.
    pub scenario_ms: Option<u64>,
    /// `--global-timeout-ms`: the most the whole run may take, and when
    /// that is (set as the run starts). No scenario starts after it, and a
    /// running one's steps are cut there.
    pub global_ms: Option<u64>,
    pub deadline: Option<Instant>,
    /// A `pause` step is in the run: the browser listens on a DevTools
    /// port, for another navigera session to look at it.
    pub pause_step: bool,
}

impl Options {
    /// A run's defaults; `stamp` (seconds) names its evidence directory.
    pub fn new(stamp: u64) -> Self {
        Options {
            base: std::env::var("BITTI_BASE_URL").ok().filter(|v| !v.is_empty()),
            vars: BTreeMap::new(),
            headed: false,
            chromium: None,
            engine: std::env::var("NAVIGERA_ENGINE")
                .ok()
                .filter(|e| !e.trim().is_empty())
                .unwrap_or_else(|| "chrome".into()),
            expect_ms: 5000,
            wait_ms: None,
            grep: None,
            grep_invert: None,
            tags: Vec::new(),
            evidence: PathBuf::from(format!(".bitti/runs/{stamp}")),
            run_id: format!("{stamp}-{}", std::process::id()),
            secrets: BTreeMap::new(),
            pause: false,
            auth: Default::default(),
            update_shots: false,
            trace: Default::default(),
            video: Default::default(),
            compare_report: vec!["html".into()],
            electron: None,
            electron_args: Vec::new(),
            attach: None,
            slowmo: None,
            scenario_ms: None,
            global_ms: None,
            deadline: None,
            pause_step: false,
        }
    }
}

/// One step that ran, for the HTML report.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub line: usize,
    pub text: String,
    pub ms: f64,
    pub ok: bool,
    /// An `optional` step that did not hold (it didn't fail the scenario).
    pub missed: bool,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub line: usize,
    pub text: String,
    pub message: String,
    /// Where the page was.
    pub at: Option<(String, String)>,
    /// Something earlier that likely explains it (a click that changed nothing).
    pub hint: Option<String>,
    pub screenshot: Option<PathBuf>,
    /// When the element wasn't there: what on the page comes closest,
    /// (`role "name"`, a locator for it, how close).
    pub nearest: Vec<(String, String, f64)>,
    /// Where the step that failed is written when that isn't the
    /// scenario's own line (a step of a flow, maybe in a `use`d file):
    /// file, line, text. `fix` changes that line.
    pub source: Option<(PathBuf, usize, String)>,
}

#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub file: PathBuf,
    pub name: String,
    pub line: usize,
    pub tags: Vec<String>,
    pub ms: f64,
    pub steps: Vec<StepResult>,
    pub failure: Option<Failure>,
    /// 1 for the first try; more when `--retries` ran it again.
    pub attempt: u32,
    /// How the earlier tries failed (a scenario that passed after one is
    /// flaky).
    pub retried: Vec<Failure>,
    /// What it saved, for `--json`.
    pub saved: BTreeMap<String, String>,
    /// Optional steps that didn't happen.
    pub notes: Vec<String>,
    /// Which worker ran it.
    pub worker: usize,
    /// Which of `--repeat`'s runs (0 when not repeated); the name says it too.
    pub rep: u32,
    /// When it started (Unix ms), for reports with a timeline.
    pub started: f64,
    /// The directory holding this scenario's `trace.jsonl` and its per-step
    /// files, when a trace was kept.
    pub trace: Option<PathBuf>,
    /// The player of the scenario's video (`--video`), when one was kept.
    pub video: Option<PathBuf>,
    /// Not run, and why (`@skip`, `@fixme`): it neither passed nor failed.
    pub skipped: Option<String>,
}

/// Now, in Unix milliseconds.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

impl ScenarioResult {
    /// A result that ran no steps: its `before all` failed, or it is an
    /// `after all` that did.
    pub fn failed(suite: &Suite, name: String, line: usize, tags: Vec<String>, failure: Failure) -> Self {
        ScenarioResult {
            file: suite.file.clone(),
            name,
            line,
            tags,
            ms: 0.0,
            steps: Vec::new(),
            failure: Some(failure),
            attempt: 1,
            retried: Vec::new(),
            saved: BTreeMap::new(),
            notes: Vec::new(),
            worker: 0,
            rep: 0,
            started: now_ms(),
            trace: None,
            video: None,
            skipped: None,
        }
    }

    /// Passed, or skipped: nothing failed.
    pub fn passed(&self) -> bool {
        self.failure.is_none()
    }

    /// A scenario that was not run (`@skip`, `@fixme`), with why.
    pub fn skip(suite: &Suite, sc: &Scenario, why: &str) -> Self {
        let mut r = ScenarioResult::failed(
            suite,
            sc.name.clone(),
            sc.line,
            sc.tags.clone(),
            Failure {
                line: sc.line,
                text: String::new(),
                message: String::new(),
                at: None,
                hint: None,
                screenshot: None,
                nearest: Vec::new(),
                source: None,
            },
        );
        r.failure = None;
        r.skipped = Some(why.to_string());
        r
    }

    pub fn flaky(&self) -> bool {
        self.passed() && !self.retried.is_empty()
    }

    /// `file::name`, the key of `.bitti/timings.json`.
    pub fn key(&self) -> String {
        format!("{}::{}", self.file.display(), self.base_name())
    }

    /// The scenario's name without `--repeat`'s ` #k`.
    pub fn base_name(&self) -> &str {
        match (self.rep, self.name.rsplit_once(" #")) {
            (1.., Some((base, _))) => base,
            _ => &self.name,
        }
    }
}

/// A scenario to run: indexes into the run's suites.
#[derive(Debug, Clone, Copy)]
pub struct Job {
    pub suite: usize,
    pub scenario: usize,
    /// `--repeat`'s run number (0 when not repeated).
    pub rep: u32,
}

impl Job {
    pub fn get<'a>(&self, suites: &'a [Suite]) -> (&'a Suite, &'a Scenario) {
        let s = &suites[self.suite];
        (s, &s.scenarios[self.scenario])
    }

    pub fn key(&self, suites: &[Suite]) -> String {
        let (s, sc) = self.get(suites);
        format!("{}::{}", s.file.display(), sc.name)
    }
}

/// Which scenarios a run takes, after `--grep`, `--grep-invert` and
/// `--tag`, in file order.
pub fn jobs(suites: &[Suite], opts: &Options) -> Vec<Job> {
    let mut out = Vec::new();
    for (si, suite) in suites.iter().enumerate() {
        for (ci, sc) in suite.scenarios.iter().enumerate() {
            let name = sc.name.to_lowercase();
            if opts.grep.as_ref().is_some_and(|g| !name.contains(&g.to_lowercase())) {
                continue;
            }
            if opts.grep_invert.as_ref().is_some_and(|g| name.contains(&g.to_lowercase())) {
                continue;
            }
            if !opts.tags.is_empty() && !opts.tags.iter().any(|t| tag_expr(t, &sc.tags).unwrap_or(false)) {
                continue;
            }
            out.push(Job { suite: si, scenario: ci, rep: 0 });
        }
    }
    out
}

/// A `--tag`: one tag (`smoke`, `@smoke`), or an expression of tags with
/// `and`, `or`, `not` and parentheses (`smoke and not slow`), as Cucumber
/// writes them. Whether `tags` satisfy it, or what is wrong with it.
pub fn tag_expr(expr: &str, tags: &[String]) -> Result<bool, String> {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ");
    let words: Vec<&str> = spaced.split_whitespace().collect();
    if words.is_empty() {
        return Err("--tag needs a tag or a tag expression".into());
    }
    struct P<'a> {
        w: Vec<&'a str>,
        i: usize,
        tags: &'a [String],
        expr: &'a str,
    }
    impl P<'_> {
        fn bad(&self, why: &str) -> String {
            format!("--tag {:?}: {why} (tags with and, or, not and parentheses: \"smoke and not slow\")", self.expr)
        }
        fn or(&mut self) -> Result<bool, String> {
            let mut v = self.and()?;
            while self.w.get(self.i) == Some(&"or") {
                self.i += 1;
                v |= self.and()?;
            }
            Ok(v)
        }
        fn and(&mut self) -> Result<bool, String> {
            let mut v = self.not()?;
            while self.w.get(self.i) == Some(&"and") {
                self.i += 1;
                v &= self.not()?;
            }
            Ok(v)
        }
        fn not(&mut self) -> Result<bool, String> {
            match self.w.get(self.i).copied() {
                Some("not") => {
                    self.i += 1;
                    Ok(!self.not()?)
                }
                Some("(") => {
                    self.i += 1;
                    let v = self.or()?;
                    if self.w.get(self.i) != Some(&")") {
                        return Err(self.bad("a ( without its )"));
                    }
                    self.i += 1;
                    Ok(v)
                }
                Some(w) if !matches!(w, "and" | "or" | ")") => {
                    self.i += 1;
                    let t = w.trim_start_matches('@');
                    Ok(self.tags.iter().any(|x| x == t))
                }
                Some(w) => Err(self.bad(&format!("a tag expected before {w:?}"))),
                None => Err(self.bad("it ends where a tag is expected")),
            }
        }
    }
    let mut p = P { w: words, i: 0, tags, expr };
    let v = p.or()?;
    if p.i < p.w.len() {
        return Err(p.bad(&format!("{:?} after a whole expression (and or or between tags)", p.w[p.i])));
    }
    Ok(v)
}

pub struct Runner {
    /// When the scenario's `--scenario-timeout-ms` runs out: a step's own
    /// wait is cut there.
    cap: Option<Instant>,
    /// None until a scenario needs a browser: one that runs only `http`
    /// and `exec` steps never starts one.
    driver: Option<Driver>,
    /// What launched it: Lightpanda (one context per process) is launched
    /// again for each scenario.
    config: SessionConfig,
    opts: Options,
    fresh: bool,
    attempt: u32,
    /// Which of `--repeat`'s runs is under way (0 when not repeated).
    pub rep: u32,
    /// The cookies of what runs with no browser (a scenario, a hook, an
    /// `auth`): what its `http` answers set.
    jar: crate::http::Jar,
    /// What runs now needs no browser: cookies are the jar's, and a
    /// failure has no page to show.
    bare: bool,
}

impl Runner {
    fn session_config(opts: &Options) -> SessionConfig {
        SessionConfig {
            headed: opts.headed,
            chromium: opts.chromium.clone(),
            engine: opts.engine.clone(),
            wait_ms: opts.wait_ms,
            // A pause leaves the browser to another navigera session: it
            // attaches over the DevTools port.
            transport: (opts.pause || opts.pause_step).then(|| "ws".to_string()),
            electron: opts.electron.clone(),
            electron_args: opts.electron_args.clone(),
            attach: opts.attach.clone(),
            ..SessionConfig::default()
        }
    }

    /// Start the browser. An error here is infrastructure, not a test result.
    pub fn launch(opts: Options) -> anyhow::Result<Self> {
        let mut r = Runner::bare(opts);
        r.driver = Some(Driver::launch(&r.config)?);
        Ok(r)
    }

    /// A runner with no browser yet: the scenarios that need none never
    /// start one, and the first that does starts it ([`Runner::ensure_browser`]).
    pub fn bare(opts: Options) -> Self {
        let config = Self::session_config(&opts);
        Runner {
            cap: None,
            driver: None,
            config,
            opts,
            fresh: true,
            attempt: 1,
            rep: 0,
            jar: Default::default(),
            bare: false,
        }
    }

    /// The browser, started now if there is none yet. An error here is
    /// infrastructure, as [`Runner::launch`]'s is.
    pub fn ensure_browser(&mut self) -> anyhow::Result<()> {
        if self.driver.is_none() {
            self.driver = Some(Driver::launch(&self.config)?);
            // A new browser is clean already.
            self.fresh = true;
        }
        Ok(())
    }

    /// This run's settings, for a browser launched before they were final
    /// (while the app server started).
    pub fn set_opts(&mut self, opts: Options) {
        self.opts = opts;
    }

    /// The browser, as the last scenario left it (started now if there is
    /// none yet).
    pub fn driver(&mut self) -> anyhow::Result<&mut Driver> {
        self.ensure_browser()?;
        self.driver.as_mut().ok_or_else(|| anyhow::anyhow!("no browser"))
    }

    /// Another runner on this one's browser, in a browser context of its
    /// own: a worker without a browser of its own. With no browser here,
    /// one with none either.
    pub fn fork(&self) -> anyhow::Result<Self> {
        let mut r = Runner::bare(self.opts.clone());
        r.config = self.config.clone();
        if let Some(d) = &self.driver {
            r.driver = Some(d.fork()?);
        }
        Ok(r)
    }

    /// A clean browser context for every scenario but the first one of
    /// the run (a new browser is clean already): no cookies, storage, tabs,
    /// routes, init scripts or `set` emulation from the one before.
    fn clean(&mut self, line: usize) -> Result<(), Box<Failure>> {
        let clean = self.ensure_browser().and_then(|()| {
            let fresh = std::mem::replace(&mut self.fresh, false);
            let Some(driver) = self.driver.as_mut() else { return Ok(()) };
            // An Electron app attached to (`--attach`) is the developer's own:
            // it can't be started again, so the scenarios go on where the one
            // before left it.
            if fresh || (self.config.attach.is_some() && driver.session().is_electron()) {
                return Ok(());
            }
            // Lightpanda has one context per process, and an Electron app's
            // windows and storage are its own: a new one for each scenario.
            if navigera::browser::is_lightpanda(&self.config.engine) || self.config.electron.is_some() {
                Driver::launch(&self.config).map(|d| {
                    std::mem::replace(driver, d).session().close();
                })
            } else {
                driver.run_op(&Command::Reset { all: true }).map(|_| ())
            }
        });
        clean.map_err(|e| {
            Box::new(Failure {
                line,
                text: "(reset)".into(),
                message: format!("could not start from a clean browser context: {e:#}"),
                at: None,
                hint: None,
                screenshot: None,
                nearest: Vec::new(),
                source: None,
            })
        })
    }

    /// `file_vars`: what the file's `before all` saved.
    pub fn run(
        &mut self,
        suite: &Suite,
        scenario: &Scenario,
        attempt: u32,
        file_vars: &BTreeMap<String, String>,
    ) -> ScenarioResult {
        self.attempt = attempt;
        // `@slow`: three times the time for each check, as Playwright's
        // `test.slow()` gives.
        let slow = scenario.tags.iter().any(|t| t == "slow");
        let (expect_ms, scenario_ms) = (self.opts.expect_ms, self.opts.scenario_ms);
        if slow {
            self.opts.expect_ms *= 3;
            self.opts.scenario_ms = scenario_ms.map(|ms| ms * 3);
        }
        let result = self.run_inner(suite, scenario, attempt, file_vars);
        self.opts.expect_ms = expect_ms;
        self.opts.scenario_ms = scenario_ms;
        result
    }

    fn run_inner(
        &mut self,
        suite: &Suite,
        scenario: &Scenario,
        attempt: u32,
        file_vars: &BTreeMap<String, String>,
    ) -> ScenarioResult {
        let started = Instant::now();
        let started_ms = now_ms();
        let mut scope = Scope::new(suite, &scenario.name, scenario.vars.clone(), &self.opts, file_vars);
        let mut steps = Vec::new();
        // Only `http`, `exec` and checks of what they answered: no browser,
        // the cookies in a jar of the scenario's own.
        let bare = !suite.scenario_needs_browser(scenario);
        self.bare = bare;
        self.jar = Default::default();
        let mut failure = if bare { None } else { self.clean(scenario.line).err().map(|f| *f) };
        if !bare {
            // `expect screenshot`'s page-quiet clock, before the first goto:
            // the per-scenario clean context dropped the one before.
            let _ = self.command(&["init-script".into(), QUIET_CLOCK.into()]);
        }
        let downloads = if failure.is_none() && !bare { self.own_downloads(suite, &scenario.name) } else { None };
        let video = if failure.is_none() && !bare && self.opts.video != crate::trace::Trace::Off {
            self.start_video(suite, &scenario.name)
        } else {
            None
        };
        let all: Vec<&Step> = suite.before_each.iter().chain(&scenario.steps).collect();
        let tracing = self.opts.trace != crate::trace::Trace::Off && failure.is_none();
        let mut trace: Vec<Value> = Vec::new();
        // Each traced step's files (name, bytes), written when the trace is kept.
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let mut ax_at_failure = None;
        let mut seen = (0usize, 0u64);
        // The last captured step's page fingerprint, its line and its file names.
        let mut last: Option<(String, usize, Option<Value>, Option<Value>)> = None;
        if tracing && !bare {
            seen = (self.console_count(), self.last_request());
        }
        if failure.is_none() {
            // `--scenario-timeout-ms` and `--global-timeout-ms`: no step
            // starts after the time is up, and none waits past it.
            let own = self.opts.scenario_ms.map(|ms| started + Duration::from_millis(ms));
            let until = match (own, self.opts.deadline) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let (scenario_ms, global_ms) = (self.opts.scenario_ms, self.opts.global_ms.unwrap_or(0));
            let limit = move |at: Instant| match (own, scenario_ms) {
                (Some(o), Some(ms)) if o == at => {
                    format!("the scenario's {} s (--scenario-timeout-ms {ms})", secs(Duration::from_millis(ms)))
                }
                _ => {
                    format!("the run's {} s (--global-timeout-ms {global_ms})", secs(Duration::from_millis(global_ms)))
                }
            };
            for step in all {
                let t = Instant::now();
                scope.optional_missed = false;
                if let Some(u) = until.filter(|u| Instant::now() >= *u) {
                    let message = format!("out of time before this step: {} ran out", limit(u));
                    failure = Some(self.failure(step, message, &scope, suite, &scenario.name));
                    break;
                }
                self.cap = until;
                let res = self.step(step, &mut scope, suite);
                self.cap = None;
                let res = match (res, until) {
                    (Err(m), Some(u)) if Instant::now() >= u => {
                        Err(format!("{m}\n({} ran out in this step)", limit(u)))
                    }
                    (res, _) => res,
                };
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let missed = res.is_ok() && scope.optional_missed;
                steps.push(StepResult { line: step.line, text: step.text.clone(), ms, ok: res.is_ok(), missed });
                if tracing {
                    let error = res.as_ref().err().map(String::as_str);
                    let mut tr = if bare {
                        // The requests the step made, in the network column.
                        let made = scope.exchanges[(seen.1 as usize).min(scope.exchanges.len())..].to_vec();
                        seen.1 = scope.exchanges.len() as u64;
                        crate::trace::step(step.line, &step.text, ms, error, Default::default(), Vec::new(), made)
                    } else {
                        let mut tr = self.trace_step(step, ms, error, &mut seen);
                        self.step_files(trace.len(), step.line, res.is_err(), &mut last, &mut tr, &mut files);
                        tr
                    };
                    if missed {
                        tr["optional_missed"] = Value::Bool(true);
                    }
                    trace.push(tr);
                    if res.is_err() && !bare {
                        ax_at_failure = match self.command(&["ax".into(), "--limit".into(), "0".into()]) {
                            Ok(Value::String(t)) => Some(t),
                            _ => None,
                        };
                    }
                }
                if let Err(message) = res {
                    failure = Some(self.failure(step, message, &scope, suite, &scenario.name));
                    break;
                }
            }
        }
        // `--soft` expectations that didn't hold fail the scenario now.
        if failure.is_none() && !scope.soft.is_empty() {
            let (first, _) = scope.soft[0].clone();
            let all: Vec<String> = scope.soft.iter().map(|(st, m)| format!("line {}: {m}", st.line)).collect();
            let n = all.len();
            let message = format!("{n} soft expectation(s) didn't hold:\n{}", all.join("\n"));
            failure = Some(self.failure(&first, message, &scope, suite, &scenario.name));
        }
        if let (true, Some(f), false) = (self.opts.pause, &failure, bare) {
            self.pause(f.line, suite, "the failure left it");
        }
        // `after each` runs whatever happened; its own failures count only
        // when the scenario had passed.
        for step in &suite.after_each {
            if let Err(message) = self.step(step, &mut scope, suite) {
                if failure.is_none() {
                    failure = Some(self.failure(step, message, &scope, suite, &scenario.name));
                }
            }
        }
        let mut result = ScenarioResult {
            file: suite.file.clone(),
            name: scenario.name.clone(),
            line: scenario.line,
            tags: scenario.tags.clone(),
            ms: started.elapsed().as_secs_f64() * 1000.0,
            steps,
            failure,
            attempt,
            retried: Vec::new(),
            saved: BTreeMap::new(),
            notes: Vec::new(),
            worker: 0,
            rep: 0,
            started: started_ms,
            trace: None,
            video: None,
            skipped: None,
        };
        if let Some(dir) = video {
            let keep = self.opts.video == crate::trace::Trace::All || result.failure.is_some();
            result.video = self.stop_video(dir, keep);
        }
        let mask = scope.masker();
        if tracing && (self.opts.trace == crate::trace::Trace::All || result.failure.is_some()) {
            let dir = self.evidence_dir(suite, &scenario.name);
            let head = serde_json::json!({
                "scenario": result.name, "file": suite.file.display().to_string(), "line": result.line,
                "passed": result.failure.is_none(), "ms": result.ms,
            });
            let text: String = std::iter::once(head).chain(trace).map(|v| mask(&v.to_string()) + "\n").collect();
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(dir.join("trace.jsonl"), text);
                if let Some(ax) = ax_at_failure {
                    let _ = std::fs::write(dir.join("ax.txt"), mask(&ax));
                }
                // The ax tree is text and is masked like the rest; a
                // screenshot is pixels (as with the failure's own).
                for (name, bytes) in files {
                    let bytes = if name.ends_with(".txt") {
                        mask(&String::from_utf8_lossy(&bytes)).into_bytes()
                    } else {
                        bytes
                    };
                    let _ = std::fs::write(dir.join(name), bytes);
                }
                result.trace = Some(dir);
            }
        }
        result.saved = scope.saved.into_iter().map(|(k, v)| (k, mask(&v))).collect();
        result.notes = scope.notes.iter().map(|n| mask(n)).collect();
        for step in result.steps.iter_mut() {
            step.text = mask(&step.text);
        }
        if let Some(f) = result.failure.as_mut() {
            mask_failure(f, &mask);
        }
        tidy(downloads);
        result
    }

    /// The scenario's (or hook's) evidence: its screenshot, its downloads.
    fn evidence_dir(&self, suite: &Suite, label: &str) -> PathBuf {
        let mut dir = match self.rep {
            0 => evidence_name(&suite.file, label),
            k => evidence_name(&suite.file, &format!("{label} #{k}")),
        };
        if self.attempt > 1 {
            dir.push_str(&format!("-try{}", self.attempt));
        }
        self.opts.evidence.join(dir)
    }

    /// `--video`: record the page into the scenario's evidence from now on.
    /// A browser that can't (Lightpanda) records nothing.
    fn start_video(&mut self, suite: &Suite, label: &str) -> Option<PathBuf> {
        let dir = self.evidence_dir(suite, label).join("video");
        let cmd = parse(&["screencast".into(), "start".into(), dir.display().to_string()]).ok()?;
        let driver = self.driver.as_mut()?;
        if let Err(e) = driver.run(&cmd) {
            // One a scenario before left running (it stopped half way).
            if !format!("{e:#}").contains("already recording") {
                return None;
            }
            let stop = parse(&["screencast".into(), "stop".into()]).ok()?;
            let _ = driver.run(&stop);
            driver.run(&cmd).ok()?;
        }
        Some(dir)
    }

    /// End the recording: its player when it is kept, else nothing left of it.
    fn stop_video(&mut self, dir: PathBuf, keep: bool) -> Option<PathBuf> {
        let stopped =
            parse(&["screencast".into(), "stop".into()]).ok().and_then(|cmd| self.driver.as_mut()?.run(&cmd).ok());
        if keep && stopped.is_some() {
            return Some(dir.join("index.html"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        if let Some(parent) = dir.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        None
    }

    /// Downloads go to a directory of the scenario's own, beside its
    /// evidence, so a file name never carries over from an earlier
    /// scenario (`report (2).csv`); `tidy` removes it when nothing came.
    /// One a run before left files in (same `--evidence`) is kept, and the
    /// next free name taken.
    fn own_downloads(&mut self, suite: &Suite, label: &str) -> Option<PathBuf> {
        let base = self.evidence_dir(suite, label);
        let dir = (1..1000)
            .map(|i| if i == 1 { base.join("downloads") } else { base.join(format!("downloads-{i}")) })
            .find(|d| std::fs::read_dir(d).map_or(true, |mut e| e.next().is_none()))?;
        let cmd = parse(&["downloads".into(), "--dir".into(), dir.display().to_string()]).ok()?;
        self.driver.as_mut()?.run(&cmd).ok().map(|_| dir)
    }

    /// A `before all` or `after all` block: in a clean context of its own;
    /// what it saved, or how it failed.
    pub fn hook(
        &mut self,
        suite: &Suite,
        steps: &[Step],
        label: &str,
        file_vars: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, Box<Failure>> {
        let line = steps.first().map(|s| s.line).unwrap_or(0);
        let bare = !crate::suite::needs_browser(suite, steps);
        self.bare = bare;
        self.jar = Default::default();
        if !bare {
            self.clean(line)?;
        }
        let downloads = if bare { None } else { self.own_downloads(suite, label) };
        let mut scope = Scope::new(suite, label, BTreeMap::new(), &self.opts, file_vars);
        for step in steps {
            if let Err(message) = self.step(step, &mut scope, suite) {
                let mut f = self.failure(step, format!("{label}: {message}"), &scope, suite, label);
                mask_failure(&mut f, &scope.masker());
                tidy(downloads);
                return Err(Box::new(f));
            }
        }
        tidy(downloads);
        Ok(scope.saved)
    }

    /// The browser stays as it is (`how`: as the failure left it), open to
    /// another navigera session, until Enter.
    fn pause(&mut self, line: usize, suite: &Suite, how: &str) {
        match self.driver.as_ref().filter(|_| !self.bare) {
            Some(driver) => {
                // The tab itself, so the look is at the page as it is.
                let ws = driver.session().page_endpoint().unwrap_or_else(|| driver.session().endpoint());
                eprintln!(
                    "PAUSED  {}:{line}  the browser stays as {how}; look at it with\n        navigera -s bt start --attach {ws}\n        navigera -s bt ax\n        Enter goes on with the run (Ctrl-C stops it)",
                    suite.file.display(),
                );
            }
            None => eprintln!(
                "PAUSED  {}:{line}  (this scenario runs without a browser)\n        Enter goes on with the run (Ctrl-C stops it)",
                suite.file.display(),
            ),
        }
        if enter(None).is_err() {
            // No one can press Enter: an agent looking from elsewhere gets
            // a while (BITTI_PAUSE_S, default 600 s); CI none.
            let ci = under_ci();
            let secs: u64 =
                std::env::var("BITTI_PAUSE_S").ok().and_then(|v| v.parse().ok()).unwrap_or(if ci { 0 } else { 600 });
            if secs == 0 {
                eprintln!("        (no terminal input, and CI: not pausing; BITTI_PAUSE_S=<s> to pause anyway)");
            } else {
                eprintln!("        (no terminal input here: going on in {secs} s; BITTI_PAUSE_S sets how long)");
                std::thread::sleep(Duration::from_secs(secs));
            }
        }
    }

    fn failure(&mut self, step: &Step, message: String, scope: &Scope, suite: &Suite, label: &str) -> Failure {
        // An API step's failure: the answer it was about says why.
        let message = match scope.exchanges.last() {
            // (A request that failed has said its status and body already.)
            Some(last) if (self.bare || reads_answer(&step.action)) && !message.contains(" answered ") => {
                let body = scope.saved.get("http.body").map(String::as_str).unwrap_or("").trim();
                let body = if body.is_empty() { String::new() } else { format!("\n  {}", clip(body, 500)) };
                format!("{message}\nlast http: {last}{body}")
            }
            _ => message,
        };
        if self.bare || self.driver.is_none() {
            return Failure {
                line: step.line,
                text: step.text.clone(),
                message: bt_terms(&message),
                at: None,
                hint: scope.noop.clone(),
                screenshot: None,
                nearest: Vec::new(),
                source: scope.inner.clone().map(|(file, s)| (file, s.line, s.text)),
            };
        }
        let at = self.driver.as_mut().and_then(|d| d.run(&Command::Url {}).ok()).map(|v| {
            (
                v.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
                v.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            )
        });
        let shot = self.evidence_dir(suite, label).join("screenshot.png");
        let screenshot = std::fs::create_dir_all(shot.parent().unwrap_or(Path::new(".")))
            .ok()
            .and_then(|_| {
                let cmd = parse(&["screenshot".into(), "--path".into(), shot.display().to_string()]).ok()?;
                self.driver.as_mut()?.run(&cmd).ok()
            })
            .map(|_| shot);
        // A step of a flow failed: what comes closest is to its element.
        let inner = scope.inner.clone();
        let action = inner.as_ref().map_or(&step.action, |(_, s)| &s.action);
        let nearest = if message.contains("no element") {
            self.nearest(action, scope)
        } else if message.contains("is covered by") {
            self.uncover(&message)
        } else {
            Vec::new()
        };
        let source = inner.map(|(file, s)| (file, s.line, s.text));
        Failure {
            line: step.line,
            text: step.text.clone(),
            message: bt_terms(&message),
            at,
            hint: scope.noop.clone(),
            screenshot,
            nearest,
            source,
        }
    }

    /// What on the page comes closest to the element a step asked for.
    fn nearest(&mut self, action: &Action, scope: &Scope) -> Vec<(String, String, f64)> {
        let words = match action {
            Action::Command(w) | Action::Save { command: w, .. } => w.clone(),
            Action::Expect(Expect { subject: Subject::Target { flags, .. }, .. }) => flags.clone(),
            Action::Optional(inner) => return self.nearest(inner, scope),
            _ => return Vec::new(),
        };
        let words = scope.words(&words).unwrap_or(words);
        let Some(want) = crate::nearest::wanted(&words) else { return Vec::new() };
        let mut nodes = match self.command(&["ax".into(), "--format".into(), "json".into()]) {
            Ok(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        // The text tree has what frames hold too.
        if let Ok(Value::String(tree)) = self.command(&["ax".into(), "--limit".into(), "0".into()]) {
            nodes.extend(crate::nearest::from_tree(&tree));
        }
        crate::nearest::rank(&Value::Array(nodes), &want, 3)
    }

    /// A click under something else (a consent bar, a modal): what in it a
    /// user would click to get it out of the way, as an `optional click`
    /// for `fix` to put before the step (`fix::BEFORE`).
    fn uncover(&mut self, message: &str) -> Vec<(String, String, f64)> {
        let Some((_, rest)) = message.split_once("covered by ") else { return Vec::new() };
        let cover = rest.split_once(" at its centre").map_or(rest, |(c, _)| c);
        let js = format!("({UNCOVER_JS})({})", serde_json::json!(cover.split(' ').next().unwrap_or("")));
        let Ok(Value::Array(found)) = self.command(&["eval".into(), js]) else { return Vec::new() };
        found
            .iter()
            .filter_map(|f| {
                let (role, name) = (f["role"].as_str()?, f["name"].as_str()?);
                let step = format!("optional click --role {role} --name {}", crate::fix::quote(name));
                let element = format!("{role} {} (on {cover})", serde_json::json!(name));
                Some((element, format!("{}{step}", crate::fix::BEFORE), f["p"].as_f64().unwrap_or(0.5)))
            })
            .collect()
    }

    /// `d`, or less: no more than the scenario has left.
    fn capped(&self, d: Duration) -> Duration {
        match self.cap {
            Some(c) => d.min(c.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))),
            None => d,
        }
    }

    fn step(&mut self, step: &Step, scope: &mut Scope, suite: &Suite) -> Result<(), String> {
        scope.inner = None;
        self.action(&step.action, step, scope, suite, 0)
    }

    fn action(
        &mut self,
        action: &Action,
        step: &Step,
        scope: &mut Scope,
        suite: &Suite,
        depth: usize,
    ) -> Result<(), String> {
        match action {
            Action::Command(words) => {
                let words = scope.words(words)?;
                if let [op, flag, ms] = words.as_slice() {
                    if op == "wait" && flag == "--ms" && self.bare {
                        let ms: u64 = ms.parse().map_err(|_| format!("wait --ms takes milliseconds, not {ms:?}"))?;
                        std::thread::sleep(Duration::from_millis(ms));
                        return Ok(());
                    }
                }
                // `--slowmo`: a moment to watch each action happen.
                if let Some(ms) = self.opts.slowmo.filter(|_| !self.bare) {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                let outcome = self.command(&words)?;
                // A click that changed nothing explains a failure soon after.
                scope.noop = outcome.get("no_effect").and_then(Value::as_str).map(|n| {
                    format!("the step at line {} ({}) changed nothing on the page: {n}", step.line, step.text)
                });
                Ok(())
            }
            Action::Save { name, command }
                if matches!(command.first().map(String::as_str), Some("json" | "header")) =>
            {
                // `save id = json $.id`, `save to = header Location`: from the last `http`.
                let what = scope.word(command.get(1).ok_or("save NAME = json <$.path> | header <name>")?)?;
                let read = if command[0] == "json" { HttpRead::Json(what) } else { HttpRead::Header(what) };
                let v = http_read(&read, scope)?
                    .ok_or_else(|| format!("the last http answer has no {}", command.join(" ")))?;
                scope.saved.insert(name.clone(), v);
                Ok(())
            }
            Action::Print(command) if crate::suite::print_reads_answer(command) || command[0].contains("${") => {
                let text = if command[0].contains("${") {
                    scope.words(command)?.join(" ")
                } else {
                    print_answer(&scope.words(command)?, scope)?
                };
                scope.notes.push(format!("{PRINTED}{}: {}", step.line, clip(&text, PRINT_MAX)));
                Ok(())
            }
            Action::Print(command) => {
                let words = scope.words(command)?;
                let v = self.command(&words)?;
                let text = match v {
                    Value::String(s) => s,
                    // One line: an agent reads every byte of it.
                    other => other.to_string(),
                };
                scope.notes.push(format!("{PRINTED}{}: {}", step.line, clip(&text, PRINT_MAX)));
                Ok(())
            }
            Action::Save { name, command } => {
                let words = scope.words(command)?;
                let v = self.command(&words)?;
                let text = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                scope.saved.insert(name.clone(), text);
                Ok(())
            }
            Action::Expect(e) => match self.expect(e, scope) {
                // A check that held: the page answered whatever came before,
                // so a click that changed nothing at once no longer explains
                // a later failure.
                Ok(()) => {
                    scope.noop = None;
                    Ok(())
                }
                // Noted; the scenario goes on and fails at its end (in a
                // `retry`, it is a failure to try again on).
                Err(m) if e.soft && !scope.retrying => {
                    scope.soft.push((step.clone(), m));
                    Ok(())
                }
                r => r,
            },
            Action::As(name) => {
                let path = self.login(name, suite)?;
                if self.bare {
                    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let state: Value = serde_json::from_str(&text)
                        .map_err(|e| format!("{}: not a state file ({e})", path.display()))?;
                    self.jar.load(&state, crate::http::unix_now());
                    return Ok(());
                }
                self.command(&["state".into(), "load".into(), path.display().to_string()]).map(|_| ())
            }
            Action::Pause => {
                self.pause(step.line, suite, "the steps before left it");
                Ok(())
            }
            Action::Retry { timeout_ms, steps } => {
                let limit = self.capped(Duration::from_millis(timeout_ms.unwrap_or(self.opts.expect_ms)));
                let started = Instant::now();
                let deadline = started + limit;
                let was = std::mem::replace(&mut scope.retrying, true);
                let mut tries = 0usize;
                let result = loop {
                    tries += 1;
                    let mut res = Ok(());
                    for st in steps {
                        if let Err(e) = self.action(&st.action, st, scope, suite, depth) {
                            res = Err(format!("line {} `{}`: {e}", st.line, st.text));
                            break;
                        }
                    }
                    let Err(e) = res else { break Ok(()) };
                    // A little later each time, and one last try at the deadline.
                    let wait = Duration::from_millis([100, 250, 500, 1000][(tries - 1).min(3)])
                        .min(deadline.saturating_duration_since(Instant::now()));
                    if wait.is_zero() {
                        let secs = started.elapsed().as_secs_f64();
                        break Err(format!("retry: still failing after {tries} tries in {secs:.1} s; the last: {e}"));
                    }
                    std::thread::sleep(wait);
                };
                scope.retrying = was;
                result
            }
            Action::Handoff(until) => self.handoff(until.as_ref(), step, scope),
            Action::If { cond, steps } => {
                if self.expect(cond, scope).is_ok() {
                    for st in steps {
                        self.action(&st.action, st, scope, suite, depth)
                            .map_err(|e| format!("in the `if`, line {} `{}`: {e}", st.line, st.text))?;
                    }
                }
                Ok(())
            }
            Action::Exec { argv, timeout_ms, dir, steps } => {
                let argv = scope.words(argv)?;
                let limit = self.capped(Duration::from_millis(timeout_ms.unwrap_or(60_000)));
                let (code, out, err) = run_program(&argv, dir, limit, self.opts.base.as_deref())?;
                scope.saved.insert("exec.exit".into(), code.to_string());
                scope.saved.insert("exec.stdout".into(), out.trim_end_matches(['\n', '\r']).to_string());
                scope.saved.insert("exec.stderr".into(), err.trim_end_matches(['\n', '\r']).to_string());
                let exit = Subject::Var("${exec.exit}".into());
                let checked = steps.iter().any(|s| matches!(&s.action, Action::Expect(e) if e.subject == exit));
                if code != 0 && !checked {
                    let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                    let last = if last.len() > 300 { &last[..last.floor_char_boundary(300)] } else { last };
                    let said = if last.is_empty() { String::new() } else { format!(": {last}") };
                    return Err(format!("`{}` exited {code}{said}", shown(&argv)));
                }
                for st in steps {
                    self.action(&st.action, st, scope, suite, depth)
                        .map_err(|e| format!("under the `exec`, line {} `{}`: {e}", st.line, st.text))?;
                }
                Ok(())
            }
            Action::Http {
                method,
                url,
                headers,
                body,
                body_file,
                json,
                timeout_ms,
                follow,
                insecure,
                form,
                multipart,
                query,
                auth,
                graphql,
                steps,
            } => {
                let mut url = scope.word(url)?;
                if url.starts_with('/') {
                    let base = self.opts.base.as_deref().ok_or_else(|| {
                        format!("{url} is relative: give the run a base URL (`base <url>` in the file or --base-url)")
                    })?;
                    url = format!("{}{url}", base.trim_end_matches('/'));
                }
                let pairs = |ps: &[(String, String)], scope: &Scope| -> Result<Vec<(String, String)>, String> {
                    ps.iter().map(|(k, v)| Ok((scope.word(k)?, scope.word(v)?))).collect()
                };
                let url = crate::http::with_query(&url, &pairs(query, scope)?);
                let mut headers = scope.words(headers)?;
                let payload = match (body, body_file) {
                    (Some(b), _) => Some(crate::http::Body::Raw(scope.word(b)?)),
                    (None, Some(f)) => {
                        let f = scope.word(f)?;
                        let text = std::fs::read_to_string(&f).map_err(|e| format!("http body file {f}: {e}"))?;
                        Some(crate::http::Body::Raw(text))
                    }
                    (None, None) if graphql.is_some() => {
                        let g = graphql.as_ref().expect("checked");
                        let text = if g.file {
                            let f = scope.word(&g.query)?;
                            std::fs::read_to_string(&f).map_err(|e| format!("http --graphql-file {f}: {e}"))?
                        } else {
                            scope.word(&g.query)?
                        };
                        let mut doc = serde_json::json!({ "query": text });
                        if let Some(v) = &g.variables {
                            let v = scope.word(v)?;
                            doc["variables"] = serde_json::from_str::<Value>(&v)
                                .ok()
                                .filter(Value::is_object)
                                .ok_or_else(|| format!("--variables takes a JSON object, not {v:?}"))?;
                        }
                        if let Some(op) = &g.operation {
                            doc["operationName"] = Value::String(scope.word(op)?);
                        }
                        Some(crate::http::Body::Raw(doc.to_string()))
                    }
                    (None, None) if !form.is_empty() => Some(crate::http::Body::Form(pairs(form, scope)?)),
                    (None, None) if !multipart.is_empty() => {
                        Some(crate::http::Body::Multipart(pairs(multipart, scope)?))
                    }
                    (None, None) => None,
                };
                if let Some((kind, v)) = auth {
                    if !crate::http::has_header(&headers, "authorization") {
                        let v = scope.word(v)?;
                        headers.push(match kind.as_str() {
                            "user" => format!("Authorization: Basic {}", crate::report::base64(v.as_bytes())),
                            _ => format!("Authorization: Bearer {v}"),
                        });
                    }
                }
                if *json && !crate::http::has_header(&headers, "content-type") {
                    headers.push("Content-Type: application/json".into());
                }
                let now = crate::http::unix_now();
                // The scenario's cookies go with it, unless it sets its own.
                if !crate::http::has_header(&headers, "cookie") {
                    let cookie = if self.bare {
                        self.jar.header(&url, now)
                    } else {
                        let jar = self.command(&["cookies".into(), "--url".into(), url.clone(), "--json".into()])?;
                        let pairs: Vec<String> = jar
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|c| Some(format!("{}={}", c.get("name")?.as_str()?, c.get("value")?.as_str()?)))
                            .collect();
                        (!pairs.is_empty()).then(|| pairs.join("; "))
                    };
                    if let Some(c) = cookie {
                        headers.push(format!("Cookie: {c}"));
                    }
                }
                let limit = self.capped(Duration::from_millis(timeout_ms.unwrap_or(self.opts.expect_ms.max(30_000))));
                let how = crate::http::How { follow: *follow, insecure: *insecure };
                let r = crate::http::request(method, &url, &headers, payload.as_ref(), limit, how)?;
                // And what it sets comes back: to the browser, or the jar.
                for (v, at) in &r.cookies {
                    if self.bare {
                        self.jar.store(v, at, now);
                    } else if let Some(words) = crate::http::cookie_words(v, at, now) {
                        self.command(&words)?;
                    }
                }
                scope.saved.insert("http.status".into(), r.status.to_string());
                scope.saved.insert("http.body".into(), r.body.clone());
                scope.saved.insert("http.ms".into(), format!("{}", (r.ms * 10.0).round() / 10.0));
                scope.http_headers = r.headers.clone();
                let kind = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| format!(", {}", v.split(';').next().unwrap_or(v).trim()))
                    .unwrap_or_default();
                scope.exchanges.push(format!(
                    "{method} {url} -> {} ({:.0} ms, {} B{kind})",
                    r.status,
                    r.ms,
                    r.body.len()
                ));
                let status = Subject::Var("${http.status}".into());
                let checked = steps.iter().any(|s| matches!(&s.action, Action::Expect(e) if e.subject == status));
                if r.status >= 400 && !checked {
                    let said = r.body.trim();
                    let said = if said.len() > 200 { &said[..said.floor_char_boundary(200)] } else { said };
                    let said = if said.is_empty() { String::new() } else { format!(": {said}") };
                    return Err(format!("{method} {url} answered {}{said}", r.status));
                }
                // GraphQL answers its errors with a 200: they fail the step,
                // unless a step under it is there to check them.
                if graphql.is_some() && !steps.iter().any(|st| st.text.contains("$.errors")) {
                    let errors = serde_json::from_str::<Value>(&r.body).ok().and_then(|d| d.get("errors").cloned());
                    if let Some(list) = errors.as_ref().and_then(Value::as_array).filter(|l| !l.is_empty()) {
                        let said: Vec<String> = list
                            .iter()
                            .take(3)
                            .map(|e| {
                                e.get("message").and_then(Value::as_str).map_or_else(|| e.to_string(), String::from)
                            })
                            .collect();
                        return Err(format!(
                            "{method} {url}: the GraphQL answer has {} error(s): {}",
                            list.len(),
                            said.join("; ")
                        ));
                    }
                }
                for st in steps {
                    self.action(&st.action, st, scope, suite, depth)
                        .map_err(|e| format!("under the `http`, line {} `{}`: {e}", st.line, st.text))?;
                }
                Ok(())
            }
            Action::Gherkin(text) => Err(format!("no step matches {text:?}")),
            Action::Optional(inner) => {
                if let Err(e) = self.action(inner, step, scope, suite, depth) {
                    scope.notes.push(format!("optional step at line {} didn't happen: {e}", step.line));
                    scope.optional_missed = true;
                }
                Ok(())
            }
            Action::Run { flow, args } => {
                if depth >= 16 {
                    return Err(format!("flows nest more than 16 deep at {flow} (a flow that runs itself?)"));
                }
                let f = suite.flows.get(flow).ok_or_else(|| format!("no flow named {flow:?}"))?;
                let values = scope.words(args)?;
                scope.layers.push(f.params.iter().cloned().zip(values).collect());
                let mut result = Ok(());
                for st in &f.steps {
                    if let Err(e) = self.action(&st.action, st, scope, suite, depth + 1) {
                        // The innermost step that failed (set first, on the way out).
                        scope.inner.get_or_insert_with(|| (f.file.clone(), st.clone()));
                        let file = f.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        result = Err(format!("in flow {flow}, {file}:{} `{}`: {e}", st.line, st.text));
                        break;
                    }
                }
                scope.layers.pop();
                result
            }
        }
    }

    /// The state file of an `auth` block: saved by running it once in a
    /// browser context of its own, or reused from an earlier run within
    /// its TTL.
    fn login(&mut self, name: &str, suite: &Suite) -> Result<PathBuf, String> {
        let auth = suite.auths.get(name).ok_or_else(|| format!("no auth named {name:?}"))?;
        let slot = self.opts.auth.slot(name);
        let mut done = slot.lock().map_err(|_| "auth: a worker failed while signing in".to_string())?;
        if let Some(r) = done.as_ref() {
            return r.clone().map_err(|e| format!("auth {name} failed earlier in this run: {e}"));
        }
        // Keyed by where it signs in: another base URL, another state.
        let base = self.opts.base.clone().or_else(|| suite.base.clone()).unwrap_or_default();
        let key = base.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
        let dir = std::path::absolute(".bitti/auth").unwrap_or_else(|_| PathBuf::from(".bitti/auth"));
        let path = dir.join(format!("{name}-{:08x}.json", key as u32));
        let reusable = auth.ttl_s.is_some_and(|ttl| {
            std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() < ttl)
        });
        let result = if reusable { Ok(path) } else { self.sign_in(auth, suite, &dir, path) };
        *done = Some(result.clone());
        result
    }

    fn sign_in(
        &mut self,
        auth: &crate::suite::Auth,
        suite: &Suite,
        dir: &Path,
        path: PathBuf,
    ) -> Result<PathBuf, String> {
        // An auth of `http` steps alone signs in with no browser: its jar's
        // cookies are saved in the state format a browser loads too.
        let api = !crate::suite::needs_browser(suite, &auth.steps);
        let mut aux = if api {
            let mut r = Runner::bare(self.opts.clone());
            r.bare = true;
            r
        } else {
            self.fork().map_err(|e| format!("auth {}: {e:#}", auth.name))?
        };
        let label = format!("auth {}", auth.name);
        let mut scope = Scope::new(suite, &label, BTreeMap::new(), &self.opts, &BTreeMap::new());
        let file = auth.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for st in &auth.steps {
            aux.action(&st.action, st, &mut scope, suite, 0)
                .map_err(|e| scope.masker()(&format!("auth {}, {file}:{} `{}`: {e}", auth.name, st.line, st.text)))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if api {
            let state = aux.jar.state(crate::http::unix_now()).to_string();
            std::fs::write(&path, state).map_err(|e| format!("{}: {e}", path.display()))?;
        } else {
            aux.command(&["state".into(), "save".into(), path.display().to_string()])?;
        }
        // Signed-in cookies: for this user only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(path)
    }

    /// The browser on screen for a person, until the clause holds (or
    /// Enter); then off screen again. Nothing to do when it already holds.
    fn handoff(&mut self, until: Option<&Expect>, step: &Step, scope: &mut Scope) -> Result<(), String> {
        if let Some(c) = until {
            if self.expect(&Expect { timeout_ms: Some(0), ..c.clone() }, scope).is_ok() {
                return Ok(());
            }
        }
        let show = !self.opts.headed;
        if show {
            if cfg!(target_os = "linux")
                && std::env::var_os("DISPLAY").is_none()
                && std::env::var_os("WAYLAND_DISPLAY").is_none()
            {
                return Err("handoff needs a person at a screen, and there is no display here (DISPLAY is unset): run it where there is one, or start signed in (`as <auth>`, `state load`)".into());
            }
            self.command(&["show".into()])?;
        }
        let what = match until {
            Some(_) => {
                format!("the run goes on once `{}` holds, or on Enter", step.text.trim_start_matches("handoff until "))
            }
            None => "press Enter when done".into(),
        };
        eprintln!("HANDOFF line {}: the browser is on screen for you; {what}", step.line);
        let res = match until {
            Some(c) => {
                let deadline = Instant::now() + Duration::from_millis(c.timeout_ms.unwrap_or(300_000));
                loop {
                    let look = Expect { timeout_ms: Some(0), ..c.clone() };
                    let entered = matches!(enter(Some(Duration::from_millis(250))), Ok(true));
                    match self.expect(&look, scope) {
                        Ok(()) => break Ok(()),
                        Err(e) if entered || Instant::now() >= deadline => {
                            break Err(format!("handoff: {e}"));
                        }
                        Err(_) => {}
                    }
                }
            }
            None => match enter(None) {
                Ok(_) => Ok(()),
                Err(()) => {
                    Err("handoff waits for Enter, and there is no terminal input here: give it `until <expect clause>`"
                        .into())
                }
            },
        };
        if show {
            if let Err(e) = self.command(&["hide".into()]) {
                return res.and(Err(format!("handoff: back off screen: {e}")));
            }
        }
        res
    }

    /// The body of response `#n` in the page's network log, as text (empty
    /// when it can't be read). Read through a file in the run's own
    /// evidence directory: a fixed name in the shared temp directory is one
    /// another user could have put a link at first.
    fn response_body(&mut self, n: &str) -> String {
        let dir = self.opts.evidence.join(".bodies");
        if std::fs::create_dir_all(&dir).is_err() {
            return String::new();
        }
        let file = dir.join(format!("{}-{n}.txt", std::process::id()));
        let got = self.command(&["network".into(), n.to_string(), "--body-file".into(), file.display().to_string()]);
        let body = got.ok().and_then(|_| std::fs::read_to_string(&file).ok()).unwrap_or_default();
        let _ = std::fs::remove_file(&file);
        // Gone again, and the run's directory with it if nothing else is in
        // it (a passing run keeps no evidence).
        let _ = std::fs::remove_dir(&dir);
        let _ = std::fs::remove_dir(&self.opts.evidence);
        body
    }

    /// One navigera command line, run in-process.
    fn command(&mut self, words: &[String]) -> Result<Value, String> {
        let words = self.absolute(words)?;
        // A key chord's ControlOrMeta, as Playwright spells it: Meta on a
        // Mac, Control elsewhere.
        let words = if matches!(words.first().map(String::as_str), Some("press" | "key" | "keypress")) {
            let modifier = if cfg!(target_os = "macos") { "Meta" } else { "Control" };
            words.iter().map(|w| w.replace("ControlOrMeta", modifier)).collect()
        } else {
            words
        };
        let cmd = parse(&words)?;
        if matches!(cmd, Command::Quit { .. }) {
            return Err("`quit` isn't a step: the run closes the browser".into());
        }
        if self.bare {
            return Err(format!(
                "`{}` needs a browser, and this runs without one (only `http`, `exec` and their checks): a bitti bug, please report it",
                words.join(" ")
            ));
        }
        let driver = self.driver().map_err(|e| browser_failed(&format!("{e:#}")))?;
        driver.run_op(&cmd).map(|o| o.result).map_err(|e| format!("{e:#}"))
    }

    /// `goto /cart` against the base URL.
    fn absolute(&self, words: &[String]) -> Result<Vec<String>, String> {
        let mut words = words.to_vec();
        if matches!(words.first().map(String::as_str), Some("goto" | "open" | "tab-new")) {
            if let Some(url) = words.get(1).filter(|u| u.starts_with('/')) {
                let base = self.opts.base.as_deref().ok_or_else(|| {
                    format!("{url} is relative: give the run a base URL (`base <url>` in the file or --base-url)")
                })?;
                words[1] = format!("{}{url}", base.trim_end_matches('/'));
            }
        }
        Ok(words)
    }

    /// The one in-page wait for an `expect` whose subject navigera can
    /// watch (see `expect`): the wait's words, or None when the subject
    /// stays on the 25 ms loop. The predicate mirrors the read `check`
    /// makes and asks for the success side (`holds != negate`); an
    /// expression error returns true so a mirrored predicate that
    /// diverges from the real read wakes early and the confirming read
    /// decides, never blocks a pass. Only `--selector <css>` targets map
    /// to one; the richer targets have no page-side predicate here.
    fn wake_wait(
        &mut self,
        e: &Expect,
        value: &str,
        scope: &Scope,
        left: Duration,
    ) -> Result<Option<Vec<String>>, String> {
        let ms = left.as_millis().max(1).to_string();
        let negate = if e.negate { "true" } else { "false" };
        let predicate = match &e.subject {
            // `expect not text`: the read is an eval over the top-level
            // documents; the wait is its exact mirror on the success side.
            Subject::Text { text: t, within: None } if e.negate => format!(
                "(() => {{ try {{ const t = {}; const docs = [document];                  for (const f of document.querySelectorAll('iframe,frame')) {{ try {{ if (f.contentDocument) docs.push(f.contentDocument); }} catch (_) {{}} }}                  return !docs.some((d) => d.body && d.body.innerText.includes(t)); }} catch (_) {{ return true; }} }})()",
                serde_json::to_string(&scope.word(t)?).unwrap_or_default()
            ),
            Subject::Target { flags, prop } => {
                let flags = scope.words(flags)?;
                let sel = match flags.as_slice() {
                    [flag, sel] if flag == "--selector" => sel.clone(),
                    _ => return Ok(None),
                };
                let sel = serde_json::to_string(&sel).unwrap_or_default();
                let value_j = serde_json::to_string(value).unwrap_or_default();
                let m = e.matcher.word();
                let cmp = |actual: &str| format!("__btCmp(({actual}), {m:?}, {value_j})");
                let holds = match prop {
                    Prop::State(state) if state == "visible" => {
                        format!("__btVisible(__btFirst({sel}))")
                    }
                    // A missing element is hidden, as `is hidden` answers.
                    Prop::State(state) if state == "hidden" => {
                        format!("!__btVisible(__btFirst({sel}))")
                    }
                    Prop::State(state) if state == "enabled" => {
                        format!("__btEnabled(__btFirst({sel}))")
                    }
                    Prop::State(state) if state == "disabled" => {
                        format!("__btDisabled(__btFirst({sel}))")
                    }
                    // Checked and focused change by property, which no
                    // observer sees: navigera's wake also ends on input,
                    // change and focus events, and [`Runner::expect`] caps
                    // each of these waits at the poll's 25 ms, so a silent
                    // write is seen no later than the poll would see it.
                    Prop::State(state) if state == "checked" => {
                        format!("__btChecked(__btFirst({sel}))")
                    }
                    // A missing box is unchecked, as the read answers.
                    Prop::State(state) if state == "unchecked" => {
                        format!("!__btChecked(__btFirst({sel}))")
                    }
                    Prop::State(state) if state == "focused" => {
                        format!("__btFocused(__btFirst({sel}))")
                    }
                    // A bare `expect <target> <prop>` (no matcher) is a
                    // read, not a condition: nothing to wait for.
                    Prop::Count | Prop::Text | Prop::Value | Prop::Attr(_) | Prop::Style(_)
                        if e.matcher == crate::expect::Matcher::Holds =>
                    {
                        return Ok(None)
                    }
                    Prop::Count => cmp(&format!("String(__btAll({sel}).length)")),
                    Prop::Text => cmp(&format!(
                        "(__btFirst({sel}) ? (__btFirst({sel}).innerText ?? __btFirst({sel}).textContent ?? '') : '').trim()"
                    )),
                    Prop::Value => cmp(&format!("(__btFirst({sel}) ? String(__btFirst({sel}).value ?? '') : '').trim()")),
                    Prop::Attr(name) => cmp(&format!(
                        "(__btFirst({sel}) ? String(__btFirst({sel}).getAttribute({}) ?? '') : '').trim()",
                        serde_json::to_string(name).unwrap_or_default()
                    )),
                    Prop::Style(name) => cmp(&format!(
                        "(__btFirst({sel}) ? String((__btFirst({sel}).ownerDocument.defaultView || window).getComputedStyle(__btFirst({sel})).getPropertyValue({}) ?? '') : '').trim()",
                        serde_json::to_string(name).unwrap_or_default()
                    )),
                    _ => return Ok(None),
                };
                format!("(() => {{ try {{ {WAKE_DEEP}{WAKE_CMP} return ({holds}) !== {negate}; }} catch (_) {{ return true; }} }})()")
            }
            _ => return Ok(None),
        };
        Ok(Some(vec!["wait".into(), "--js".into(), predicate, "--timeout-ms".into(), ms]))
    }

    fn expect(&mut self, e: &Expect, scope: &mut Scope) -> Result<(), String> {
        // In a `retry`, the block is the wait: a check looks once.
        let default_ms = if scope.retrying { 0 } else { self.opts.expect_ms };
        let limit = self.capped(Duration::from_millis(e.timeout_ms.unwrap_or(default_ms)));
        let deadline = Instant::now() + limit;
        let value = scope.word(&e.value)?;
        let mut woken = false;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (holds, actual) = self.check(e, &value, scope, left)?;
            if holds != e.negate {
                // The file that came: `${download.path}`, `${download.name}`.
                if let (Subject::Download(_) | Subject::DownloadCompare { .. }, false) = (&e.subject, e.negate) {
                    let name = Path::new(&actual).file_name().map(|n| n.to_string_lossy().into_owned());
                    scope.saved.insert("download.name".into(), name.unwrap_or_default());
                    scope.saved.insert("download.path".into(), actual);
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(describe(e, &value, &actual, limit));
            }
            // The DOM subjects navigera can watch in the page: after the
            // first real read says not-yet, one wait bounded by the time
            // left has the page wake the runner instead of the 25 ms poll.
            // The predicate mirrors the read check() makes; a fired wait is
            // confirmed by the loop's next read, and a wait that errors or
            // times out leaves the loop to finish as it does without it
            // (its last read gives the failure its actual). An expectation
            // that already holds pays nothing for this. A state a property
            // holds (checked, focused) wakes on the events that change it,
            // not on a silent write: its wait is capped at the poll's 25 ms
            // and taken again until it fires. A wake that fires and the read
            // then disagrees leaves the rest to the poll.
            if !woken {
                let left = deadline.saturating_duration_since(Instant::now());
                let by_property = matches!(&e.subject, Subject::Target { prop: Prop::State(s), .. }
                    if matches!(s.as_str(), "checked" | "unchecked" | "focused"));
                woken = !by_property;
                if !left.is_zero() {
                    let cap = if by_property { left.min(POLL) } else { left };
                    if let Some(words) = self.wake_wait(e, &value, scope, cap)? {
                        if self.command(&words).is_ok() {
                            woken = true;
                        }
                        continue;
                    }
                }
                woken = true;
            }
            std::thread::sleep(POLL);
        }
    }

    /// `expect json … snapshot <name>`: the answer (its `--ignore`d values
    /// left out) against `<file>.bt.snapshots/<name>.json`. No baseline
    /// yet: it is written and the check fails, so a person reads it first;
    /// `--update-snapshots` writes or replaces it and passes.
    fn json_snapshot(
        &self,
        path: &str,
        name: &str,
        ignore: &[String],
        scope: &Scope,
    ) -> Result<(bool, String), String> {
        let body = scope.saved.get("http.body").ok_or("no `http` answer yet: an `http` step goes first")?;
        let doc: Value = serde_json::from_str(body).map_err(|e| format!("the answer isn't JSON ({e})"))?;
        let Some(at) = crate::expect::json_value_at(&doc, &crate::expect::json_path(&scope.word(path)?)?) else {
            return Ok((false, format!("nothing at {path}")));
        };
        let now = crate::schema::without(at, ignore);
        let stem = scope.bt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = scope.bt.parent().unwrap_or(Path::new(".")).join(format!("{stem}.snapshots"));
        let baseline = dir.join(format!("{name}.json"));
        // As the run was given it: `/` between the parts on every OS.
        let shown = format!("{}.snapshots/{name}.json", scope.bt.display());
        let write = |why: &str| -> Result<(bool, String), String> {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let text = serde_json::to_string_pretty(&now).unwrap_or_default() + "\n";
            std::fs::write(&baseline, text).map_err(|e| format!("{}: {e}", baseline.display()))?;
            Ok((self.opts.update_shots, format!("{why} {shown}")))
        };
        let was = match std::fs::read_to_string(&baseline) {
            Ok(t) => serde_json::from_str::<Value>(&t).map_err(|e| format!("{shown} isn't JSON ({e})"))?,
            Err(_) => {
                return write(if self.opts.update_shots {
                    "wrote the baseline"
                } else {
                    "no baseline yet: wrote it, read it and run again:"
                })
            }
        };
        // The baseline as kept, its own ignored values left out too.
        let was = crate::schema::without(&was, ignore);
        let diff = crate::schema::differences(&was, &now);
        if diff.is_empty() {
            return Ok((true, "the same".into()));
        }
        if self.opts.update_shots {
            return write("updated the baseline");
        }
        Ok((false, format!("{shown} differs: {}", diff.join("; "))))
    }

    /// `expect screenshot`: take it (masks painted, animations stopped),
    /// crop it to its target, and compare it with the baseline beside the
    /// `.bt` file (`<file>.screenshots/<name>-<os>.png`). No baseline yet:
    /// it is written, and the step fails so a person looks at it first.
    /// With a baseline to meet, the first capture is compared at once and
    /// a match passes. A mismatch, or a baseline being written or updated,
    /// waits for a settled page: two captures a frame apart must agree and
    /// the page's quiet clock (QUIET_CLOCK) must show no DOM change for
    /// half a second; the expectation's own time caps the wait, the last
    /// capture stands, and a failed compare says the page never settled.
    fn screenshot(&mut self, shot: &Shot, scope: &Scope, left: Duration) -> Result<(bool, String), String> {
        let stem = scope.bt.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = scope.bt.parent().unwrap_or(Path::new(".")).join(format!("{stem}.screenshots"));
        let baseline = dir.join(format!("{}-{}.png", shot.name, std::env::consts::OS));
        let flags = scope.words(&shot.flags)?;
        // Still and masked while it is taken.
        let masks = serde_json::to_string(&shot.masks).unwrap_or_default();
        let freeze = format!(
            "(() => {{ const s = document.createElement('style'); s.id = '__bt_freeze'; \
             s.textContent = '*,*::before,*::after{{animation:none!important;transition:none!important;caret-color:transparent!important}}'; \
             document.documentElement.append(s); const n = []; for (const css of {masks}) {{ const all = document.querySelectorAll(css); \
             n.push(all.length); for (const e of all) {{ \
             const r = e.getBoundingClientRect(); const m = document.createElement('div'); m.className = '__bt_mask'; \
             m.style.cssText = `position:absolute;left:${{r.left + scrollX}}px;top:${{r.top + scrollY}}px;width:${{r.width}}px;height:${{r.height}}px;background:#ff00ff;z-index:2147483647;pointer-events:none`; \
             document.documentElement.append(m); }} }} return [scrollX, scrollY, devicePixelRatio, n]; }})()"
        );
        let unfreeze = "(() => { document.getElementById('__bt_freeze')?.remove(); for (const m of document.querySelectorAll('.__bt_mask')) m.remove(); return 1; })()";
        // The target in view first: scrolled inside its scroll containers
        // and frames, which a full-page capture shows only as they stand.
        if !flags.is_empty() {
            let mut w = vec!["scroll".to_string()];
            w.extend(flags.iter().cloned());
            self.command(&w)?;
        }
        let at = self.command(&["eval".into(), freeze])?;
        let n = |i: usize| at.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        let (sx, sy, dpr) = (n(0), n(1), n(2).max(1.0));
        // A mask that hides nothing would let what it was for into the baseline.
        let counts = at.get(3).and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some((css, _)) = shot.masks.iter().zip(&counts).find(|(_, c)| c.as_u64() == Some(0)) {
            let _ = self.command(&["eval".into(), unfreeze.into()]);
            return Err(format!(
                "--mask {css:?} matches nothing on the page (masks are CSS selectors of the top page)"
            ));
        }
        // Its box where the capture will have it: page pixels, frames' offsets in.
        let target = if flags.is_empty() {
            None
        } else {
            let mut w = vec!["get".to_string(), "box".into()];
            w.extend(flags.iter().cloned());
            match self.command(&w) {
                Ok(b) => Some(b),
                Err(e) => {
                    let _ = self.command(&["eval".into(), unfreeze.into()]);
                    return Err(e);
                }
            }
        };
        let tmp = self.opts.evidence.join(format!("{stem}-{}-taking.png", shot.name));
        let _ = std::fs::create_dir_all(&self.opts.evidence);
        let mut words = vec!["screenshot".to_string(), "--path".into(), tmp.display().to_string()];
        if shot.full_page || !flags.is_empty() {
            words.push("--full-page".into());
        }
        let crop = |img: crate::shot::Image| -> crate::shot::Image {
            match &target {
                None => img,
                Some(b) => {
                    let f = |k: &str| b.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    let px = |v: f64| (v * dpr).round().max(0.0) as u32;
                    crate::shot::crop(&img, px(f("x") + sx), px(f("y") + sy), px(f("width")), px(f("height")))
                }
            }
        };
        // Match-first, as Playwright's toHaveScreenshot: with a baseline
        // to meet (not one being written or updated), the first capture
        // is cropped and compared at once, and a match passes on that one
        // capture, with no quiet clock. Only a mismatch runs the stability
        // loop below before the final compare.
        if !self.opts.update_shots {
            if let Ok(base_bytes) = std::fs::read(&baseline) {
                let first = (|| -> Result<crate::shot::Image, String> {
                    self.command(&words).map(|_| ())?;
                    let bytes = std::fs::read(&tmp).map_err(|e| format!("screenshot {}: {e}", tmp.display()))?;
                    crate::shot::decode(&bytes)
                })();
                match first {
                    Ok(img) => {
                        let img = crop(img);
                        let base =
                            crate::shot::decode(&base_bytes).map_err(|e| format!("{}: {e}", baseline.display()))?;
                        let ok = match crate::shot::compare(&base, &img, shot.threshold) {
                            Ok(d) => shot.allows(d.pixels, d.total),
                            Err(_) => false,
                        };
                        if ok {
                            let _ = self.command(&["eval".into(), unfreeze.into()]);
                            let _ = std::fs::remove_file(&tmp);
                            return Ok((true, "it matches".into()));
                        }
                        // A mismatch is what makes it wait: the stability
                        // loop decides the final compare.
                    }
                    Err(e) => {
                        let _ = self.command(&["eval".into(), unfreeze.into()]);
                        let _ = std::fs::remove_file(&tmp);
                        return Err(e);
                    }
                }
            }
        }
        // The init script registered before this scenario's first goto
        // has the clock on every document since; a page that predates
        // it gets the clock now, its quiet timed from here (a no-op
        // once the clock runs).
        let _ = self.command(&["eval".into(), QUIET_CLOCK.into()]);
        // Keep room after the last capture for the crop and the compare.
        let deadline = Instant::now() + left;
        let reserve = Duration::from_millis(250);
        let budget_ms = left.as_millis();
        // A fixed beat between captures, then how quiet the page is in
        // milliseconds (-1: the clock is not on this document, which
        // predates both the init script and the direct install, as a
        // document can mid-navigation). A timer, not rAF: headless Chrome can
        // starve animation frames between captures.
        let frame = "new Promise(r=>setTimeout(()=>r(window.__btQuiet === undefined ? -1 : performance.now() - window.__btQuiet), 50))";
        let started = Instant::now();
        let mut prev: Option<crate::shot::Image> = None;
        let (img, stable) = loop {
            let took = self.command(&words);
            if let Err(e) = took {
                let _ = self.command(&["eval".into(), unfreeze.into()]);
                return Err(e);
            }
            let bytes = match std::fs::read(&tmp) {
                Ok(b) => b,
                Err(e) => {
                    let _ = self.command(&["eval".into(), unfreeze.into()]);
                    return Err(format!("screenshot {}: {e}", tmp.display()));
                }
            };
            let this = match crate::shot::decode(&bytes) {
                Ok(i) => i,
                Err(e) => {
                    let _ = self.command(&["eval".into(), unfreeze.into()]);
                    return Err(e);
                }
            };
            let agreed = prev.as_ref() == Some(&this);
            let quiet = self.command(&["eval".into(), frame.into()]).ok().and_then(|v| v.as_f64()).unwrap_or(-1.0);
            // Without the clock, half a second of looking stands in.
            if agreed && (quiet >= 500.0 || (quiet < 0.0 && started.elapsed() >= Duration::from_millis(500))) {
                break (this, true);
            }
            if deadline.saturating_duration_since(Instant::now()) <= reserve {
                break (this, false);
            }
            prev = Some(this);
        };
        let _ = self.command(&["eval".into(), unfreeze.into()]);
        let _ = std::fs::remove_file(&tmp);
        let img = crop(img);
        let write = |path: &Path, img: &crate::shot::Image| -> Result<(), String> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(path, crate::shot::encode(img)).map_err(|e| format!("{}: {e}", path.display()))
        };
        let Ok(base_bytes) = std::fs::read(&baseline) else {
            write(&baseline, &img)?;
            if self.opts.update_shots {
                return Ok((true, "baseline written".into()));
            }
            return Err(format!(
                "no baseline yet: wrote {} from this run; look at it, keep it with the tests, and run again",
                baseline.display()
            ));
        };
        let base = crate::shot::decode(&base_bytes).map_err(|e| format!("{}: {e}", baseline.display()))?;
        let unsettled = if stable {
            String::new()
        } else {
            format!("; the page never settled: still changing after {budget_ms} ms")
        };
        let verdict = crate::shot::compare(&base, &img, shot.threshold);
        let ok = match &verdict {
            Ok(d) => shot.allows(d.pixels, d.total),
            Err(_) => false,
        };
        if ok {
            return Ok((true, "it matches".into()));
        }
        if self.opts.update_shots {
            write(&baseline, &img)?;
            return Ok((true, "baseline updated".into()));
        }
        let actual = self.opts.evidence.join(format!("{stem}-{}-actual.png", shot.name));
        write(&actual, &img)?;
        Ok((
            false,
            match verdict {
                Ok(d) => {
                    let diff = self.opts.evidence.join(format!("{stem}-{}-diff.png", shot.name));
                    write(&diff, &crate::shot::diff_image(&base, &img, shot.threshold))?;
                    format!(
                        "{} of {} pixels differ ({:.2}%), {} allowed{unsettled}; this run {}, the differences {}",
                        d.pixels,
                        d.total,
                        100.0 * d.pixels as f64 / d.total.max(1) as f64,
                        shot.allowed(),
                        actual.display(),
                        diff.display()
                    )
                }
                Err(why) => format!("{why}{unsettled}; this run {}", actual.display()),
            },
        ))
    }

    /// The latest finished download whose name matches `glob`: its path;
    /// or what downloads there are.
    fn download_done(&mut self, glob: &str) -> Result<(bool, String), String> {
        let v = self.command(&["downloads".into(), "--json".into()])?;
        let list = v.as_array().cloned().unwrap_or_default();
        let name = |d: &Value| d.get("file").and_then(Value::as_str).unwrap_or("").to_string();
        let done = list.iter().rev().find(|d| {
            crate::expect::glob(glob, &name(d)) && d.get("state").and_then(Value::as_str) == Some("completed")
        });
        Ok(match done {
            Some(d) => (true, d.get("path").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| name(d))),
            None => {
                let seen: Vec<String> = list
                    .iter()
                    .map(|d| format!("{} ({})", name(d), d.get("state").and_then(Value::as_str).unwrap_or("?")))
                    .collect();
                (false, if seen.is_empty() { "no download".into() } else { seen.join(", ") })
            }
        })
    }

    /// compara on a downloaded file and the expected one: nothing when they
    /// are the same, else its counts and what it wrote to the evidence
    /// directory (`report`: html, json, rows), each unless the step's own
    /// compara flags already say where.
    fn compara(&self, got: &str, expected: &str, flags: &[String], report: &[String]) -> Result<(), String> {
        let exe = std::env::var("BITTI_COMPARA").unwrap_or_else(|_| "compara".into());
        let stem = Path::new(got).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // Beside the scenario's downloads (`<evidence>/<scenario>/downloads/`).
        let dir = Path::new(got)
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.opts.evidence.clone());
        let _ = std::fs::create_dir_all(&dir);
        let given = |names: &[&str]| flags.iter().any(|f| names.contains(&f.as_str()));
        let mut outputs: Vec<(&str, &str, PathBuf)> = Vec::new();
        for (kind, flag, names, what, file) in [
            ("html", "-o", &["-o", "--out"][..], "the report", format!("{stem}-compare.html")),
            ("json", "--json", &["--json"][..], "the summary", format!("{stem}-compare.json")),
            ("rows", "--export-dir", &["--export-dir"][..], "the rows", format!("{stem}-compare-rows")),
        ] {
            if report.iter().any(|r| r == kind) && !given(names) {
                outputs.push((flag, what, dir.join(file)));
            }
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(["compare", got, expected]).args(flags);
        for (flag, _, path) in &outputs {
            cmd.arg(flag).arg(path);
        }
        let out = cmd.output().map_err(|e| {
            format!(
                "compare needs compara ({exe}: {e}; cargo install compara, npm i -g compara or pip install compara)"
            )
        })?;
        let said = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        let first = said.lines().find(|l| !l.trim().is_empty() && !l.starts_with("Report:")).unwrap_or("").trim();
        let written: Vec<String> = outputs.iter().map(|(_, what, path)| format!("{what} {}", path.display())).collect();
        let written = if written.is_empty() { String::new() } else { format!("; {}", written.join(", ")) };
        match out.status.code() {
            Some(0) => Ok(()),
            Some(1) => Err(format!("{got} differs from {expected}: {first}{written}")),
            _ => Err(format!("compara on {got} and {expected}: {first}")),
        }
    }

    /// How many console messages the tab has so far.
    fn console_count(&mut self) -> usize {
        match self.command(&["console".into(), "--json".into()]) {
            Ok(Value::Array(all)) => all.len(),
            _ => 0,
        }
    }

    /// The tab's latest request number.
    fn last_request(&mut self) -> u64 {
        match self.command(&["network".into()]) {
            Ok(Value::String(t)) => crate::trace::last_request(&t),
            _ => 0,
        }
    }

    /// A step's line of the trace: where the page is, and the console
    /// messages and requests since the step before (`seen`: how many
    /// messages, the last request number).
    fn trace_step(&mut self, step: &Step, ms: f64, error: Option<&str>, seen: &mut (usize, u64)) -> Value {
        let page = match self.command(&["url".into()]) {
            Ok(v) => (
                v.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
                v.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            ),
            Err(_) => Default::default(),
        };
        let console = match self.command(&["console".into(), "--json".into()]) {
            Ok(Value::Array(all)) => {
                // A shorter list is a new tab's: all of it is new.
                let from = if all.len() < seen.0 { 0 } else { seen.0 };
                seen.0 = all.len();
                all[from..].iter().take(200).cloned().collect()
            }
            _ => Vec::new(),
        };
        let network = match self.command(&["network".into(), "--since".into(), seen.1.to_string()]) {
            Ok(Value::String(t)) => {
                seen.1 = crate::trace::last_request(&t).max(seen.1);
                t.lines().filter(|l| l.starts_with('#')).take(200).map(String::from).collect()
            }
            _ => Vec::new(),
        };
        crate::trace::step(step.line, &step.text, ms, error, page, console, network)
    }

    /// What the page looked like after a traced step: its ax tree and a
    /// JPEG of the viewport, named `<n>-L<line>` in the scenario's directory
    /// and noted in the step's trace line (`ax`, `shot`). Neither waits for
    /// the page to go quiet (the step before did that), and a read that
    /// fails leaves its key out.
    /// A step that left the page as the last captured one was (the same
    /// fingerprint: address, element count, text length, field values,
    /// scroll) reuses that step's files and says so (`same_as`), unless it
    /// failed: that one is always captured.
    fn step_files(
        &mut self,
        n: usize,
        line: usize,
        failed: bool,
        last: &mut Option<(String, usize, Option<Value>, Option<Value>)>,
        tr: &mut Value,
        files: &mut Vec<(String, Vec<u8>)>,
    ) {
        const FINGERPRINT: &str = "(() => { const d = document; let h = location.href + '|' + d.getElementsByTagName('*').length + '|' + (d.body ? d.body.innerText.length : 0) + '|' + scrollX + ',' + scrollY + '|' + (d.activeElement ? d.activeElement.tagName + (d.activeElement.id || '') : ''); for (const e of d.querySelectorAll('input,select,textarea,details,dialog')) h += '|' + (e.value || '') + (e.checked ? 'c' : '') + (e.open ? 'o' : ''); return h; })()";
        let fp = match self.command(&["eval".into(), FINGERPRINT.into(), "--timeout-ms".into(), "1000".into()]) {
            Ok(Value::String(s)) => Some(s),
            _ => None,
        };
        if let (Some(fp), Some((was, from, ax, shot)), false) = (&fp, last.as_ref(), failed) {
            if fp == was {
                tr["same_as"] = Value::from(*from);
                if let Some(v) = ax {
                    tr["ax"] = v.clone();
                }
                if let Some(v) = shot {
                    tr["shot"] = v.clone();
                }
                return;
            }
        }
        let stem = format!("{:03}-L{line}", n + 1);
        if let Ok(Value::String(ax)) = self.command(&["ax".into(), "--no-wait".into()]) {
            let name = format!("{stem}.ax.txt");
            tr["ax"] = Value::String(name.clone());
            files.push((name, ax.into_bytes()));
        }
        // Workers are threads of one process: the name must be unique per capture.
        static CAPTURES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = CAPTURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // In this user's own temp directory: the shared one would show each
        // step's page to every account on the machine, under a name they can guess.
        let tmp = navigera::session::private_temp_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join(format!("bitti-step-{}-{seq}.jpg", std::process::id()));
        // A capture that cannot finish (a page stuck behind a native dialog never
        // answers) is dropped after a second, not after navigera's 35 s.
        let words = [
            "screenshot",
            "--no-wait",
            "--quality",
            "60",
            "--timeout-ms",
            "1000",
            "--path",
            &tmp.display().to_string(),
        ];
        if self.command(&words.map(String::from)).is_ok() {
            if let Ok(bytes) = std::fs::read(&tmp) {
                let name = format!("{stem}.jpg");
                tr["shot"] = Value::String(name.clone());
                files.push((name, bytes));
            }
        }
        let _ = std::fs::remove_file(&tmp);
        *last = fp.map(|fp| (fp, line, tr.get("ax").cloned(), tr.get("shot").cloned()));
    }

    /// One look: whether the subject holds, and what was seen.
    fn check(&mut self, e: &Expect, value: &str, scope: &Scope, left: Duration) -> Result<(bool, String), String> {
        // Each read waits for its element at most this long, so an
        // expectation keeps being re-read until its own deadline. When the
        // element's absence is an answer (`hidden`, any `not …`), a read
        // doesn't wait for it to appear: waiting would hold every such look up
        // by a second (a dismissed banner would take 2 s to be seen gone).
        let absence_answers =
            e.negate || matches!(&e.subject, Subject::Target { prop: Prop::State(s), .. } if s == "hidden");
        let poll_ms = if absence_answers { "0".to_string() } else { left.as_millis().clamp(50, 1000).to_string() };
        Ok(match &e.subject {
            Subject::Url | Subject::Title => {
                let v = self.command(&["url".into()])?;
                let key = if e.subject == Subject::Url { "url" } else { "title" };
                let actual = v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                (compare(&actual, e.matcher, value), actual)
            }
            Subject::Text { text, within: Some(flags) } => {
                let t = scope.word(text)?;
                let mut words = vec!["get".to_string(), "text".to_string()];
                words.extend(scope.words(flags)?);
                words.extend(["--timeout-ms".to_string(), poll_ms]);
                match self.command(&words) {
                    Ok(v) => {
                        let inside = v.as_str().unwrap_or("").to_string();
                        (inside.contains(&t), inside)
                    }
                    Err(err) => (false, err),
                }
            }
            Subject::Text { text: t, within: None } => {
                let t = scope.word(t)?;
                if e.negate {
                    // Gone everywhere a user could read it: look now, again later.
                    let js = format!(
                        "(() => {{ const t = {}; const docs = [document]; for (const f of document.querySelectorAll('iframe,frame')) {{ try {{ if (f.contentDocument) docs.push(f.contentDocument); }} catch (_) {{}} }} return docs.some((d) => d.body && d.body.innerText.includes(t)); }})()",
                        serde_json::to_string(&t).unwrap_or_default()
                    );
                    let v = self.command(&["eval".into(), js])?;
                    let present = v.as_bool() == Some(true);
                    (present, if present { "present".into() } else { "absent".into() })
                } else {
                    // navigera's own text wait: frames included.
                    match self.command(&[
                        "wait".into(),
                        "--text".into(),
                        t,
                        "--timeout-ms".into(),
                        left.as_millis().max(1).to_string(),
                    ]) {
                        Ok(_) => (true, "present".into()),
                        Err(_) => (false, "absent".into()),
                    }
                }
            }
            Subject::Var(raw) => {
                let actual = scope.word(raw)?;
                (compare(&actual, e.matcher, value), actual)
            }
            Subject::Accessible { selector, tags, ignore } => {
                // navigera's `a11y`: axe-core in a world of its own in the
                // page, loaded once per document.
                let mut w = vec!["a11y".to_string(), "--tags".into(), tags.join(",")];
                if !ignore.is_empty() {
                    w.extend(["--ignore".to_string(), ignore.join(",")]);
                }
                if let Some(sel) = selector {
                    w.extend(["--selector".to_string(), scope.word(sel)?]);
                }
                match self.command(&w) {
                    Ok(r) => {
                        let rules = r["rules"].as_array().cloned().unwrap_or_default();
                        if rules.is_empty() {
                            (true, "no violations".into())
                        } else {
                            let lines: Vec<String> = rules
                                .iter()
                                .map(|v| {
                                    let count = v["count"].as_u64().unwrap_or(0);
                                    let shown: Vec<&str> = v["nodes"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .take(3)
                                        .filter_map(|n| n["selector"].as_str())
                                        .collect();
                                    format!(
                                        "{} ({}, {count}: {}{}): {}",
                                        v["rule"].as_str().unwrap_or("?"),
                                        v["impact"].as_str().unwrap_or("?"),
                                        shown.join(", "),
                                        if count > 3 { ", …" } else { "" },
                                        v["help"].as_str().unwrap_or("")
                                    )
                                })
                                .collect();
                            (false, format!("{} violation(s): {}", lines.len(), lines.join("; ")))
                        }
                    }
                    Err(err) => (false, err),
                }
            }
            Subject::Metric(name) => {
                // navigera's `vitals`: every number as the browser measured
                // it, with the element behind LCP, CLS and INP.
                let v = self.command(&["vitals".into()])?;
                let behind = match name.as_str() {
                    "lcp" => v["lcp_element"].as_str(),
                    "inp" => v["inp_element"].as_str(),
                    "cls" => v["cls_elements"].get(0).and_then(Value::as_str),
                    _ => None,
                };
                match v[name.as_str()].as_f64() {
                    Some(n) => {
                        let actual = format!("{}", (n * 1000.0).round() / 1000.0);
                        let held = compare(&actual, e.matcher, value);
                        match behind {
                            Some(el) if held == e.negate => (held, format!("{actual} ({el})")),
                            _ => (held, actual),
                        }
                    }
                    // Every interaction was quicker than the browser keeps.
                    None if name == "inp" && v["inp_under_ms"].is_number() => {
                        let under = v["inp_under_ms"].as_f64().unwrap_or(104.0);
                        let limit: f64 = value.parse().unwrap_or(0.0);
                        if limit < under {
                            return Err(format!(
                                "expect inp: every interaction took under {under} ms, and the browser keeps none that \
                                 quick, so a budget under {under} ms can't be checked"
                            ));
                        }
                        (matches!(e.matcher, Matcher::Lt | Matcher::Le), format!("under {under}"))
                    }
                    None if name == "inp" => (false, "no interaction yet".into()),
                    None => (false, "not measured yet".into()),
                }
            }
            Subject::Js(_) | Subject::JsFile(_) | Subject::JsMain(_) => {
                // The step's own time bounds the script, not navigera's 35 s.
                let limit_ms = e.timeout_ms.unwrap_or(self.opts.expect_ms);
                let eval_ms = (left.as_millis() as u64).clamp(50, limit_ms.max(50)).to_string();
                let secs = format!("{}", (limit_ms as f64 / 1000.0 * 10.0).round() / 10.0);
                let what = match &e.subject {
                    Subject::JsFile(f) => {
                        let f = scope.word(f)?;
                        Path::new(&f).file_name().map_or(f.clone(), |n| n.to_string_lossy().into_owned())
                    }
                    _ => "the script".to_string(),
                };
                let ran = match &e.subject {
                    Subject::JsFile(f) => {
                        let f = scope.word(f)?;
                        // Waiting won't make it appear: say so now.
                        let src = std::fs::read_to_string(&f).map_err(|_| {
                            format!("expect js --file: no file {f} (a relative name is found next to the .bt)")
                        })?;
                        // An expression, or a function body that `return`s
                        // its answer: tried as the first, then run as the
                        // second. A syntax error in both forms won't go away
                        // by waiting either.
                        let tm = ["--timeout-ms".to_string(), eval_ms.clone()];
                        let mut first = vec!["eval".to_string(), src.clone()];
                        first.extend(tm.clone());
                        match self.command(&first) {
                            Err(e) if e.contains("SyntaxError") => {
                                let body = format!("(async () => {{\n{src}\n}})()");
                                let mut second = vec!["eval".to_string(), body];
                                second.extend(tm);
                                match self.command(&second) {
                                    Err(e2) if e2.contains("SyntaxError") => {
                                        return Err(format!(
                                            "expect js --file {f}: {} (the file is read as an expression, \
                                             then as a function body that returns its answer)",
                                            e2.trim_start_matches("eval: ")
                                        ))
                                    }
                                    other => other,
                                }
                            }
                            other => other,
                        }
                    }
                    Subject::Js(code) => {
                        let code = scope.word(code)?;
                        self.command(&["eval".into(), code, "--timeout-ms".into(), eval_ms.clone()])
                    }
                    Subject::JsMain(code) => {
                        let code = scope.word(code)?;
                        self.command(&["eval".into(), "--main".into(), code, "--timeout-ms".into(), eval_ms.clone()])
                    }
                    _ => unreachable!("matched above"),
                };
                match ran {
                    Ok(v) => {
                        let actual = match &v {
                            Value::String(s) => s.clone(),
                            Value::Null => "null".into(),
                            other => other.to_string(),
                        };
                        let holds = if e.matcher == Matcher::Holds {
                            !matches!(&v, Value::Null | Value::Bool(false))
                                && v != serde_json::json!(0)
                                && v != serde_json::json!("")
                        } else {
                            compare(&actual, e.matcher, value)
                        };
                        (holds, actual)
                    }
                    // Waiting longer won't settle it: the step's time is up.
                    // In this navigera's words or an older one's.
                    Err(err) if err.contains("did not answer within") || err.contains("command timed out") => {
                        return Err(format!("{what} did not finish within {secs} s; a Promise that never settles?"))
                    }
                    Err(err) => (false, err),
                }
            }
            Subject::Request { method, url, status, read } => {
                let url = scope.word(url)?;
                let v = self.command(&["network".into(), "--limit".into(), "1000".into()])?;
                // `#3 POST 200 Fetch /api/cart 196 B 3 ms`; another origin's
                // URL in full, matched by its path too.
                let mut seen = Vec::new();
                let mut hits: Vec<String> = Vec::new();
                for l in v.as_str().unwrap_or("").lines() {
                    let w: Vec<&str> = l.split_whitespace().collect();
                    if w.len() < 5 {
                        continue;
                    }
                    let (n, m, st, u) = (w[0], w[1], w[2], w[4]);
                    let path = u.split_once("://").and_then(|(_, r)| r.find('/').map(|i| &r[i..])).unwrap_or(u);
                    if crate::expect::glob(&url, u) || crate::expect::glob(&url, path) {
                        seen.push(format!("{m} {u} {st}"));
                        if (method == "*" || m.eq_ignore_ascii_case(method))
                            && status.as_deref().is_none_or(|s| s == st)
                        {
                            hits.push(n.trim_start_matches('#').to_string());
                        }
                    }
                }
                let none = || if seen.is_empty() { format!("no request to {url}") } else { seen.join(", ") };
                match read {
                    RequestRead::Made => (!hits.is_empty(), none()),
                    RequestRead::Count => {
                        let n = hits.len().to_string();
                        (compare(&n, e.matcher, value), format!("{n} ({})", none()))
                    }
                    RequestRead::Body => match hits.last() {
                        None => (false, none()),
                        Some(n) => {
                            let body = self.response_body(n);
                            let cut: String = body.chars().take(200).collect();
                            (compare(&body, e.matcher, value), cut)
                        }
                    },
                    RequestRead::Json(path) => match hits.last() {
                        None => (false, none()),
                        Some(n) => {
                            let body = self.response_body(n);
                            let steps = crate::expect::json_path(path)?;
                            match serde_json::from_str::<Value>(&body) {
                                Ok(json) => match crate::expect::json_at(&json, &steps) {
                                    Some(actual) => (compare(&actual, e.matcher, value), actual),
                                    None => (false, format!("no {path} in the response")),
                                },
                                Err(_) => {
                                    let cut: String = body.chars().take(120).collect();
                                    (false, format!("the response isn't JSON: {cut:?}"))
                                }
                            }
                        }
                    },
                }
            }
            Subject::DownloadCompare { glob, expected, flags, report } => {
                match self.download_done(&scope.word(glob)?)? {
                    (true, path) => {
                        let expected = scope.word(expected)?;
                        let flags = scope.words(flags)?;
                        let report = report.as_ref().unwrap_or(&self.opts.compare_report).clone();
                        self.compara(&path, &expected, &flags, &report)?;
                        (true, path)
                    }
                    seen => seen,
                }
            }
            Subject::Download(glob) => self.download_done(&scope.word(glob)?)?,
            Subject::Console => {
                let v = self.command(&["console".into(), "--json".into()])?;
                let texts: Vec<String> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.get("text").and_then(Value::as_str).map(str::to_string))
                    .collect();
                let hit = texts.iter().any(|t| compare(t, e.matcher, value));
                (hit, if texts.is_empty() { "no console messages".into() } else { texts.join(" | ") })
            }
            Subject::Cookie(name) if self.bare => {
                let name = scope.word(name)?;
                match self.jar.get(&name, crate::http::unix_now()) {
                    Some(c) => (compare(&c.value, e.matcher, value), c.value.clone()),
                    None => (false, "no such cookie".into()),
                }
            }
            Subject::Cookie(name) => {
                let name = scope.word(name)?;
                let v = self.command(&["cookies".into(), "--all".into(), "--json".into()])?;
                let found = v.as_array().and_then(|a| {
                    a.iter().find(|c| c.get("name").and_then(Value::as_str) == Some(name.as_str())).cloned()
                });
                match found {
                    Some(c) => {
                        let val = c.get("value").and_then(Value::as_str).unwrap_or("").to_string();
                        (compare(&val, e.matcher, value), val)
                    }
                    None => (false, "no such cookie".into()),
                }
            }
            Subject::Storage { area, key } => {
                let key = scope.word(key)?;
                match self.command(&["storage".into(), area.clone(), "get".into(), key]) {
                    Ok(Value::String(val)) => (compare(&val, e.matcher, value), val),
                    Ok(_) => (false, "no such key".into()),
                    Err(err) => (false, err),
                }
            }
            Subject::Ax { flags, tree } => {
                // The whole tree (or the target's): `--limit 0` cuts nothing.
                let mut words = vec!["ax".to_string(), "--limit".into(), "0".into()];
                words.extend(scope.words(flags)?);
                let template = crate::axtree::compile(&scope.words(tree)?)?;
                match self.command(&words) {
                    Ok(Value::String(page)) => match crate::axtree::matches(&template, &page) {
                        Ok(()) => (true, "it matches".into()),
                        Err(miss) => (false, miss),
                    },
                    Ok(other) => (false, other.to_string()),
                    Err(err) => (false, err),
                }
            }
            Subject::Screenshot(shot) => self.screenshot(shot, scope, left)?,
            Subject::Http(read) => match http_read(read, scope)? {
                Some(actual) if e.matcher == Matcher::Holds => (actual != "null", actual),
                Some(actual) if matches!(read, HttpRead::Json(_)) => {
                    (crate::expect::compare_json(&actual, e.matcher, value), actual)
                }
                Some(actual) => (compare(&actual, e.matcher, value), actual),
                None => (false, "absent".into()),
            },
            Subject::JsonSnapshot { path, name, ignore } => self.json_snapshot(path, name, ignore, scope)?,
            Subject::JsonSchema { path, file } => {
                let body = scope.saved.get("http.body").ok_or("no `http` answer yet: an `http` step goes first")?;
                let doc: Value = serde_json::from_str(body).map_err(|e| format!("the answer isn't JSON ({e})"))?;
                let at = crate::expect::json_value_at(&doc, &crate::expect::json_path(&scope.word(path)?)?);
                let file = scope.word(file)?;
                // `openapi.json#/components/schemas/Item`: a schema inside a
                // larger document, its `$ref`s read against the whole of it.
                let (file, pointer) = file.split_once('#').map_or((file.as_str(), None), |(f, p)| (f, Some(p)));
                let text = std::fs::read_to_string(file).map_err(|e| format!("schema {file}: {e}"))?;
                let root: Value = serde_json::from_str(&text).map_err(|e| format!("schema {file} isn't JSON ({e})"))?;
                let schema = match pointer {
                    Some(p) => {
                        root.pointer(&p.replace("%25", "%")).ok_or_else(|| format!("schema {file}: nothing at #{p}"))?
                    }
                    None => &root,
                };
                match at {
                    None => (false, format!("nothing at {path}")),
                    Some(v) => match crate::schema::validate_in(&root, schema, v) {
                        Ok(()) => (true, "valid".into()),
                        Err(errors) => (false, errors.join("; ")),
                    },
                }
            }
            Subject::Errors => {
                let v = self.command(&["errors".into(), "--json".into()])?;
                let list = v.as_array().cloned().unwrap_or_default();
                (
                    list.is_empty(),
                    if list.is_empty() { "none".into() } else { serde_json::to_string(&list).unwrap_or_default() },
                )
            }
            Subject::Target { flags, prop } => {
                let flags = scope.words(flags)?;
                match prop {
                    Prop::State(state) if state == "empty" => {
                        // No text, and no value when it takes one.
                        let read = |r: &mut Self, what: &str| -> Result<String, String> {
                            let mut words = vec!["get".to_string(), what.to_string()];
                            words.extend(flags.clone());
                            words.extend(["--timeout-ms".to_string(), poll_ms.clone()]);
                            r.command(&words).map(|v| match v {
                                Value::String(s) => s,
                                Value::Null => String::new(),
                                other => other.to_string(),
                            })
                        };
                        match read(self, "text") {
                            Ok(text) => {
                                let value = read(self, "value").unwrap_or_default();
                                let empty = text.trim().is_empty() && value.is_empty();
                                let seen = if empty {
                                    "empty".to_string()
                                } else {
                                    format!("{:?}", if value.is_empty() { text.trim() } else { &value })
                                };
                                (empty, seen)
                            }
                            Err(err) => (false, err),
                        }
                    }
                    Prop::State(state) => {
                        let (asked, flip) =
                            if state == "unchecked" { ("checked", true) } else { (state.as_str(), false) };
                        let mut words = vec!["is".to_string(), asked.to_string()];
                        words.extend(flags);
                        words.extend(["--timeout-ms".to_string(), poll_ms]);
                        match self.command(&words) {
                            Ok(v) => {
                                let b = (v.as_bool() == Some(true)) != flip;
                                (b, if b { state.clone() } else { format!("not {state}") })
                            }
                            Err(err) => (false, err),
                        }
                    }
                    Prop::Count => {
                        let css = flags.get(1).cloned().unwrap_or_default();
                        let v = self.command(&["get".into(), "count".into(), css])?;
                        let actual = v.to_string();
                        (compare(&actual, e.matcher, value), actual)
                    }
                    Prop::Text | Prop::Value | Prop::Attr(_) | Prop::Style(_) => {
                        let mut words = vec!["get".to_string()];
                        match prop {
                            Prop::Text => words.push("text".into()),
                            Prop::Value => words.push("value".into()),
                            Prop::Attr(name) => words.extend(["attr".to_string(), name.clone()]),
                            Prop::Style(name) => words.extend(["style".to_string(), name.clone()]),
                            _ => unreachable!(),
                        }
                        words.extend(flags);
                        words.extend(["--timeout-ms".to_string(), poll_ms]);
                        match self.command(&words) {
                            Ok(v) => {
                                let actual = match v {
                                    Value::String(s) => s.trim().to_string(),
                                    Value::Null => String::new(),
                                    other => other.to_string(),
                                };
                                (compare(&actual, e.matcher, value), actual)
                            }
                            Err(err) => (false, err),
                        }
                    }
                }
            }
        })
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(d) = self.driver.as_mut() {
            d.session().close();
        }
    }
}

/// The expect loop's poll between reads.
const POLL: Duration = Duration::from_millis(25);

/// The page-side helpers an `expect` wake wait's predicate resolves and
/// reads elements with: the same deep search navigera's own reads use
/// (the documents of the same-origin frame tree, plus open shadow roots;
/// first match in that order), and its `is` rules (visible, disabled,
/// checked, focused). The `__bt` prefix keeps clear of navigera's own
/// `__nv` names.
const WAKE_DEEP: &str = r#"const __btDocs = (d, out) => { out.push(d); for (const f of d.querySelectorAll('iframe, frame')) { let cd = null; try { cd = f.contentDocument; } catch (_) {} if (cd && cd.documentElement) __btDocs(cd, out); } return out; };
const __btShadows = (root, out) => { const tw = (root.ownerDocument || root).createTreeWalker(root, NodeFilter.SHOW_ELEMENT); for (let n = tw.nextNode(); n; n = tw.nextNode()) { if (n.shadowRoot) { out.push(n.shadowRoot); __btShadows(n.shadowRoot, out); } } return out; };
const __btRoots = () => { const out = []; for (const d of __btDocs(document, [])) { out.push(d); __btShadows(d, out); } return out; };
const __btFirst = (sel) => { for (const r of __btRoots()) { const el = r.querySelector(sel); if (el) return el; } return null; };
const __btAll = (sel) => __btRoots().flatMap((r) => Array.from(r.querySelectorAll(sel)));
const __btVisible = (el) => !!el && el.getClientRects().length > 0 && (el.ownerDocument.defaultView || window).getComputedStyle(el).visibility !== 'hidden';
const __btDisabled = (el) => !!el && !!(el.disabled || el.closest('fieldset[disabled]') || el.getAttribute('aria-disabled') === 'true');
const __btEnabled = (el) => !!el && !__btDisabled(el);
const __btChecked = (el) => !!el && !!(el.checked || el.getAttribute('aria-checked') === 'true');
const __btFocused = (el) => { if (!el) return false; const a = el.getRootNode().activeElement || el.ownerDocument.activeElement; return el === a || el.contains(a); };
"#;

/// `compare()` mirrored for a wake wait's predicate: the matcher word is
/// the switch key, strings compare as bitti's trimmed reads do,
/// numbers as its `parse::<f64>` (an empty or non-numeric side is not a
/// number). Anything the mirror cannot say (a pattern JS rejects) throws,
/// and the predicate's own catch wakes early instead of blocking a pass.
const WAKE_CMP: &str = r#"const __btNum = (s) => { const t = String(s).trim(); if (t === '') return null; const n = Number(t); return Number.isNaN(n) ? null : n; };
const __btCmp = (actual, m, expected) => { actual = String(actual); switch (m) {
  case 'is': return actual === expected || (__btNum(actual) !== null && __btNum(expected) !== null && __btNum(actual) === __btNum(expected));
  case 'contains': return actual.includes(expected);
  case 'starts-with': return actual.startsWith(expected);
  case 'ends-with': return actual.endsWith(expected);
  case 'matches': return new RegExp(expected).test(actual);
  case '>': case '>=': case '<': case '<=': {
    const a = __btNum(actual), b = __btNum(expected);
    if (a === null || b === null) return false;
    if (m === '>') return a > b; if (m === '>=') return a >= b; if (m === '<') return a < b; return a <= b;
  }
  default: return false;
} };
"#;

/// `expect screenshot`'s page-quiet clock: from document start in every
/// tab, the time of the page's own last DOM change. bitti's freeze
/// style and mask divs (`__bt_freeze`, `__bt_mask`) do not count. It
/// installs once a document (the init script runs it first; a direct
/// eval is the fallback for pages that predate the registration).
const QUIET_CLOCK: &str = r#"(() => {
  if (window.__btQuiet !== undefined) return;
  const mark = () => { window.__btQuiet = performance.now(); };
  mark();
  const own = (n) => n.nodeType === 1 && (n.id === '__bt_freeze' || (n.classList && n.classList.contains('__bt_mask')));
  new MutationObserver((ms) => {
    for (const m of ms) {
      if (m.type === 'childList') {
        const ns = [...m.addedNodes, ...m.removedNodes];
        if (ns.length && ns.every(own)) continue;
      }
      mark();
      return;
    }
  }).observe(document, { childList: true, subtree: true, attributes: true, characterData: true });
})()"#;

/// What to click in the element a click hit instead of its own (by
/// navigera's description's selector): the buttons and links of the
/// fixed or sticky box around it, else of an open dialog; the ones that
/// say "Accept", "Close", "Got it" … first.
const UNCOVER_JS: &str = r#"(sel) => {
  let hit = null;
  try { hit = document.querySelector(sel); } catch (_) {}
  if (!hit) return [];
  const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const shown = (e) => { const b = e.getBoundingClientRect(); return b.width > 0 && b.height > 0 && getComputedStyle(e).visibility !== 'hidden'; };
  let top = hit;
  for (let p = hit; p && p !== document.body; p = p.parentElement) if (/fixed|sticky/.test(getComputedStyle(p).position)) top = p;
  const CLICKY = 'button,[role=button],a[href],input[type=button],input[type=submit]';
  let els = [...(top.matches(CLICKY) ? [top] : []), ...top.querySelectorAll(CLICKY)].filter(shown);
  // A backdrop holds nothing: the dialog beside it does.
  if (!els.length) els = [...document.querySelectorAll('dialog[open],[role=dialog],[role=alertdialog],[aria-modal=true]')]
    .flatMap((d) => [...d.querySelectorAll(CLICKY)]).filter(shown);
  const AWAY = /^(accept( all)?|(i )?agree|allow( all)?|ok(ay)?|got it|close|dismiss|continue|no,? thanks|reject( all)?|decline|skip|later|not now|×|✕|x)$/i;
  const seen = new Set();
  return els.map((e) => ({ role: e.tagName === 'A' ? 'link' : 'button', name: norm(e.getAttribute('aria-label') || e.innerText || e.value || e.title) }))
    .filter((c) => c.name && c.name.length <= 60 && !seen.has(c.name) && seen.add(c.name))
    .map((c) => ({ ...c, p: AWAY.test(c.name) ? 0.9 : 0.5 }))
    .sort((a, b) => b.p - a.p).slice(0, 3);
}"#;

/// `@serial` (or `@exclusive`): the scenario runs while no other does.
pub fn is_serial(tags: &[String]) -> bool {
    tags.iter().any(|t| t == "serial" || t == "exclusive")
}

pub(crate) fn parse(words: &[String]) -> Result<Command, String> {
    let parsed =
        parse_args(std::iter::once("navigera".to_string()).chain(words.iter().cloned())).map_err(|e| match e {
            navigera::protocol::ArgsError::Help(_) => format!("`{}` needs more", words.join(" ")),
            navigera::protocol::ArgsError::Invalid(m) => m.lines().next().unwrap_or("").to_string(),
        })?;
    if parsed.start || parsed.mcp || parsed.local.is_some() {
        return Err(format!("`{}` isn't a browser step", words.first().map(String::as_str).unwrap_or("")));
    }
    parsed.command.ok_or_else(|| format!("`{}` isn't a browser step", words.join(" ")))
}

fn describe(e: &Expect, value: &str, actual: &str, limit: Duration) -> String {
    let not = if e.negate { "not " } else { "" };
    if let Subject::JsonSnapshot { path, name, .. } = &e.subject {
        return format!("expected json {path} {not}to match snapshot {name}: {actual} (--update-snapshots takes it as the baseline)");
    }
    if let Subject::JsonSchema { path, file } = &e.subject {
        let name = schema_name(file);
        return if e.negate {
            format!("expected json {path} not to match schema {name}; it does")
        } else {
            format!("expected json {path} to match schema {name}; it doesn't: {actual}")
        };
    }
    if let Subject::Screenshot(s) = &e.subject {
        return format!(
            "expected screenshot {} {not}to match its baseline within {} ms: {actual}",
            s.name,
            limit.as_millis()
        );
    }
    if let Subject::Accessible { selector, tags, .. } = &e.subject {
        let on = selector.as_ref().map(|s| format!(" under {s:?}")).unwrap_or_default();
        return format!(
            "expected the page{on} {not}to pass axe ({}) within {} ms; saw {actual}",
            tags.join(", "),
            limit.as_millis()
        );
    }
    if let Subject::Metric(m) = &e.subject {
        let unit = if m == "cls" { "" } else { " ms" };
        let seen = if actual.parse::<f64>().is_ok() { format!("{actual}{unit}") } else { actual.to_string() };
        return format!(
            "expected {not}{m} {} {value}{unit} within {} ms; saw {seen}",
            e.matcher.word(),
            limit.as_millis()
        );
    }
    if let Subject::Ax { flags, .. } = &e.subject {
        let on = if flags.is_empty() { String::new() } else { format!(" {}", flags.join(" ")) };
        return format!("expected ax{on} {not}to match the tree under it within {} ms: {actual}", limit.as_millis());
    }
    let what = match &e.subject {
        Subject::Url => "url".to_string(),
        Subject::Title => "title".to_string(),
        Subject::Text { text, within: None } => format!("text {text:?}"),
        Subject::Text { text, within: Some(w) } => format!("text {text:?} within {}", w.join(" ")),
        Subject::Download(g) => format!("a download {g:?}"),
        Subject::DownloadCompare { glob, expected, .. } => format!("a download {glob:?} the same as {expected}"),
        Subject::Console => "a console message".to_string(),
        Subject::Var(v) => v.clone(),
        Subject::Errors => "no page errors".to_string(),
        Subject::Js(c) => format!("js {c:?}"),
        Subject::JsMain(c) => format!("js --main {c:?}"),
        Subject::JsFile(f) => format!("js --file {f}"),
        Subject::Request { method, url, status, read } => {
            let read = match read {
                RequestRead::Made => String::new(),
                RequestRead::Json(p) => format!(" json {p}"),
                RequestRead::Body => " body".to_string(),
                RequestRead::Count => " count".to_string(),
            };
            format!(
                "request {method} {url}{}{read}",
                status.as_ref().map(|s| format!(" status {s}")).unwrap_or_default()
            )
        }
        Subject::Cookie(c) => format!("cookie {c}"),
        Subject::Storage { area, key } => format!("storage {area} {key}"),
        // Said above.
        Subject::Ax { .. } => String::new(),
        Subject::Screenshot(s) => format!("screenshot {}", s.name),
        Subject::Metric(m) => m.clone(),
        Subject::Accessible { .. } => "accessible".to_string(),
        Subject::Http(HttpRead::Json(p)) => format!("json {p}"),
        Subject::Http(HttpRead::Count(p)) => format!("json {p} count"),
        Subject::Http(HttpRead::Header(h)) => format!("header {h}"),
        Subject::JsonSchema { path, file } => {
            let name = schema_name(file);
            format!("json {path} to match schema {name}")
        }
        // Said above.
        Subject::JsonSnapshot { .. } => String::new(),
        Subject::Target { flags, prop } => {
            let p = match prop {
                Prop::State(s) => s.clone(),
                Prop::Text => "text".into(),
                Prop::Value => "value".into(),
                Prop::Attr(a) => format!("attr {a}"),
                Prop::Style(a) => format!("style {a}"),
                Prop::Count => "count".into(),
            };
            format!("{} {p}", flags.join(" "))
        }
    };
    let want = if e.matcher == Matcher::Holds { String::new() } else { format!(" {} {value:?}", e.matcher.word()) };
    // A long value is cut, and says how much more there is and how to see
    // it whole: paging through a page with failing expects costs a run each.
    let (seen, more) = if actual.len() > 300 {
        let cut = actual.floor_char_boundary(300);
        (format!("{}…", &actual[..cut]), actual[cut..].chars().count())
    } else {
        (actual.to_string(), 0)
    };
    // Text looked for on the page has no value to show: "saw \"absent\""
    // would read as if the page showed the word.
    let seen = match (&e.subject, seen.as_str()) {
        (Subject::Text { .. }, "absent") => "it isn't on the page".to_string(),
        (Subject::Text { .. }, "present") => "it is still on the page".to_string(),
        _ if more > 0 => format!("saw {seen:?} ({more} more characters; a `print` step shows a value whole)"),
        _ => format!("saw {seen:?}"),
    };
    // An answer that is read once (`http`, `exec`) isn't waited for.
    if limit.is_zero() {
        return format!("expected {not}{what}{want}; {seen}");
    }
    format!("expected {not}{what}{want} within {} ms; {seen}", limit.as_millis())
}

/// How a `print` step's output starts in a scenario's notes, which the
/// console shows (other notes, optional steps that didn't happen, it
/// doesn't).
pub const PRINTED: &str = "print at line ";

/// The most a `print` shows: a page's tree is long, and what the test
/// needs is usually near its top.
const PRINT_MAX: usize = 6000;

/// `text` cut to `max` bytes at a character boundary, saying how much was
/// left out.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let cut = text.floor_char_boundary(max);
    format!(
        "{}… ({} more bytes; print a part: `--selector`, `--limit`, or a smaller `eval`)",
        &text[..cut],
        text.len() - cut
    )
}

/// Why the browser didn't start, in words that say what to do. navigera's
/// text leads with its launch flags (`engine=chrome, headed=false`) and, for a
/// missing screen, three lines of Chrome's log with the reason in the middle.
pub fn browser_failed(e: &str) -> String {
    let rest = match e.find("): ") {
        Some(at) if e.starts_with("browser launch failed (") => &e[at + 3..],
        _ => e,
    };
    if e.contains("Missing X server or $DISPLAY") {
        return "the browser didn't start: it was to open a window (--headed, or `record`) and there is no \
                screen here ($DISPLAY isn't set). Run headless, or on a machine with a display"
            .into();
    }
    if let Some(at) = rest.find("spawn ").filter(|_| rest.contains("No such file or directory")) {
        let path = rest[at + 6..].split(" --").next().unwrap_or("").trim();
        return format!(
            "the browser didn't start: there is no browser at {path}. Point --chromium or CHROME_BIN at \
             Chrome, Chromium or Edge, or run `bitti navigera install-browser`"
        );
    }
    let rest = rest.replace("`navigera install-browser`", "`bitti navigera install-browser`");
    format!("the browser didn't start: {rest}")
}

/// navigera's errors end with advice for its own loop (read `ax`, act on a
/// ref), which a `.bt` file can't follow: refs change between runs and are
/// refused. Here the screenshot and a locator are the way.
pub fn bt_terms(message: &str) -> String {
    let mut out = message.to_string();
    for (from, to) in [
        (
            "(a word that is not a number is read as CSS; refs are the numbers in `ax`, as in `click 12`, and --text matches visible text)",
            "(a bare word is read as CSS; for what the page shows, quote it: click \"Buy now\", or use --text)",
        ),
        ("; run `ax` to see the page)", "; the screenshot shows the page)"),
    ] {
        out = out.replace(from, to);
    }
    if out.contains("ERR_CONNECTION_REFUSED") && !out.contains("[server]") {
        out.push_str(" (a [server] in bitti.toml can start the app before the run)");
    }
    // Any other `(… ax …)` aside, in this navigera's words or an older one's.
    while let Some(at) = out.find("`ax`") {
        let (Some(open), Some(close)) = (out[..at].rfind('('), out[at..].find(')').map(|c| at + c)) else { break };
        out.replace_range(
            open..=close,
            "(the screenshot shows the page; a locator such as --role button --name \"…\" or --label may find it)",
        );
    }
    out
}

/// Run a program (no shell) in `dir`, reading its output as it comes, and
/// stop it at `limit`: its exit code (-1 when a signal ended it), stdout and
/// stderr (the first 16 MB of each). Something it left running that holds the
/// output open (a server started in the background) doesn't hold the run.
fn run_program(
    argv: &[String],
    dir: &Path,
    limit: Duration,
    base: Option<&str>,
) -> Result<(i32, String, String), String> {
    use std::io::Read;
    use std::process::Stdio;
    const CAP: usize = 16 << 20;
    let (program, args) = argv.split_first().ok_or("exec needs a program")?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if !dir.as_os_str().is_empty() {
        cmd.current_dir(dir);
    }
    if let Some(b) = base {
        cmd.env("BITTI_BASE_URL", b);
    }
    let mut child = cmd.spawn().map_err(|e| format!("exec {program}: {e}"))?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = buf.clone();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                let mut b = sink.lock().unwrap_or_else(|e| e.into_inner());
                let room = CAP.saturating_sub(b.len());
                b.extend_from_slice(&chunk[..n.min(room)]);
            }
        });
        (buf, reader)
    };
    let (out, out_reader) = read(Box::new(child.stdout.take().expect("piped")));
    let (err, err_reader) = read(Box::new(child.stderr.take().expect("piped")));
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| format!("exec {program}: {e}"))? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("`{}` still ran after {} ms; stopped it", shown(argv), limit.as_millis()));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let grace = Instant::now() + Duration::from_millis(500);
    while !(out_reader.is_finished() && err_reader.is_finished()) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(5));
    }
    let text =
        |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
    Ok((status.code().unwrap_or(-1), text(&out), text(&err)))
}

/// A value of the last `http` answer: in its JSON (`$.items[0].sku`), or a
/// header; `None` when it isn't there.
fn http_read(read: &HttpRead, scope: &Scope) -> Result<Option<String>, String> {
    let body = scope.saved.get("http.body").ok_or("no `http` answer yet: an `http` step goes first")?;
    Ok(match read {
        HttpRead::Json(path) => {
            let path = crate::expect::json_path(&scope.word(path)?)?;
            let v: Value = serde_json::from_str(body).map_err(|e| format!("the answer isn't JSON ({e})"))?;
            crate::expect::json_at(&v, &path)
        }
        HttpRead::Count(path) => {
            let p = crate::expect::json_path(&scope.word(path)?)?;
            let v: Value = serde_json::from_str(body).map_err(|e| format!("the answer isn't JSON ({e})"))?;
            match crate::expect::json_value_at(&v, &p) {
                Some(Value::Array(a)) => Some(a.len().to_string()),
                Some(Value::Object(o)) => Some(o.len().to_string()),
                Some(other) => return Err(format!("{path} is {other}, not an array or an object to count")),
                None => None,
            }
        }
        HttpRead::Header(name) => {
            let name = scope.word(name)?;
            let found: Vec<&str> = scope
                .http_headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(&name))
                .map(|(_, v)| v.as_str())
                .collect();
            (!found.is_empty()).then(|| found.join(", "))
        }
    })
}

/// `print http|body|json [<$.path>]|header <name>`: the last `http`
/// answer, or a part of it; JSON pretty-printed.
fn print_answer(words: &[String], scope: &Scope) -> Result<String, String> {
    let body = scope.saved.get("http.body").ok_or("no `http` answer yet: an `http` step goes first")?;
    let pretty = |text: &str| match serde_json::from_str::<Value>(text) {
        Ok(v) if text.trim_start().starts_with(['{', '[']) => serde_json::to_string_pretty(&v).unwrap_or_default(),
        _ => text.to_string(),
    };
    match words {
        [w] if w == "http" => {
            let status = scope.saved.get("http.status").cloned().unwrap_or_default();
            let mut out = format!("{} {status}\n", scope.exchanges.last().map(String::as_str).unwrap_or(""));
            for (k, v) in &scope.http_headers {
                out.push_str(&format!("{k}: {v}\n"));
            }
            out.push('\n');
            out.push_str(&pretty(body));
            Ok(out)
        }
        [w] if w == "body" => Ok(pretty(body)),
        [w] if w == "json" => Ok(pretty(body)),
        [w, path] if w == "json" => {
            let p = crate::expect::json_path(path)?;
            let v: Value = serde_json::from_str(body).map_err(|e| format!("the answer isn't JSON ({e})"))?;
            match crate::expect::json_value_at(&v, &p) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(v) => Ok(serde_json::to_string_pretty(v).unwrap_or_default()),
                None => Ok(format!("(nothing at {path})")),
            }
        }
        [w, name] if w == "header" => {
            Ok(http_read(&HttpRead::Header(name.clone()), scope)?.unwrap_or_else(|| format!("(no {name} header)")))
        }
        _ => Err("print http, print body, print json [<$.path>] or print header <name>".into()),
    }
}

/// Whether a step is about the last `http` answer: the request itself, or
/// a check or a save of what it said.
fn reads_answer(action: &Action) -> bool {
    match action {
        Action::Http { .. } | Action::Retry { .. } => true,
        Action::Save { command, .. } => matches!(command.first().map(String::as_str), Some("json" | "header")),
        Action::Expect(e) => match &e.subject {
            Subject::Http(_) | Subject::JsonSchema { .. } | Subject::JsonSnapshot { .. } => true,
            Subject::Var(v) => v.starts_with("${http."),
            _ => false,
        },
        Action::Optional(inner) => reads_answer(inner),
        _ => false,
    }
}

/// A command line as it could be typed again: words with spaces quoted.
fn shown(argv: &[String]) -> String {
    let word = |a: &String| if a.is_empty() || a.contains(char::is_whitespace) { format!("{a:?}") } else { a.clone() };
    argv.iter().map(word).collect::<Vec<_>>().join(" ")
}

/// A downloads directory nothing landed in, and the evidence directory
/// above it when that is empty too.
fn tidy(downloads: Option<PathBuf>) {
    if let Some(dir) = downloads {
        if std::fs::remove_dir(&dir).is_ok() {
            if let Some(parent) = dir.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

fn evidence_name(file: &Path, scenario: &str) -> String {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("scenario");
    let slug: String = scenario
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{stem}-{}", &slug[..slug.len().min(60)])
}

/// Variables a scenario sees: what it saved, the run's `--var`s, the
/// file's `set`s, then the environment.
struct Scope {
    /// Flow arguments, innermost last.
    layers: Vec<BTreeMap<String, String>>,
    saved: BTreeMap<String, String>,
    /// The scenario's data row.
    row: BTreeMap<String, String>,
    /// What the file's `before all` saved.
    before_all: BTreeMap<String, String>,
    run: BTreeMap<String, String>,
    file: BTreeMap<String, String>,
    /// `${scenario.name}`, `${run.id}`.
    name: String,
    run_id: String,
    secrets: BTreeMap<String, String>,
    /// The secrets' values used so far: masked in what the run prints.
    used_secrets: RefCell<BTreeSet<String>>,
    noop: Option<String>,
    /// Optional steps that didn't happen.
    notes: Vec<String>,
    /// `--soft` expectations that didn't hold, and why.
    soft: Vec<(Step, String)>,
    /// The last `http` answer's headers.
    http_headers: Vec<(String, String)>,
    /// The `.bt` file: screenshot baselines are beside it.
    bt: PathBuf,
    /// The step of a flow that failed, and its file (see `Failure::source`).
    inner: Option<(PathBuf, Step)>,
    /// An `optional` step just now didn't hold.
    optional_missed: bool,
    /// Every `http` exchange so far, one line each (`POST <url> -> 201 (12
    /// ms, 54 B, application/json)`): the trace's and a failure's.
    exchanges: Vec<String>,
    /// Inside a `retry`: a check looks once, and a `--soft` one fails.
    retrying: bool,
}

impl Scope {
    fn new(
        suite: &Suite,
        name: &str,
        row: BTreeMap<String, String>,
        opts: &Options,
        before_all: &BTreeMap<String, String>,
    ) -> Self {
        let mut scope = Scope {
            layers: Vec::new(),
            saved: BTreeMap::new(),
            row,
            before_all: before_all.clone(),
            run: opts.vars.clone(),
            file: suite.vars.clone(),
            name: name.to_string(),
            run_id: opts.run_id.clone(),
            secrets: opts.secrets.clone(),
            used_secrets: RefCell::new(BTreeSet::new()),
            noop: None,
            notes: Vec::new(),
            soft: Vec::new(),
            http_headers: Vec::new(),
            bt: suite.file.clone(),
            inner: None,
            optional_missed: false,
            exchanges: Vec::new(),
            retrying: false,
        };
        // A `set` value's own `${…}` (a default, `--var`, another `set`, a
        // secret, the environment) is filled in once, here; one that names
        // nothing known stays as written. Each value is interpolated once,
        // so `$$` stays a literal `$`.
        let raw = scope.file.clone();
        scope.file = raw.keys().map(|k| (k.clone(), fill_set(&scope, &raw, k, 0))).collect();
        scope
    }

    /// A flow's arguments, then what was saved, the data row, the file's
    /// `before all`, `--var`, the file's `set`s, the environment.
    fn lookup(&self, name: &str) -> Option<String> {
        if let Some(key) = name.strip_prefix("secret:") {
            let v = std::env::var(key).ok().or_else(|| self.secrets.get(key).cloned())?;
            self.used_secrets.borrow_mut().insert(v.clone());
            return Some(v);
        }
        // An authenticator app's code now, from its secret (masked like one).
        if let Some(key) = name.strip_prefix("totp:") {
            let v = std::env::var(key).ok().or_else(|| self.secrets.get(key).cloned())?;
            self.used_secrets.borrow_mut().insert(v.clone());
            return navigera::totp::Totp::parse(&v).ok().map(|t| t.now().0);
        }
        match name {
            "run.id" => return Some(self.run_id.clone()),
            "scenario.name" => return Some(self.name.clone()),
            _ => {}
        }
        self.layers
            .last()
            .and_then(|l| l.get(name))
            .or_else(|| self.saved.get(name))
            .or_else(|| self.row.get(name))
            .or_else(|| self.before_all.get(name))
            .or_else(|| self.run.get(name))
            .or_else(|| self.file.get(name))
            .cloned()
            .or_else(|| std::env::var(name).ok())
    }

    /// [`Scope::lookup`] without the file's `set`s and the environment.
    fn above_file(&self, name: &str) -> Option<String> {
        self.layers
            .last()
            .and_then(|l| l.get(name))
            .or_else(|| self.saved.get(name))
            .or_else(|| self.row.get(name))
            .or_else(|| self.before_all.get(name))
            .or_else(|| self.run.get(name))
            .cloned()
    }

    /// Replaces the secrets' values in a text with `***`.
    fn masker(&self) -> impl Fn(&str) -> String {
        let values = secret_forms(self.used_secrets.borrow().iter());
        move |text: &str| values.iter().fold(text.to_string(), |t, v| t.replace(v.as_str(), "***"))
    }

    fn word(&self, w: &str) -> Result<String, String> {
        interpolate(w, &|n| self.lookup(n))
    }

    fn words(&self, ws: &[String]) -> Result<Vec<String>, String> {
        ws.iter().map(|w| self.word(w)).collect()
    }
}

/// Every way a secret's value can stand in what a run writes, longest
/// first: as it is, as a JSON string holds it (the trace is JSON lines: a
/// `"` or `\` in a password is written escaped there), and as a URL or a
/// form body holds it (percent-encoded, a space as `+` or `%20`). Values
/// under three characters are left: masking them would blank ordinary text.
fn secret_forms<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut forms = BTreeSet::new();
    for v in values.filter(|v| v.len() >= 3) {
        let json = serde_json::Value::String(v.clone()).to_string();
        let url = |space: &str| {
            v.bytes().fold(String::new(), |mut out, b| {
                match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
                    b' ' => out.push_str(space),
                    _ => out.push_str(&format!("%{b:02X}")),
                }
                out
            })
        };
        forms.extend([v.clone(), json[1..json.len() - 1].to_string(), url("+"), url("%20")]);
    }
    let mut forms: Vec<String> = forms.into_iter().collect();
    forms.sort_by_key(|v| std::cmp::Reverse(v.len()));
    forms
}

/// A `set` value with its `${…}` filled in: another `set` by its own
/// filled-in value (unless something above the file names it), anything
/// else as a scenario would see it.
fn fill_set(scope: &Scope, raw: &BTreeMap<String, String>, key: &str, depth: u32) -> String {
    let value = &raw[key];
    if !value.contains('$') || depth > 8 {
        return value.clone();
    }
    interpolate(value, &|n| {
        if n == key {
            None
        } else if raw.contains_key(n) && scope.above_file(n).is_none() {
            Some(fill_set(scope, raw, n, depth + 1))
        } else {
            scope.lookup(n)
        }
    })
    .unwrap_or_else(|_| value.clone())
}

/// A failure as the run prints and saves it: secrets used in the scenario
/// replaced by `***` wherever they could show (the message, the page's URL
/// and title, what the page offered instead).
fn mask_failure(f: &mut Failure, mask: &impl Fn(&str) -> String) {
    f.message = mask(&f.message);
    f.text = mask(&f.text);
    if let Some((url, title)) = f.at.as_mut() {
        *url = mask(url);
        *title = mask(title);
    }
    for (what, locator, _) in f.nearest.iter_mut() {
        *what = mask(what);
        *locator = mask(locator);
    }
    if let Some(hint) = f.hint.as_mut() {
        *hint = mask(hint);
    }
}

/// Lines typed on stdin, read by one thread for the whole run (pauses and
/// handoffs wait on it).
fn stdin_lines() -> &'static Mutex<mpsc::Receiver<Option<String>>> {
    static LINES: OnceLock<Mutex<mpsc::Receiver<Option<String>>>> = OnceLock::new();
    LINES.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(None);
                    return;
                }
                Ok(_) => {
                    if tx.send(Some(line)).is_err() {
                        return;
                    }
                }
            }
        });
        Mutex::new(rx)
    })
}

/// Waits for Enter, at most `wait` (none: as long as it takes): whether it
/// came; `Err` when stdin is closed.
fn enter(wait: Option<Duration>) -> Result<bool, ()> {
    let rx = stdin_lines().lock().map_err(|_| ())?;
    let got = match wait {
        Some(d) => match rx.recv_timeout(d) {
            Ok(v) => Some(v),
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
        },
        None => rx.recv().ok(),
    };
    match got {
        Some(Some(_)) => Ok(true),
        _ => Err(()),
    }
}

/// Running under CI: `CI` set to anything but empty, `false` or `0`.
pub fn under_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| !matches!(v.trim(), "" | "false" | "0"))
}

/// A schema as a failure names it: the file's name, and the part of it
/// after a `#`.
fn schema_name(file: &str) -> String {
    let (f, part) = file.split_once('#').map_or((file, None), |(f, p)| (f, Some(p)));
    let name = Path::new(f).file_name().map_or(f.to_string(), |n| n.to_string_lossy().into_owned());
    match part {
        Some(p) => format!("{name}#{p}"),
        None => name,
    }
}

/// Seconds, as a person reads a limit: `30`, `1.5`, `0.15`. Exact to the
/// millisecond the limit was given in: one decimal made 150 ms `0.1` and
/// 40 ms `0.0`.
pub(crate) fn secs(d: Duration) -> String {
    let ms = d.as_millis();
    let whole = ms / 1000;
    match ms % 1000 {
        0 => whole.to_string(),
        part => format!("{whole}.{}", format!("{part:03}").trim_end_matches('0')),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A password with a quote, a backslash or a space is written
    /// differently in the trace's JSON and in a URL: each form is masked.
    #[test]
    fn a_secret_is_masked_as_json_and_urls_write_it() {
        let secret = r#"p"ss\w rd+1"#.to_string();
        let forms = secret_forms([&secret, &"ab".to_string()].into_iter());
        let mask = |text: &str| forms.iter().fold(text.to_string(), |t, v| t.replace(v.as_str(), "***"));
        assert_eq!(mask(&serde_json::json!({ "value": secret }).to_string()), r#"{"value":"***"}"#);
        assert_eq!(mask("https://h/?t=p%22ss%5Cw+rd%2B1&x=1"), "https://h/?t=***&x=1");
        assert_eq!(mask("https://h/p%22ss%5Cw%20rd%2B1"), "https://h/***");
        assert_eq!(mask(&format!("typed {secret} into ab")), "typed *** into ab");
    }

    #[test]
    fn a_limit_is_printed_to_the_millisecond_it_was_given_in() {
        let secs = |ms| super::secs(std::time::Duration::from_millis(ms));
        assert_eq!(secs(30_000), "30");
        assert_eq!(secs(1_500), "1.5");
        assert_eq!(secs(1_000), "1");
        assert_eq!(secs(150), "0.15");
        assert_eq!(secs(40), "0.04");
        assert_eq!(secs(1_005), "1.005");
    }

    #[test]
    fn a_long_value_says_how_much_was_cut_and_how_to_see_it() {
        let words: Vec<String> = ["js", "document.body.innerText", "is", "x"].iter().map(|w| w.to_string()).collect();
        let e = crate::expect::parse(&words).unwrap();
        let m = describe(&e, "x", &"é".repeat(400), Duration::from_millis(5000));
        assert!(m.contains("…\" (250 more characters; a `print` step shows a value whole)"), "{m}");
        assert!(!describe(&e, "x", "short", Duration::from_millis(5000)).contains("more characters"));
    }

    #[test]
    fn navigera_advice_is_put_in_a_bt_files_terms() {
        let m = bt_terms("click text \"Buy now\": no element with text \"Buy now\" within 600 ms (run `ax` to see the page and act on a ref from it)");
        assert!(!m.contains("`ax`") && m.contains("screenshot") && m.contains("--role button"), "{m}");
        let old = bt_terms(
            "click: no element matches selector \"Buy now\" within 1500 ms (run `ax` to see the page and act by --ref)",
        );
        assert!(!old.contains("--ref") && !old.contains("`ax`"), "{old}");
        let refused = bt_terms("goto http://127.0.0.1:1/: net::ERR_CONNECTION_REFUSED");
        assert!(refused.contains("[server]"), "{refused}");
    }

    #[test]
    fn a_browser_that_did_not_start_says_what_to_do() {
        let missing = browser_failed("browser launch failed (engine=chrome, headed=false): launch chrome: spawn /no/chrome --remote-debugging-pipe: No such file or directory (os error 2)");
        assert!(missing.contains("no browser at /no/chrome") && !missing.contains("engine="), "{missing}");
        let screen = browser_failed(
            "browser launch failed (engine=chrome, headed=true): closed (… Missing X server or $DISPLAY …)",
        );
        assert!(screen.contains("no screen"), "{screen}");
        let none = browser_failed(
            "browser launch failed (engine=chrome, headed=false): no Chrome found — run `navigera install-browser`",
        );
        assert!(none.contains("`bitti navigera install-browser`") && !none.contains("engine="), "{none}");
    }

    #[test]
    fn tag_expressions_read_as_cucumber_writes_them() {
        let t = |tags: &[&str]| tags.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let smoke_slow = t(&["smoke", "slow"]);
        assert!(tag_expr("smoke", &smoke_slow).unwrap());
        assert!(tag_expr("@smoke", &smoke_slow).unwrap());
        assert!(!tag_expr("smoke and not slow", &smoke_slow).unwrap());
        assert!(tag_expr("not wip and (smoke or api)", &smoke_slow).unwrap());
        assert!(tag_expr("api or slow and smoke", &smoke_slow).unwrap(), "and binds tighter");
        assert!(!tag_expr("not (smoke or api)", &smoke_slow).unwrap());
        for bad in ["", "smoke and", "(smoke", "smoke slow", "and smoke", "smoke)"] {
            assert!(tag_expr(bad, &smoke_slow).is_err(), "{bad:?}");
        }
    }
}
