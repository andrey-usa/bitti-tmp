//! Scenario files (`.bt`): plain text, one step per line.
//!
//! ```text
//! # comment
//! set coupon = SAVE10
//! before each
//!   goto /shop
//! scenario "A coupon lowers the total" @smoke
//!   fill --label "Coupon code" ${coupon}
//!   click --role button --name "Apply coupon"
//!   expect --testid total text is "$36.00"
//! ```
//!
//! Top-level lines are `set`, `base`, `use`, `flow`, `auth`, `before
//! all|each`, `after each|all` and `scenario`; the indented lines under a
//! block are its steps. A step is a navigera command line (tokenized like a
//! shell, without expansion), an `expect` line, `save NAME = <command>`,
//! `run`, `as`, `optional`, `if` or `handoff`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::expect::Expect;

/// What `http --graphql` sends.
#[derive(Debug, Clone, PartialEq)]
pub struct Graphql {
    /// The query, or the file it is in (`file`).
    pub query: String,
    pub file: bool,
    /// `--variables`: a JSON object.
    pub variables: Option<String>,
    /// `--operation`: which operation of the document to run.
    pub operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// A navigera command line.
    Command(Vec<String>),
    Expect(Expect),
    /// `save NAME = <command>`: the command's result as a variable.
    Save {
        name: String,
        command: Vec<String>,
    },
    /// `print <command…>`: a read command (`get text …`, `eval …`, `ax`,
    /// `url`) whose result the run prints, checking nothing and waiting
    /// for nothing: a look at the page or a value while the test is written.
    Print(Vec<String>),
    /// `run <flow> <args…>`.
    Run {
        flow: String,
        args: Vec<String>,
    },
    /// `optional <step>`: a failure is noted, not fatal (a banner that may
    /// or may not show).
    Optional(Box<Action>),
    /// `if <expect clause>` and the steps indented under it: run them when
    /// the clause holds now (or within its `--timeout-ms`).
    If {
        cond: Expect,
        steps: Vec<Step>,
    },
    /// `as <auth>`: start signed in, from the `auth` block's saved state.
    As(String),
    /// `handoff [until <expect clause>]`: the browser on screen for a
    /// person (a 2FA prompt, a CAPTCHA), until the clause holds or Enter.
    Handoff(Option<Expect>),
    /// `exec [--timeout-ms n] <program> <args…>`: a program run (no shell)
    /// in the `.bt` file's directory; its exit code and output become
    /// `${exec.exit}`, `${exec.stdout}`, `${exec.stderr}`. The steps
    /// indented under it run after it; a nonzero exit fails the step unless
    /// one of them checks `exit`.
    Exec {
        argv: Vec<String>,
        timeout_ms: Option<u64>,
        dir: PathBuf,
        steps: Vec<Step>,
    },
    /// `http <METHOD> <url> [--json <text> | --body <text> | --json-file f |
    /// --body-file f] [--header "Name: value"]… [--timeout-ms n]`: a request
    /// outside the page, sharing the scenario's cookies both ways. Its
    /// answer is `${http.status}`, `${http.body}`, `expect status|body|json
    /// <$.path>|header <name>`; a 4xx or 5xx fails the step unless one of
    /// the steps indented under it checks `status`.
    Http {
        method: String,
        url: String,
        headers: Vec<String>,
        body: Option<String>,
        body_file: Option<String>,
        json: bool,
        timeout_ms: Option<u64>,
        /// `--no-follow`: the redirect itself, not where it leads.
        follow: bool,
        /// `--insecure`: accept a certificate nothing vouches for (a local
        /// test server's own).
        insecure: bool,
        /// `--form name=value`…: an urlencoded form body.
        form: Vec<(String, String)>,
        /// `--multipart name=value` or `name=@file`…: a multipart body.
        multipart: Vec<(String, String)>,
        /// `--query name=value`…: added to the URL, encoded.
        query: Vec<(String, String)>,
        /// `--user name:password` (Basic) or `--bearer <token>`: the
        /// `Authorization` header, as `Basic …` or `Bearer …`.
        auth: Option<(String, String)>,
        /// `--graphql <query>` or `--graphql-file <f>` [`--variables <json>`]
        /// [`--operation <name>`]: a GraphQL request as its JSON body; an
        /// answer with `errors` fails the step unless a step under it
        /// checks `$.errors`.
        graphql: Option<Graphql>,
        steps: Vec<Step>,
    },
    /// `retry [--timeout-ms n]` and the steps indented under it: run them
    /// again, a little later each time, until all of them pass (a job that
    /// finishes in the background, an order that ships). Each `expect` in
    /// it looks once, unless it says how long.
    Retry {
        timeout_ms: Option<u64>,
        steps: Vec<Step>,
    },
    /// `pause`: stop here, the browser as it is (open to another navigera
    /// session), until Enter.
    Pause,
    /// A `.feature` step as written, until [`resolve`] finds what it does.
    Gherkin(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub line: usize,
    /// The line as written (trimmed), for reports.
    pub text: String,
    pub action: Action,
    /// How far the line is indented: the steps of an `if` are indented
    /// more than the `if`.
    pub indent: usize,
}

/// Steps read in order, nested by indentation under their `if`s.
fn nest(flat: Vec<Step>) -> Result<Vec<Step>, (usize, String)> {
    fn level(items: &[Step], i: &mut usize, indent: usize) -> Result<Vec<Step>, (usize, String)> {
        let mut out = Vec::new();
        while *i < items.len() {
            let st = &items[*i];
            if st.indent < indent {
                break;
            }
            if st.indent > indent {
                return Err((
                    st.line,
                    "indented more than the step before it (only the steps of an `if`, `retry`, `exec` or `http` are)"
                        .into(),
                ));
            }
            *i += 1;
            let mut st = st.clone();
            let line = st.line;
            let kids = match &mut st.action {
                Action::If { steps, .. } => Some((steps, "if")),
                Action::Retry { steps, .. } => Some((steps, "retry")),
                Action::Exec { steps, .. } | Action::Http { steps, .. } => Some((steps, "")),
                _ => None,
            };
            if let Some((steps, needed)) = kids {
                match items.get(*i) {
                    Some(next) if next.indent > indent => *steps = level(items, i, next.indent)?,
                    _ if needed == "if" => {
                        return Err((line, "`if` needs the steps it guards indented under it".into()))
                    }
                    _ if needed == "retry" => {
                        return Err((line, "`retry` needs the steps it runs again indented under it".into()))
                    }
                    _ => {}
                }
            }
            out.push(st);
        }
        Ok(out)
    }
    if flat.is_empty() {
        return Ok(flat);
    }
    let mut i = 0;
    let base = flat[0].indent;
    let out = level(&flat, &mut i, base)?;
    match flat.get(i) {
        Some(st) => Err((st.line, "indented less than the first step of its block".into())),
        None => Ok(out),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub tags: Vec<String>,
    pub line: usize,
    pub steps: Vec<Step>,
    /// A data row's values (`examples`, `for each <file.csv>`).
    pub vars: BTreeMap<String, String>,
}

/// `auth name [ttl 30m]`: steps that sign in, run once (or reused from
/// the last run within the TTL), whose cookies and storage `as name` loads.
#[derive(Debug, Clone, PartialEq)]
pub struct Auth {
    pub name: String,
    /// How long the saved state is reused across runs (seconds); none: once
    /// per run.
    pub ttl_s: Option<u64>,
    pub file: PathBuf,
    pub line: usize,
    pub steps: Vec<Step>,
}

/// `30m`, `1h`, `90s`, `2d`, or plain seconds.
pub fn duration_s(text: &str) -> Option<u64> {
    let (num, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len()));
    let n: u64 = num.parse().ok()?;
    Some(
        n * match unit {
            "" | "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86400,
            _ => return None,
        },
    )
}

/// `flow name(a, b)`: steps a scenario runs with `run name x y`.
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub name: String,
    pub params: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
    pub steps: Vec<Step>,
}

/// A `.feature` file's `Feature:` line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feature {
    pub name: String,
    pub line: usize,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Suite {
    pub file: PathBuf,
    /// Set for a `.feature` file.
    pub feature: Option<Feature>,
    pub base: Option<String>,
    pub vars: BTreeMap<String, String>,
    /// Once for the file, before its first scenario; what it saves the
    /// scenarios see.
    pub before_all: Vec<Step>,
    pub before_each: Vec<Step>,
    pub after_each: Vec<Step>,
    /// Once for the file, after its last scenario.
    pub after_all: Vec<Step>,
    pub scenarios: Vec<Scenario>,
    pub flows: BTreeMap<String, Flow>,
    pub auths: BTreeMap<String, Auth>,
    /// `use <file.bt>`: files whose flows and `set`s this one takes.
    pub uses: Vec<(usize, PathBuf)>,
}

/// A problem in a file, with where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub file: PathBuf,
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file.display(), self.line, self.message)
    }
}

/// Split a line the way a shell would, without expanding anything: words
/// separated by blanks, `'…'` taken as is, `"…"` with `\"`, `\\` and `\$`
/// escapes, a backslash outside quotes escaping the next character.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("a quote ' is not closed".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\')) => cur.push(ch),
                            // A line break in a value (a textarea's):
                            // the same in a pattern.
                            Some('n') => cur.push('\n'),
                            Some('r') => cur.push('\r'),
                            // `\$` stays escaped for interpolation to see.
                            Some('$') => cur.push_str("$$"),
                            Some(ch) => {
                                cur.push('\\');
                                cur.push(ch);
                            }
                            None => return Err("a quote \" is not closed (inside double quotes, write \\\" for a quote, or use single quotes)".into()),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("a quote \" is not closed (inside double quotes, write \\\" for a quote, or use single quotes)".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('$') => cur.push_str("$$"),
                    Some(ch) => cur.push(ch),
                    None => {}
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// Replace `${NAME}` and `${NAME:-default}` in a word, looking names up with
/// `lookup`; `$$` is a literal `$`.
pub fn interpolate(word: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
        } else if let Some(body) = after.strip_prefix('{') {
            let end = body.find('}').ok_or_else(|| format!("`${{` without `}}` in {word:?}"))?;
            let inner = &body[..end];
            let (name, default) = match inner.split_once(":-") {
                Some((n, d)) => (n, Some(d)),
                None => (inner, None),
            };
            let bare = name.strip_prefix("secret:").or_else(|| name.strip_prefix("totp:")).unwrap_or(name);
            if bare.is_empty() || !bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                return Err(format!("bad variable name {name:?} in {word:?}"));
            }
            match lookup(name).or_else(|| default.map(str::to_string)) {
                Some(v) => out.push_str(&v),
                None if name.starts_with("totp:") => {
                    return Err(format!(
                        "${{{name}}}: set {bare} (the environment, or --secrets) to the authenticator's base32 key \
                         or the otpauth:// URI of the site's QR code"
                    ))
                }
                None => return Err(format!("unknown variable ${{{name}}}")),
            }
            rest = &body[end + 1..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The tree an `expect ax … matches` step (or `optional` one) holds.
fn ax_tree(action: &mut Action) -> Option<&mut Vec<String>> {
    match action {
        Action::Expect(Expect { subject: crate::expect::Subject::Ax { tree, .. }, .. }) => Some(tree),
        Action::Optional(inner) => ax_tree(inner),
        _ => None,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Replace the `${NAME}`s that `vars` has, leave the others as written.
fn interpolate_known(word: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = word;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let body = &rest[i + 2..];
        match body.find('}') {
            Some(end) if vars.contains_key(&body[..end]) => {
                out.push_str(&vars[&body[..end]]);
                rest = &body[end + 1..];
            }
            _ => {
                out.push_str("${");
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A table row: `| a | b |` -> ["a", "b"].
fn table_row(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

/// Rows of a CSV file (first line: the column names). Double quotes around
/// a field allow commas in it (`""` for a quote).
pub fn csv_rows(text: &str) -> Result<Vec<BTreeMap<String, String>>, String> {
    let split = |line: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                '"' => quoted = !quoted,
                ',' if !quoted => out.push(std::mem::take(&mut cur).trim().to_string()),
                c => cur.push(c),
            }
        }
        out.push(cur.trim().to_string());
        out
    };
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = split(lines.next().ok_or("the file is empty")?);
    let mut rows = Vec::new();
    for (i, l) in lines.enumerate() {
        let cells = split(l);
        if cells.len() != header.len() {
            return Err(format!("row {} has {} fields, the header {}", i + 2, cells.len(), header.len()));
        }
        rows.push(header.iter().cloned().zip(cells).collect());
    }
    Ok(rows)
}

/// The `${NAME}`s a word uses (not `$${…}`), and whether each has a
/// default (`${NAME:-x}`).
pub fn used_vars(word: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = word;
    while let Some(i) = rest.find('$') {
        let after = &rest[i + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            rest = tail;
        } else if let Some(body) = after.strip_prefix('{') {
            let Some(end) = body.find('}') else { break };
            let inner = &body[..end];
            match inner.split_once(":-") {
                Some((n, _)) => out.push((n.to_string(), true)),
                None => out.push((inner.to_string(), false)),
            }
            rest = &body[end + 1..];
        } else {
            rest = after;
        }
    }
    out
}

/// Variables a scenario uses that nothing sets: not the file's `set`s or
/// its `use`d files', not `--var`, not its data row, not a `save` before
/// the use, not the environment. (line, name) for each.
pub fn unset_vars(suite: &Suite, sc: &Scenario, run_vars: &[String]) -> Vec<(usize, String)> {
    let mut known: std::collections::BTreeSet<String> = suite.vars.keys().cloned().collect();
    known.extend(run_vars.iter().cloned());
    known.extend(sc.vars.keys().cloned());
    known.extend(
        [
            "run.id",
            "scenario.name",
            "download.path",
            "download.name",
            "exec.exit",
            "exec.stdout",
            "exec.stderr",
            "http.status",
            "http.body",
        ]
        .map(String::from),
    );
    for st in &suite.before_all {
        if let Action::Save { name, .. } = &st.action {
            known.insert(name.clone());
        }
    }
    let mut out = Vec::new();
    fn words(a: &Action) -> Vec<String> {
        match a {
            Action::Command(w) | Action::Print(w) => w.clone(),
            Action::Save { command, .. } => command.clone(),
            Action::Run { args, .. } => args.clone(),
            Action::Optional(inner) => words(inner),
            Action::As(name) => vec![name.clone()],
            Action::Exec { argv, .. } => argv.clone(),
            Action::Gherkin(_) => Vec::new(),
            Action::Http { url, headers, body, form, multipart, query, auth, graphql, .. } => {
                std::iter::once(url)
                    .chain(headers)
                    .chain(body)
                    .chain(graphql.iter().flat_map(|g| {
                        (!g.file).then_some(&g.query).into_iter().chain(&g.variables).chain(&g.operation)
                    }))
                    .chain(auth.iter().map(|(_, v)| v))
                    .chain(form.iter().chain(multipart).chain(query).flat_map(|(k, v)| [k, v]))
                    .cloned()
                    .collect()
            }
            Action::Expect(_) | Action::If { .. } | Action::Handoff(_) | Action::Retry { .. } | Action::Pause => {
                Vec::new()
            }
        }
    }
    for st in suite.before_each.iter().chain(&sc.steps) {
        // An expect's words are in its text.
        let mut ws = match &st.action {
            Action::Expect(_) => tokenize(&st.text).unwrap_or_default(),
            a => words(a),
        };
        if let Action::Expect(Expect { subject: crate::expect::Subject::Ax { tree, .. }, .. }) = &st.action {
            ws.extend(tree.iter().cloned());
        }
        for w in &ws {
            for (name, has_default) in used_vars(w) {
                let bare = name.strip_prefix("secret:").or_else(|| name.strip_prefix("totp:")).unwrap_or(&name);
                let env = std::env::var_os(bare).is_some();
                if !has_default && !known.contains(&name) && !env {
                    out.push((st.line, name));
                }
            }
        }
        if let Action::Save { name, .. } = &st.action {
            known.insert(name.clone());
        }
        // What a flow saves is known after it runs.
        if let Action::Run { flow, .. } = &st.action {
            if let Some(f) = suite.flows.get(flow) {
                for fs in &f.steps {
                    if let Action::Save { name, .. } = &fs.action {
                        known.insert(name.clone());
                    }
                }
            }
        }
    }
    out
}

/// `print http`, `print body`, `print json [<$.path>]`, `print header
/// <name>`: what the last `http` answer said, no browser needed.
pub fn print_reads_answer(words: &[String]) -> bool {
    matches!(words.first().map(String::as_str), Some("http" | "body" | "json" | "header"))
}

/// Whether these steps need a browser. Everything does except `http` and
/// `exec` with the checks of their answers, checks of saved values and of
/// the cookies `http` collected, `save … = json|header`, `print` of an
/// answer, `wait --ms`, `retry` and `pause` over such steps, a flow made
/// of them, and `as` an auth that needs none itself. A scenario that needs
/// none runs with no browser at all.
pub fn needs_browser(suite: &Suite, steps: &[Step]) -> bool {
    fn expect(e: &Expect) -> bool {
        use crate::expect::Subject;
        !matches!(
            e.subject,
            Subject::Var(_)
                | Subject::Http(_)
                | Subject::Cookie(_)
                | Subject::JsonSchema { .. }
                | Subject::JsonSnapshot { .. }
        )
    }
    fn action(a: &Action, suite: &Suite, depth: usize) -> bool {
        // A flow that runs itself fails in the run; it isn't the browser's.
        if depth > 16 {
            return false;
        }
        let all = |steps: &[Step]| steps.iter().any(|st| action(&st.action, suite, depth + 1));
        match a {
            Action::Command(w) => !matches!(w.as_slice(), [op, flag, _] if op == "wait" && flag == "--ms"),
            Action::Save { command, .. } => !matches!(command.first().map(String::as_str), Some("json" | "header")),
            Action::Print(w) => !print_reads_answer(w) && !w.first().is_some_and(|w| w.contains("${")),
            Action::Expect(e) => expect(e),
            Action::If { cond, steps } => expect(cond) || all(steps),
            Action::Exec { steps, .. } | Action::Http { steps, .. } | Action::Retry { steps, .. } => all(steps),
            Action::Optional(inner) => action(inner, suite, depth),
            Action::Run { flow, .. } => suite.flows.get(flow).is_none_or(|f| all(&f.steps)),
            Action::As(name) => suite.auths.get(name).is_none_or(|auth| all(&auth.steps)),
            Action::Pause => false,
            Action::Handoff(_) | Action::Gherkin(_) => true,
        }
    }
    steps.iter().any(|st| action(&st.action, suite, 0))
}

impl Suite {
    /// Whether this scenario (with the file's `before each` and `after
    /// each`) needs a browser.
    pub fn scenario_needs_browser(&self, sc: &Scenario) -> bool {
        needs_browser(self, &self.before_each)
            || needs_browser(self, &sc.steps)
            || needs_browser(self, &self.after_each)
    }

    /// Whether running this scenario may start a browser: it, or the
    /// file's `before all` or `after all`, needs one.
    pub fn job_needs_browser(&self, sc: &Scenario) -> bool {
        self.scenario_needs_browser(sc) || needs_browser(self, &self.before_all) || needs_browser(self, &self.after_all)
    }

    /// Whether a `pause` step is anywhere this scenario may reach.
    pub fn has_pause(&self, sc: &Scenario) -> bool {
        fn any(steps: &[Step]) -> bool {
            steps.iter().any(|st| match &st.action {
                Action::Pause => true,
                Action::Optional(a) => matches!(a.as_ref(), Action::Pause),
                Action::If { steps, .. }
                | Action::Exec { steps, .. }
                | Action::Http { steps, .. }
                | Action::Retry { steps, .. } => any(steps),
                _ => false,
            })
        }
        [&self.before_all, &self.before_each, &sc.steps, &self.after_each, &self.after_all].into_iter().any(|s| any(s))
            || self.flows.values().any(|f| any(&f.steps))
            || self.auths.values().any(|a| any(&a.steps))
    }
}

/// Parse one file's text.
pub fn parse(file: &Path, text: &str) -> Result<Suite, ParseError> {
    let err = |line: usize, message: String| ParseError { file: file.to_path_buf(), line, message };
    let mut suite = Suite { file: file.to_path_buf(), ..Suite::default() };
    enum Block {
        None,
        BeforeAll,
        BeforeEach,
        AfterEach,
        AfterAll,
        Scenario,
        Flow(String),
        Auth(String),
    }
    let mut block = Block::None;
    let mut base_line: Option<usize> = None;
    // Data rows of the scenario being read: (line of `examples`, header, rows).
    let mut rows: BTreeMap<usize, Vec<BTreeMap<String, String>>> = BTreeMap::new();
    let mut table: Option<(usize, Option<Vec<String>>)> = None;
    let lines: Vec<&str> = text.lines().collect();
    // Lines up to here are the tree under an `expect ax … matches`.
    let mut tree_until = 0;
    for (i, raw) in lines.iter().copied().enumerate() {
        if i < tree_until {
            continue;
        }
        let line_no = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent_of(raw) > 0 {
            // `examples` and the table under it.
            if let Some((si, header)) = table.as_mut() {
                if trimmed.starts_with('|') {
                    let cells = table_row(trimmed);
                    match header {
                        None => *header = Some(cells),
                        Some(h) if h.len() == cells.len() => {
                            rows.entry(*si).or_default().push(h.iter().cloned().zip(cells).collect())
                        }
                        Some(h) => {
                            return Err(err(
                                line_no,
                                format!("{} cells in this row, {} in the header", cells.len(), h.len()),
                            ))
                        }
                    }
                    continue;
                }
                table = None;
            }
            if trimmed == "examples" {
                match block {
                    Block::Scenario => {
                        table = Some((suite.scenarios.len() - 1, None));
                        continue;
                    }
                    _ => return Err(err(line_no, "`examples` belongs under a scenario".into())),
                }
            }
            let mut step = parse_step(trimmed).map_err(|m| err(line_no, m))?;
            if let Some(tree) = ax_tree(&mut step) {
                // The tree: the lines indented under the step.
                let mut j = i + 1;
                while j < lines.len() && (lines[j].trim().is_empty() || indent_of(lines[j]) > indent_of(raw)) {
                    j += 1;
                }
                *tree = lines[i + 1..j].iter().map(|l| l.to_string()).collect();
                crate::axtree::compile(tree).map_err(|m| err(line_no, m))?;
                tree_until = j;
            }
            anchor_inputs(&mut step, file.parent().unwrap_or(Path::new(".")));
            let step = Step { line: line_no, text: trimmed.to_string(), action: step, indent: indent_of(raw) };
            match &block {
                Block::None => {
                    return Err(err(line_no, "a step outside any `scenario`, `flow` or `before each` block".into()))
                }
                Block::BeforeAll => suite.before_all.push(step),
                Block::BeforeEach => suite.before_each.push(step),
                Block::AfterEach => suite.after_each.push(step),
                Block::AfterAll => suite.after_all.push(step),
                Block::Scenario => suite.scenarios.last_mut().expect("a scenario is open").steps.push(step),
                Block::Flow(name) => suite.flows.get_mut(name).expect("the flow is open").steps.push(step),
                Block::Auth(name) => suite.auths.get_mut(name).expect("the auth is open").steps.push(step),
            }
            continue;
        }
        table = None;
        let words = tokenize(trimmed).map_err(|m| err(line_no, format!("{m}: {trimmed}")))?;
        match words.first().map(String::as_str) {
            Some("set") => {
                // set NAME = value…
                let rest = trimmed["set".len()..].trim();
                let (name, value) =
                    rest.split_once('=').ok_or_else(|| err(line_no, "expected `set NAME = value`".into()))?;
                let name = name.trim();
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(err(line_no, format!("bad variable name {name:?}")));
                }
                let value = tokenize(value.trim()).map_err(|m| err(line_no, m))?.join(" ");
                suite.vars.insert(name.to_string(), value);
                block = Block::None;
            }
            Some("base") => {
                let url = words.get(1).ok_or_else(|| err(line_no, "expected `base <url>`".into()))?;
                if let Some(first) = base_line {
                    return Err(err(line_no, format!("a second `base` (the first is at line {first}); keep one")));
                }
                base_line = Some(line_no);
                suite.base = Some(url.clone());
                block = Block::None;
            }
            Some("use") => {
                let path = words.get(1).ok_or_else(|| err(line_no, "expected `use <file.bt>`".into()))?;
                let dir = file.parent().unwrap_or(Path::new("."));
                suite.uses.push((line_no, dir.join(path)));
                block = Block::None;
            }
            Some(w @ ("before" | "after")) => {
                let which = words.get(1).map(String::as_str);
                if words.len() != 2 || !matches!(which, Some("each" | "all")) {
                    return Err(err(line_no, format!("expected `{w} each` or `{w} all`")));
                }
                let (b, taken) = match (w, which) {
                    ("before", Some("all")) => (Block::BeforeAll, !suite.before_all.is_empty()),
                    ("before", _) => (Block::BeforeEach, !suite.before_each.is_empty()),
                    ("after", Some("all")) => (Block::AfterAll, !suite.after_all.is_empty()),
                    _ => (Block::AfterEach, !suite.after_each.is_empty()),
                };
                if taken {
                    return Err(err(line_no, format!("a second `{trimmed}` block: put its steps in the first")));
                }
                block = b;
            }
            Some("auth") => {
                // auth name [ttl 30m]
                let name = words.get(1).ok_or_else(|| err(line_no, "expected `auth <name> [ttl 30m]`".into()))?.clone();
                if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return Err(err(line_no, format!("bad auth name {name:?}")));
                }
                let ttl_s = match &words[2..] {
                    [] => None,
                    [t, d] if t == "ttl" => Some(duration_s(d).ok_or_else(|| {
                        err(line_no, format!("ttl takes a duration like 30m, 1h, 90s, 2d (got {d:?})"))
                    })?),
                    other => {
                        return Err(err(
                            line_no,
                            format!("after the auth's name: `ttl <duration>` or nothing (got {other:?})"),
                        ))
                    }
                };
                if suite.auths.contains_key(&name) {
                    return Err(err(line_no, format!("a second auth named {name:?}")));
                }
                suite.auths.insert(
                    name.clone(),
                    Auth { name: name.clone(), ttl_s, file: file.to_path_buf(), line: line_no, steps: Vec::new() },
                );
                block = Block::Auth(name);
            }
            Some("flow") => {
                // flow name(a, b)   or   flow name
                let rest = trimmed["flow".len()..].trim();
                let (name, params) = match rest.split_once('(') {
                    Some((n, p)) => {
                        let p = p.strip_suffix(')').ok_or_else(|| err(line_no, "expected `flow name(a, b)`".into()))?;
                        (n.trim(), p.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                    }
                    None => (rest, Vec::new()),
                };
                let ok =
                    |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !ok(name) || !params.iter().all(|p: &String| ok(p)) {
                    return Err(err(line_no, format!("bad flow header {rest:?} (expected `flow name(a, b)`)")));
                }
                if suite.flows.contains_key(name) {
                    return Err(err(line_no, format!("a second flow named {name:?}")));
                }
                suite.flows.insert(
                    name.to_string(),
                    Flow { name: name.to_string(), params, file: file.to_path_buf(), line: line_no, steps: Vec::new() },
                );
                block = Block::Flow(name.to_string());
            }
            Some("step") => {
                // step "I sign in as {email}": a step for .feature files.
                let pattern = match words.as_slice() {
                    [_, p] if !p.trim().is_empty() => p.clone(),
                    _ => return Err(err(line_no, "expected `step \"<words, {placeholders}>\"`".into())),
                };
                let params = crate::gherkin::step_params(&pattern).map_err(|m| err(line_no, m))?;
                let name = format!("step {pattern:?}");
                if suite.flows.contains_key(&name) {
                    return Err(err(line_no, format!("a second step {pattern:?}")));
                }
                suite.flows.insert(
                    name.clone(),
                    Flow { name: name.clone(), params, file: file.to_path_buf(), line: line_no, steps: Vec::new() },
                );
                block = Block::Flow(name);
            }
            Some("scenario") => {
                let name = words.get(1).ok_or_else(|| err(line_no, "expected `scenario \"name\"`".into()))?.clone();
                let mut tags = Vec::new();
                let mut k = 2;
                while k < words.len() {
                    let w = &words[k];
                    if w == "for" && words.get(k + 1).map(String::as_str) == Some("each") {
                        let csv =
                            words.get(k + 2).ok_or_else(|| err(line_no, "expected `for each <file.csv>`".into()))?;
                        let path = file.parent().unwrap_or(Path::new(".")).join(csv);
                        let text = std::fs::read_to_string(&path)
                            .map_err(|e| err(line_no, format!("reading {}: {e}", path.display())))?;
                        let data = csv_rows(&text).map_err(|m| err(line_no, format!("{}: {m}", path.display())))?;
                        rows.entry(suite.scenarios.len()).or_default().extend(data);
                        k += 3;
                        continue;
                    }
                    match w.strip_prefix('@') {
                        Some(t) if !t.is_empty() => tags.push(t.to_string()),
                        _ => {
                            return Err(err(
                                line_no,
                                format!("after the name, only @tags or `for each <file.csv>`: {w:?}"),
                            ))
                        }
                    }
                    k += 1;
                }
                if let Some(first) = suite.scenarios.iter().find(|s| s.name == name) {
                    return Err(err(
                        line_no,
                        format!("a second scenario named {name:?} (the first is at line {}); rename one", first.line),
                    ));
                }
                suite.scenarios.push(Scenario { name, tags, line: line_no, steps: Vec::new(), vars: BTreeMap::new() });
                block = Block::Scenario;
            }
            Some(other) => {
                // A step at the left margin is the common slip: say that.
                let step = other == "expect"
                    || matches!(other, "save" | "run" | "optional" | "if" | "handoff")
                    || navigera::protocol::op_help("navigera", other).is_some();
                return Err(err(
                    line_no,
                    if step {
                        format!(
                            "{other:?} is a step, and a step sits indented under a scenario (or `before each`, a \
                             flow, …): add `scenario \"<name>\"` above it and indent it"
                        )
                    } else {
                        format!("unknown line {other:?} (top level: set, base, use, flow, auth, before all|each, after each|all, scenario; steps are indented)")
                    },
                ));
            }
            None => {}
        }
    }
    let nested = |steps: Vec<Step>| nest(steps).map_err(|(line, m)| err(line, m));
    suite.before_all = nested(std::mem::take(&mut suite.before_all))?;
    suite.before_each = nested(std::mem::take(&mut suite.before_each))?;
    suite.after_each = nested(std::mem::take(&mut suite.after_each))?;
    suite.after_all = nested(std::mem::take(&mut suite.after_all))?;
    for a in suite.auths.values_mut() {
        a.steps = nested(std::mem::take(&mut a.steps))?;
        if a.steps.is_empty() {
            return Err(err(a.line, format!("auth {:?} has no steps", a.name)));
        }
    }
    for sc in suite.scenarios.iter_mut() {
        sc.steps = nested(std::mem::take(&mut sc.steps))?;
    }
    for f in suite.flows.values_mut() {
        f.steps = nested(std::mem::take(&mut f.steps))?;
    }
    for s in &suite.scenarios {
        if s.steps.is_empty() {
            return Err(err(s.line, format!("scenario {:?} has no steps", s.name)));
        }
    }
    for f in suite.flows.values() {
        if f.steps.is_empty() {
            return Err(err(f.line, format!("flow {:?} has no steps", f.name)));
        }
    }
    // One scenario per data row, named from the row.
    if !rows.is_empty() {
        let mut out = Vec::new();
        for (i, sc) in suite.scenarios.drain(..).enumerate() {
            let Some(data) = rows.get(&i) else {
                out.push(sc);
                continue;
            };
            if data.is_empty() {
                return Err(err(sc.line, format!("scenario {:?}: its data has no rows", sc.name)));
            }
            for (k, row) in data.iter().enumerate() {
                let mut name = interpolate_known(&sc.name, row);
                if name == sc.name {
                    name = format!("{} [{}]", sc.name, k + 1);
                }
                out.push(Scenario { name, vars: row.clone(), ..sc.clone() });
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for sc in &out {
            if !seen.insert(sc.name.clone()) {
                return Err(err(
                    sc.line,
                    format!("two data rows name the scenario {:?}: put a column in its name", sc.name),
                ));
            }
        }
        suite.scenarios = out;
    }
    Ok(suite)
}

/// `use` files merged in: their flows and `set`s, then this file's own on
/// top. Checks that every `run` names a flow with the right arguments.
pub fn resolve(suites: &mut [Suite]) -> Result<(), ParseError> {
    // A file `use`d by many suites is read and parsed once for the run,
    // not once per suite that names it.
    let mut parsed: std::collections::BTreeMap<PathBuf, Suite> = std::collections::BTreeMap::new();
    for suite in suites.iter_mut() {
        resolve_uses(suite, &mut parsed)?;
    }
    // A .feature file's steps: the `step` blocks of every .bt file in the
    // run, each once however many files `use` it, compiled once.
    let mut defined = std::collections::BTreeSet::new();
    let defs: Vec<Flow> = suites
        .iter()
        .flat_map(|s| s.flows.values())
        .filter(|f| f.name.starts_with("step \"") && defined.insert((f.file.clone(), f.line)))
        .cloned()
        .collect();
    let compiled = crate::gherkin::compile(&defs);
    // Steps navigera has already read, by file and line: a flow shared
    // through `use` is read once for the run, not once per suite using it.
    let mut read = std::collections::HashSet::new();
    for suite in suites.iter_mut() {
        if suite.feature.is_some() {
            crate::gherkin::resolve(suite, &compiled)?;
        }
        check_suite(suite, &mut read)?;
    }
    Ok(())
}

/// `use <file.bt>`: the flows, auths and `set`s of the files it names.
/// `parsed` is the run's cache, keyed by canonical path: a shared steps
/// file is parsed the first time any suite uses it and never again.
fn resolve_uses(suite: &mut Suite, parsed: &mut std::collections::BTreeMap<PathBuf, Suite>) -> Result<(), ParseError> {
    {
        let mut seen = std::collections::BTreeSet::new();
        let mut todo: Vec<(usize, PathBuf)> = suite.uses.clone();
        while let Some((line, path)) = todo.pop() {
            let key = std::fs::canonicalize(&path).unwrap_or(path.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(used) = parsed.get(&key).cloned() {
                for (name, flow) in used.flows {
                    suite.flows.entry(name).or_insert(flow);
                }
                for (name, auth) in used.auths {
                    suite.auths.entry(name).or_insert(auth);
                }
                for (k, v) in used.vars {
                    suite.vars.entry(k).or_insert(v);
                }
                todo.extend(used.uses);
                continue;
            }
            let err = |m: String| ParseError { file: suite.file.clone(), line, message: m };
            #[cfg(test)]
            USE_READS.with(|n| n.set(n.get() + 1));
            let text = std::fs::read_to_string(&path).map_err(|e| {
                err(match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        format!("use {}: no such file (a name is found next to this .bt)", path.display())
                    }
                    _ => format!("use {}: {}", path.display(), crate::plain_io(&e)),
                })
            })?;
            let used = parse(&path, &text)?;
            parsed.insert(key, used.clone());
            for (name, flow) in used.flows {
                suite.flows.entry(name).or_insert(flow);
            }
            for (name, auth) in used.auths {
                suite.auths.entry(name).or_insert(auth);
            }
            for (k, v) in used.vars {
                suite.vars.entry(k).or_insert(v);
            }
            todo.extend(used.uses);
        }
    }
    Ok(())
}

// How many `use` files the run read, for the test that holds a shared
// file to one parse per run.
#[cfg(test)]
thread_local! {
    static USE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn check_suite(suite: &Suite, read: &mut std::collections::HashSet<(PathBuf, usize)>) -> Result<(), ParseError> {
    {
        // Every `run`, wherever it is, names a known flow; every `as` a
        // known auth (both per suite: a shared flow can lean on what the
        // suite using it defines). What a step says on its own (its files,
        // navigera's reading of it) is checked once per step in the run.
        fn check(
            steps: &[Step],
            file: &Path,
            suite: &Suite,
            read: &mut std::collections::HashSet<(PathBuf, usize)>,
        ) -> Result<(), ParseError> {
            let flows = &suite.flows;
            for st in steps {
                // A script that isn't there is a mistake in the test, not
                // the page: refused even under `if` or `optional`, which
                // would otherwise read it as a check that didn't hold.
                let expect = match &st.action {
                    Action::If { cond, .. } => Some(cond),
                    Action::Expect(e) => Some(e),
                    Action::Optional(a) => match a.as_ref() {
                        Action::Expect(e) => Some(e),
                        _ => None,
                    },
                    _ => None,
                };
                let first = read.insert((file.to_path_buf(), st.line));
                if let (true, Some(Expect { subject: crate::expect::Subject::JsFile(f), .. })) = (first, expect) {
                    if !f.contains("${") && !Path::new(f).exists() {
                        return Err(ParseError {
                            file: file.to_path_buf(),
                            line: st.line,
                            message: format!("js --file {f}: no such file (a script is found next to the .bt)"),
                        });
                    }
                }
                if let (true, Some(Expect { subject: crate::expect::Subject::JsonSchema { file: f, .. }, .. })) =
                    (first, expect)
                {
                    // `openapi.json#/components/schemas/Item`: the file before the `#`.
                    let f = f.split_once('#').map_or(f.as_str(), |(f, _)| f);
                    if !f.contains("${") && !Path::new(f).exists() {
                        return Err(ParseError {
                            file: file.to_path_buf(),
                            line: st.line,
                            message: format!("schema {f}: no such file (a schema is found next to the .bt)"),
                        });
                    }
                }
                let action = match &st.action {
                    Action::Optional(a) => a.as_ref(),
                    Action::If { steps, .. }
                    | Action::Exec { steps, .. }
                    | Action::Http { steps, .. }
                    | Action::Retry { steps, .. } => {
                        check(steps, file, suite, read)?;
                        continue;
                    }
                    a => a,
                };
                // A browser step navigera would refuse (a misspelled step,
                // a flag it lacks) fails here, before any browser starts,
                // so `lint` catches it too. A word with a variable in it is
                // checked with a stand-in value.
                let words = match action {
                    Action::Print(w) if print_reads_answer(w) => None,
                    Action::Command(w) | Action::Print(w) => Some(w),
                    Action::Save { command, .. }
                        if !matches!(command.first().map(String::as_str), Some("json" | "header")) =>
                    {
                        Some(command)
                    }
                    _ => None,
                };
                if let Some(words) = words.filter(|w| first && w.first().is_some_and(|op| !op.contains("${"))) {
                    let stand_in: Vec<String> =
                        words.iter().map(|w| if w.contains("${") { "1".to_string() } else { w.clone() }).collect();
                    if let Err(message) = crate::runner::parse(&stand_in) {
                        return Err(ParseError { file: file.to_path_buf(), line: st.line, message });
                    }
                }
                if let Action::As(name) = action {
                    if !suite.auths.contains_key(name) {
                        return Err(ParseError {
                            file: file.to_path_buf(),
                            line: st.line,
                            message: format!("no auth named {name:?} (define it with `auth {name}` or `use` its file)"),
                        });
                    }
                }
                if let Action::Run { flow, args } = action {
                    let err = |m: String| ParseError { file: file.to_path_buf(), line: st.line, message: m };
                    let f = flows.get(flow).ok_or_else(|| {
                        err(format!("no flow named {flow:?} (define it with `flow {flow}(…)` or `use` its file)"))
                    })?;
                    if f.params.len() != args.len() {
                        return Err(err(format!(
                            "flow {flow} takes {} argument(s) ({}), not {}",
                            f.params.len(),
                            f.params.join(", "),
                            args.len()
                        )));
                    }
                }
            }
            Ok(())
        }
        let file = suite.file.clone();
        for steps in [&suite.before_all, &suite.before_each, &suite.after_each, &suite.after_all] {
            check(steps, &file, suite, read)?;
        }
        for sc in &suite.scenarios {
            check(&sc.steps, &file, suite, read)?;
        }
        for f in suite.flows.values() {
            check(&f.steps, &f.file, suite, read)?;
        }
        for a in suite.auths.values() {
            check(&a.steps, &a.file, suite, read)?;
        }
    }
    Ok(())
}

/// Files a step reads (`upload`, `state load`, `eval --file`, `init-script
/// --file`, `route --body-file`, `route --har`) are found next to the `.bt` file, as `use`
/// and `for each` are, so a suite runs the same from any directory. Absolute
/// paths, `-` (stdin) and paths that start with a variable are left alone.
fn anchor_inputs(action: &mut Action, dir: &Path) {
    let words = match action {
        Action::Command(w) | Action::Print(w) | Action::Save { command: w, .. } => w,
        Action::Optional(inner) => return anchor_inputs(inner, dir),
        Action::Exec { argv, dir: d, .. } => {
            // It runs there; a program named by a path is found from there.
            *d = dir.to_path_buf();
            if let Some(p) = argv.first_mut().filter(|p| (p.contains('/') || p.contains('\\')) && !p.starts_with("${"))
            {
                // Absolute: a relative one would be looked up from the
                // program's own working directory (`dir`) a second time.
                if Path::new(p.as_str()).is_relative() {
                    let at = dir.join(p.as_str());
                    *p = std::path::absolute(&at).unwrap_or(at).to_string_lossy().into_owned();
                }
            }
            return;
        }
        Action::Expect(Expect { subject: crate::expect::Subject::DownloadCompare { expected: f, .. }, .. }) => {
            if !f.starts_with("${") && Path::new(f.as_str()).is_relative() {
                *f = dir.join(f.as_str()).to_string_lossy().into_owned();
            }
            return;
        }
        Action::Expect(Expect { subject: crate::expect::Subject::JsFile(f), .. })
        | Action::Expect(Expect { subject: crate::expect::Subject::JsonSchema { file: f, .. }, .. })
        | Action::If { cond: Expect { subject: crate::expect::Subject::JsFile(f), .. }, .. } => {
            if !f.starts_with("${") && Path::new(f.as_str()).is_relative() {
                *f = dir.join(f.as_str()).to_string_lossy().into_owned();
            }
            return;
        }
        Action::Http { body_file, multipart, graphql, .. } => {
            if let Some(g) = graphql.as_mut().filter(|g| g.file) {
                if !g.query.starts_with("${") && Path::new(&g.query).is_relative() {
                    g.query = dir.join(&g.query).to_string_lossy().into_owned();
                }
            }
            if let Some(f) = body_file.as_mut() {
                if !f.starts_with("${") && Path::new(f.as_str()).is_relative() {
                    *f = dir.join(f.as_str()).to_string_lossy().into_owned();
                }
            }
            // `name=@file`: the file beside the `.bt`.
            for (_, v) in multipart.iter_mut() {
                if let Some(f) = v.strip_prefix('@') {
                    let path = f.split(';').next().unwrap_or(f);
                    if !path.starts_with("${") && Path::new(path).is_relative() {
                        *v = format!("@{}{}", dir.join(path).to_string_lossy(), &f[path.len()..]);
                    }
                }
            }
            return;
        }
        _ => return,
    };
    let anchor = |w: &mut String| {
        if !w.is_empty() && w != "-" && !w.starts_with("${") && Path::new(w.as_str()).is_relative() {
            *w = dir.join(w.as_str()).to_string_lossy().into_owned();
        }
    };
    let flag_value = |words: &mut [String], flag: &str| {
        let mut i = 1;
        while i + 1 < words.len() {
            if words[i] == flag {
                anchor(&mut words[i + 1]);
                i += 1;
            }
            i += 1;
        }
    };
    match words.first().map(String::as_str) {
        Some("upload") => {
            // `upload [<target>] --file a [--file b] [c d …]`: every flag
            // takes a value; the first bare word is the target when no
            // locator flag names one, the rest are files.
            const TARGET: &[&str] = &[
                "--selector",
                "--ref",
                "--text",
                "--role",
                "--name",
                "--label",
                "--placeholder",
                "--testid",
                "--nth",
                "--frame",
            ];
            let mut target_open = !words.iter().any(|w| TARGET.contains(&w.as_str()));
            let mut i = 1;
            while i < words.len() {
                if words[i] == "--file" {
                    if let Some(v) = words.get_mut(i + 1) {
                        anchor(v);
                    }
                    i += 2;
                } else if words[i].starts_with("--") {
                    i += 2;
                } else if target_open {
                    target_open = false;
                    i += 1;
                } else {
                    anchor(&mut words[i]);
                    i += 1;
                }
            }
        }
        Some("state") if words.get(1).is_some_and(|w| w == "load") => {
            if let Some(path) = words.get_mut(2) {
                anchor(path);
            }
        }
        Some("eval" | "evaluate" | "init-script" | "addinitscript") => flag_value(words, "--file"),
        Some("route") => {
            flag_value(words, "--body-file");
            flag_value(words, "--har");
        }
        _ => {}
    }
}

fn parse_step(line: &str) -> Result<Action, String> {
    // The line with it: a long step's open quote is hard to find by number.
    let words = tokenize(line).map_err(|m| format!("{m}: {line}"))?;
    parse_words(&words)
}

pub fn parse_words(words: &[String]) -> Result<Action, String> {
    match words.first().map(String::as_str) {
        Some("expect") => Ok(Action::Expect(crate::expect::parse(&words[1..])?)),
        // A file's variable written under a scenario: a step `set` is the
        // browser's (viewport, locale, …), and would fail only in the run.
        Some("set") if words.get(2).is_some_and(|w| w == "=") => Err(format!(
            "`set {} = …` goes at the top of the file, not indented; in a scenario, `save NAME = <command>` keeps \
             a result",
            words[1]
        )),
        Some("save") => {
            // save NAME = <command…>
            if words.len() < 4 || words[2] != "=" {
                return Err("expected `save NAME = <command>`".into());
            }
            Ok(Action::Save { name: words[1].clone(), command: words[3..].to_vec() })
        }
        Some("print") => {
            if words.len() < 2 {
                return Err("print needs a command whose result to show: `print get text --testid total`, \
                            `print eval \"document.title\"`, `print ax --selector main`"
                    .into());
            }
            Ok(Action::Print(words[1..].to_vec()))
        }
        Some("run") => {
            let flow = words.get(1).ok_or("expected `run <flow> <args…>`")?.clone();
            Ok(Action::Run { flow, args: words[2..].to_vec() })
        }
        Some("if") => {
            if words.len() < 2 {
                return Err("expected `if <expect clause>` (as after `expect`), then indented steps".into());
            }
            // `if expect text "x"` reads as written: the word is optional.
            let clause = if words.len() > 2 && words[1] == "expect" { &words[2..] } else { &words[1..] };
            let mut cond = crate::expect::parse(clause)?;
            if matches!(cond.subject, crate::expect::Subject::Ax { .. }) {
                return Err("`if` takes its steps indented under it, so not `ax … matches`".into());
            }
            // A condition looks at the page as it is, unless it says how long.
            cond.timeout_ms = Some(cond.timeout_ms.unwrap_or(0));
            Ok(Action::If { cond, steps: Vec::new() })
        }
        Some("exec") => {
            let usage = "expected `exec [--timeout-ms <ms>] <program> <args…>`";
            let mut rest = &words[1..];
            let mut timeout_ms = None;
            if rest.first().map(String::as_str) == Some("--timeout-ms") {
                let v = rest.get(1).ok_or(usage)?;
                timeout_ms = Some(v.parse::<u64>().map_err(|_| format!("--timeout-ms takes milliseconds, not {v:?}"))?);
                rest = &rest[2..];
            }
            if rest.is_empty() {
                return Err(usage.into());
            }
            Ok(Action::Exec { argv: rest.to_vec(), timeout_ms, dir: PathBuf::new(), steps: Vec::new() })
        }
        Some("http") => {
            let usage = "expected `http <METHOD> <url> [--json <text> | --body <text> | --json-file <f> | --body-file <f> | --form name=value… | --multipart name=value|name=@file… | --graphql <query> | --graphql-file <f> [--variables <json>] [--operation <name>]] [--query name=value]… [--user name:password | --bearer <token>] [--header \"Name: value\"]… [--no-follow] [--insecure] [--timeout-ms <ms>]`";
            let method = words.get(1).ok_or(usage)?.to_uppercase();
            let url = words.get(2).ok_or(usage)?.clone();
            if method.starts_with('-') || url.starts_with("--") {
                return Err(usage.into());
            }
            let (mut headers, mut body, mut body_file, mut json, mut timeout_ms) =
                (Vec::new(), None, None, false, None);
            let (mut follow, mut insecure) = (true, false);
            let (mut form, mut multipart, mut query) = (Vec::new(), Vec::new(), Vec::new());
            let mut auth: Option<(String, String)> = None;
            let (mut graphql, mut variables, mut operation): (Option<(String, bool)>, Option<String>, Option<String>) =
                (None, None, None);
            let pair = |flag: &str, v: &str| -> Result<(String, String), String> {
                v.split_once('=')
                    .filter(|(k, _)| !k.is_empty())
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .ok_or_else(|| format!("{flag} takes name=value, not {v:?}"))
            };
            let mut rest = words[3..].iter();
            while let Some(flag) = rest.next() {
                match flag.as_str() {
                    "--no-follow" => {
                        follow = false;
                        continue;
                    }
                    "--insecure" => {
                        insecure = true;
                        continue;
                    }
                    _ => {}
                }
                let v = rest.next().ok_or_else(|| format!("{flag} needs a value"))?.clone();
                match flag.as_str() {
                    "--header" => {
                        if !v.contains(':') {
                            return Err(format!("--header takes \"Name: value\", not {v:?}"));
                        }
                        headers.push(v);
                    }
                    "--user" | "--bearer" => {
                        if auth.is_some() {
                            return Err("one of --user and --bearer".into());
                        }
                        if flag == "--user" && !v.contains(':') && !v.contains("${") {
                            return Err(format!("--user takes name:password, not {v:?}"));
                        }
                        auth = Some((flag.trim_start_matches("--").to_string(), v));
                    }
                    "--form" => form.push(pair(flag, &v)?),
                    "--multipart" => multipart.push(pair(flag, &v)?),
                    "--query" => query.push(pair(flag, &v)?),
                    "--json" | "--body" | "--json-file" | "--body-file" => {
                        if body.is_some() || body_file.is_some() {
                            return Err("one body: --json, --body, --json-file or --body-file".into());
                        }
                        json = flag.starts_with("--json");
                        if flag.ends_with("-file") {
                            body_file = Some(v);
                        } else {
                            body = Some(v);
                        }
                    }
                    "--graphql" | "--graphql-file" => {
                        if graphql.is_some() {
                            return Err("one of --graphql and --graphql-file".into());
                        }
                        graphql = Some((v, flag.ends_with("-file")));
                    }
                    "--variables" => variables = Some(v),
                    "--operation" => operation = Some(v),
                    "--timeout-ms" => {
                        timeout_ms =
                            Some(v.parse::<u64>().map_err(|_| format!("--timeout-ms takes milliseconds, not {v:?}"))?)
                    }
                    other => return Err(format!("{usage} (got {other:?})")),
                }
            }
            let kinds =
                [body.is_some() || body_file.is_some(), !form.is_empty(), !multipart.is_empty(), graphql.is_some()];
            if kinds.iter().filter(|k| **k).count() > 1 {
                return Err("one kind of body: --json/--body (or their -file), --form, --multipart or --graphql".into());
            }
            if graphql.is_none() && (variables.is_some() || operation.is_some()) {
                return Err("--variables and --operation go with --graphql".into());
            }
            if let Some(v) = variables.as_deref().filter(|v| !v.contains("${")) {
                if !serde_json::from_str::<serde_json::Value>(v).is_ok_and(|j| j.is_object()) {
                    return Err(format!("--variables takes a JSON object, not {v:?}"));
                }
            }
            let graphql = graphql.map(|(query, file)| Graphql { query, file, variables, operation });
            json |= graphql.is_some();
            Ok(Action::Http {
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
                steps: Vec::new(),
            })
        }
        Some("retry") => {
            let usage = "expected `retry [--timeout-ms <ms>]`, then the steps to run again indented under it";
            match &words[1..] {
                [] => Ok(Action::Retry { timeout_ms: None, steps: Vec::new() }),
                [flag, v] if flag == "--timeout-ms" => Ok(Action::Retry {
                    timeout_ms: Some(
                        v.parse::<u64>().map_err(|_| format!("--timeout-ms takes milliseconds, not {v:?}"))?,
                    ),
                    steps: Vec::new(),
                }),
                _ => Err(usage.into()),
            }
        }
        Some("pause") if words.len() == 1 => Ok(Action::Pause),
        Some("as") => match words {
            [_, name] => Ok(Action::As(name.clone())),
            _ => Err("expected `as <auth name>`".into()),
        },
        Some("handoff") => match words.get(1).map(String::as_str) {
            None => Ok(Action::Handoff(None)),
            Some("until") if words.len() > 2 => {
                let mut cond = crate::expect::parse(&words[2..])?;
                if matches!(cond.subject, crate::expect::Subject::Ax { .. }) {
                    return Err("`handoff until` can't wait for `ax … matches`".into());
                }
                // A person takes a while: five minutes unless it says.
                cond.timeout_ms = Some(cond.timeout_ms.unwrap_or(300_000));
                Ok(Action::Handoff(Some(cond)))
            }
            _ => Err("expected `handoff` or `handoff until <expect clause>`".into()),
        },
        Some("optional") => {
            if words.len() < 2 {
                return Err("expected `optional <step>`".into());
            }
            match parse_words(&words[1..])? {
                Action::Optional(_) => Err("`optional` is said once, before the step: `optional click …`".into()),
                a => Ok(Action::Optional(Box::new(a))),
            }
        }
        Some(op) => {
            const TAKES_A_REF: &[&str] =
                &["click", "dblclick", "hover", "fill", "select", "check", "uncheck", "focus", "upload", "drag"];
            if TAKES_A_REF.contains(&op)
                && words.get(1).is_some_and(|w| w.trim_start_matches(['@', 'e']).parse::<u64>().is_ok())
            {
                // `click 12`: a ref from an `ax` of some earlier session.
                return Err(format!(
                    "refs ({}) change between runs: use a locator (--role/--name, --label, --text, --testid, --selector)",
                    words[1]
                ));
            }
            // `click "Buy now"`: visible text, not the CSS `buy now` (no
            // element is named that). A word CSS could mean (`button`,
            // `#save`, `.nav a`) stays CSS.
            if TAKES_A_REF.contains(&op) && op != "drag" && words.get(1).is_some_and(|w| reads_as_text(w)) {
                let mut out = vec![words[0].clone(), "--text".to_string()];
                out.extend(words[1..].iter().cloned());
                return Ok(Action::Command(out));
            }
            Ok(Action::Command(words.to_vec()))
        }
        None => Err("empty step".into()),
    }
}

/// A step's first word that only makes sense as the text of an element:
/// no CSS punctuation, and a space, a capital or a non-ASCII letter in it.
fn reads_as_text(word: &str) -> bool {
    !word.starts_with('-')
        && !word.contains("${")
        && !word.contains(['#', '.', '[', ']', ':', '>', '~', '+', '*', '=', '(', ')', '"', '\''])
        && word.chars().any(|c| c.is_whitespace() || c.is_uppercase() || !c.is_ascii())
}

/// The `.bt` files under each path (a file is taken as is).
pub fn discover(paths: &[PathBuf]) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(&p, out)?;
            } else if matches!(p.extension().and_then(|e| e.to_str()), Some("bt" | "feature")) {
                out.push(p);
            }
        }
        Ok(())
    }
    for p in paths {
        if p.is_dir() {
            walk(p, &mut out)?;
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_like_a_shell_without_expansion() {
        assert_eq!(
            tokenize(r#"fill --label "Coupon code" SAVE10"#).unwrap(),
            ["fill", "--label", "Coupon code", "SAVE10"]
        );
        assert_eq!(tokenize(r#"expect text 'It''s'"#).unwrap(), ["expect", "text", "Its"]);
        assert_eq!(tokenize(r#"a "x \"y\" \\ z""#).unwrap(), ["a", r#"x "y" \ z"#]);
        assert_eq!(
            tokenize(r##"click --selector "#cart .item:nth-child(2)""##).unwrap()[2],
            "#cart .item:nth-child(2)"
        );
        assert_eq!(tokenize(r#"expect text "$36.00""#).unwrap()[2], "$36.00");
        assert!(tokenize(r#"a "open"#).is_err());
    }

    #[test]
    fn variables_interpolate_with_defaults_and_escapes() {
        let look = |n: &str| (n == "who").then(|| "Ada Lovelace".to_string());
        assert_eq!(interpolate("hi ${who}!", &look).unwrap(), "hi Ada Lovelace!");
        assert_eq!(interpolate("${nope:-fallback}", &look).unwrap(), "fallback");
        assert_eq!(interpolate("$36.00", &look).unwrap(), "$36.00");
        assert_eq!(interpolate("$${who}", &look).unwrap(), "${who}");
        assert!(interpolate("${nope}", &look).unwrap_err().contains("unknown variable ${nope}"));
        // `\$` in quotes keeps the dollar literal.
        let w = tokenize(r#""\${who}""#).unwrap();
        assert_eq!(interpolate(&w[0], &look).unwrap(), "${who}");
    }

    #[test]
    fn a_file_parses_into_blocks_and_steps() {
        let text = "# shop\nbase http://127.0.0.1:8000\nset coupon = SAVE10\n\nbefore each\n  goto /shop\n\nscenario \"Add one\" @smoke @cart\n  click --text \"Add to cart\"\n  expect --selector #cart-count text is 1\n  save n = get text --selector #cart-count\n\nscenario \"Docs\"\n  goto /docs\n";
        let s = parse(Path::new("t.bt"), text).unwrap();
        assert_eq!(s.base.as_deref(), Some("http://127.0.0.1:8000"));
        assert_eq!(s.vars["coupon"], "SAVE10");
        assert_eq!(s.before_each.len(), 1);
        assert_eq!(s.scenarios.len(), 2);
        assert_eq!(s.scenarios[0].tags, ["smoke", "cart"]);
        assert_eq!(s.scenarios[0].steps[0].line, 9);
        assert!(matches!(s.scenarios[0].steps[1].action, Action::Expect(_)));
        assert!(matches!(&s.scenarios[0].steps[2].action, Action::Save { name, .. } if name == "n"));
        let p = parse(Path::new("p.bt"), "scenario \"p\"\n  print get text --testid total\n").unwrap();
        assert!(matches!(&p.scenarios[0].steps[0].action, Action::Print(w) if w[0] == "get" && w[3] == "total"));
        assert!(parse(Path::new("p.bt"), "scenario \"p\"\n  print\n").unwrap_err().to_string().contains("print needs"));
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        let e = parse(Path::new("t.bt"), "scenario \"A\"\n  click 12\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("refs"), "{e}");
        let e = parse(Path::new("t.bt"), "  goto /\n").unwrap_err();
        assert!(e.message.contains("outside"), "{e}");
        let e = parse(Path::new("t.bt"), "scenario \"A\"\n  goto /\nscenario \"A\"\n  goto /\n").unwrap_err();
        assert_eq!(e.line, 3);
        let e = parse(Path::new("t.bt"), "scenario \"Empty\"\n").unwrap_err();
        assert!(e.message.contains("no steps"), "{e}");
        let e = parse(Path::new("t.bt"), "scenario \"A\"\n  expect url sounds-like x\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn variables_reach_values_checked_at_parse_time() {
        // A flow's arguments fill a count and a regex; they're checked when filled in (T-0058).
        let text = "flow rows(n, re)\n  expect --selector tr count is ${n}\n  expect title matches ${re}\n  expect request GET /api/* count >= ${n}\nscenario \"x\"\n  run rows 3 ^Shop\n";
        let mut s = parse(Path::new("t.bt"), text).unwrap();
        resolve(std::slice::from_mut(&mut s)).unwrap();
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  expect --selector tr count is three\n").is_err());
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  expect title matches (\n").is_err());
    }

    #[test]
    fn exec_takes_its_checks_under_it_and_runs_beside_the_file() {
        let text = "scenario \"x\"\n  exec --timeout-ms 900 ./seed.sh ${who}\n    expect exit 0\n  exec psql -c \"select 1\"\n  expect ax matches\n    - button \"Go\"\n\n    - link\n  goto /\n";
        let mut s = parse(Path::new("dir/t.bt"), text).unwrap();
        resolve(std::slice::from_mut(&mut s)).unwrap();
        let steps = &s.scenarios[0].steps;
        match &steps[0].action {
            Action::Exec { argv, timeout_ms, dir, steps } => {
                assert_eq!(argv[0], std::path::absolute("dir/./seed.sh").unwrap().to_string_lossy());
                assert_eq!(argv[1], "${who}");
                assert_eq!(*timeout_ms, Some(900));
                assert_eq!(dir, Path::new("dir"));
                assert_eq!(steps.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&steps[1].action, Action::Exec { argv, steps, .. } if argv[0] == "psql" && steps.is_empty()));
        assert!(matches!(
            &steps[2].action,
            Action::Expect(Expect { subject: crate::expect::Subject::Ax { tree, .. }, .. }) if tree.len() == 3
        ));
        assert!(matches!(&steps[3].action, Action::Command(w) if w[0] == "goto"));
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  exec\n").is_err());
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  if ax matches\n    goto /\n").is_err());
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  expect ax matches\n  goto /\n").is_err());
    }

    #[test]
    fn flows_rows_and_optional_steps() {
        let text = "flow sign-in(email, password)\n  goto /login\n  fill --label Email ${email}\n\nscenario \"Ship to ${country}\"\n  examples\n    | country | fee |\n    | Canada  | $9  |\n    | Mexico  | $12 |\n  run sign-in a@b.c pw\n  optional click --selector #banner\n  expect text ${fee}\n";
        let mut s = parse(Path::new("t.bt"), text).unwrap();
        assert_eq!(s.flows["sign-in"].params, ["email", "password"]);
        assert_eq!(
            s.scenarios.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Ship to Canada", "Ship to Mexico"]
        );
        assert_eq!(s.scenarios[1].vars["fee"], "$12");
        assert!(
            matches!(&s.scenarios[0].steps[0].action, Action::Run { flow, args } if flow == "sign-in" && args.len() == 2)
        );
        assert!(matches!(&s.scenarios[0].steps[1].action, Action::Optional(_)));
        resolve(std::slice::from_mut(&mut s)).unwrap();
        // A wrong flow call is a file error, with its line.
        let mut bad = parse(Path::new("t.bt"), "flow f(a)\n  goto /\nscenario \"x\"\n  run f\n").unwrap();
        let e = resolve(std::slice::from_mut(&mut bad)).unwrap_err();
        assert_eq!(e.line, 4);
        assert!(e.message.contains("takes 1 argument"), "{e}");
        let mut bad = parse(Path::new("t.bt"), "scenario \"x\"\n  run nowhere\n").unwrap();
        assert!(resolve(std::slice::from_mut(&mut bad)).unwrap_err().message.contains("no flow named"));
        // Rows that would give two scenarios one name.
        let e = parse(
            Path::new("t.bt"),
            "scenario \"Same ${a}\"\n  examples\n    | a | b |\n    | 1 | x |\n    | 1 | y |\n  goto /\n",
        )
        .unwrap_err();
        assert!(e.message.contains("two data rows"), "{e}");
        // A name without a column gets the row's number.
        let s =
            parse(Path::new("t.bt"), "scenario \"Plain\"\n  examples\n    | a |\n    | 1 |\n    | 2 |\n  goto /${a}\n")
                .unwrap();
        assert_eq!(s.scenarios[1].name, "Plain [2]");
    }

    #[test]
    fn files_a_step_reads_are_found_next_to_the_suite() {
        let dir = Path::new("suites").join("shop");
        let text =
            "scenario \"Files\"\n  upload --selector \"#f\" a.txt --file b.txt /abs/c.txt\n  upload #g d.txt\n  \
                    upload --frame iframe e.txt\n  optional state load s.json\n  eval --file x.js\n  \
                    route **/api --body-file body.json\n  upload --file ${download.path}\n  eval -\n  \
                    fill --selector a.txt b.txt\n  route **/api/* --har api.har --not-found continue\n";
        let suite = parse(&dir.join("t.bt"), text).unwrap();
        let at = |p: &str| dir.join(p).to_string_lossy().into_owned();
        let words: Vec<Vec<String>> = suite.scenarios[0]
            .steps
            .iter()
            .map(|s| match &s.action {
                Action::Command(w) => w.clone(),
                Action::Optional(a) => match a.as_ref() {
                    Action::Command(w) => w.clone(),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            })
            .collect();
        let abs = if cfg!(windows) { at("/abs/c.txt") } else { "/abs/c.txt".to_string() };
        assert_eq!(words[0], ["upload", "--selector", "#f", &at("a.txt"), "--file", &at("b.txt"), &abs]);
        assert_eq!(words[1], ["upload", "#g", &at("d.txt")]);
        assert_eq!(words[2], ["upload", "--frame", "iframe", &at("e.txt")]);
        assert_eq!(words[3], ["state", "load", &at("s.json")]);
        assert_eq!(words[4], ["eval", "--file", &at("x.js")]);
        assert_eq!(words[5], ["route", "**/api", "--body-file", &at("body.json")]);
        assert_eq!(words[6], ["upload", "--file", "${download.path}"]);
        assert_eq!(words[7], ["eval", "-"]);
        assert_eq!(words[8], ["fill", "--selector", "a.txt", "b.txt"]);
        assert_eq!(words[9], ["route", "**/api/*", "--har", &at("api.har"), "--not-found", "continue"]);
        // The step's text stays as written, for messages.
        assert!(suite.scenarios[0].steps[0].text.contains(" a.txt "));
    }

    #[test]
    fn csv_and_use_files() {
        let rows = csv_rows("email,password\nada@x.io,\"p,w\"\n\"bo \"\"b\"\"\",z\n").unwrap();
        assert_eq!(rows[0]["password"], "p,w");
        assert_eq!(rows[1]["email"], "bo \"b\"");
        assert!(csv_rows("a,b\n1\n").is_err());
        let dir = std::env::temp_dir().join(format!("bt-use-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("flows")).unwrap();
        std::fs::write(dir.join("flows/auth.bt"), "set who = ada\nflow hello\n  goto /\n").unwrap();
        std::fs::write(dir.join("logins.csv"), "user\nada\nbo\n").unwrap();
        let main = dir.join("main.bt");
        std::fs::write(&main, "use flows/auth.bt\nscenario \"Hi ${user}\" for each logins.csv @data\n  run hello\n")
            .unwrap();
        let mut s = parse(&main, &std::fs::read_to_string(&main).unwrap()).unwrap();
        resolve(std::slice::from_mut(&mut s)).unwrap();
        assert_eq!(s.scenarios.len(), 2);
        assert_eq!(s.scenarios[0].tags, ["data"]);
        assert_eq!(s.vars["who"], "ada");
        assert!(s.flows.contains_key("hello"));
    }

    #[test]
    fn a_file_many_suites_use_is_read_once_per_run() {
        // 40 suites sharing one 200-step file read and parsed it 40 times
        // (9.0 s); the run reads each `use`d file once, nested ones too.
        let dir = std::env::temp_dir().join(format!("bt-use-once-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shared.bt"), "use more.bt\nflow open\n  goto /\n").unwrap();
        std::fs::write(dir.join("more.bt"), "set who = ada\nflow deep\n  goto /deep\n").unwrap();
        let mut suites: Vec<Suite> = (0..40)
            .map(|i| {
                let file = dir.join(format!("s{i}.bt"));
                let text = format!("use shared.bt\nscenario \"s{i}\"\n  run open\n  run deep\n");
                std::fs::write(&file, &text).unwrap();
                parse(&file, &text).unwrap()
            })
            .collect();
        USE_READS.with(|n| n.set(0));
        resolve(&mut suites).unwrap();
        assert_eq!(USE_READS.with(|n| n.get()), 2, "shared.bt and more.bt, once each for the run");
        for s in &suites {
            assert!(
                s.flows.contains_key("open") && s.flows.contains_key("deep") && s.vars["who"] == "ada",
                "{:?}",
                s.file
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_files_variable_under_a_scenario_says_where_it_goes() {
        let e = parse(Path::new("t.bt"), "scenario \"x\"\n  set who = ada\n  goto /\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.message.contains("`set who = …` goes at the top of the file, not indented"), "{}", e.message);
        // The browser's own `set` is a step.
        assert!(parse(Path::new("t.bt"), "scenario \"x\"\n  set viewport 800x600\n  goto /\n").is_ok());
        // What navigera refuses from the words alone fails in lint too. An
        // older navigera (CI builds against `master` too) refuses these only
        // in the run, so there is nothing for lint to see.
        for (step, said) in
            [("click --text Add --nth 1", "--text takes the one best match"), ("set WHO x", "unknown setting \"WHO\"")]
        {
            let words: Vec<String> = step.split(' ').map(String::from).collect();
            if crate::runner::parse(&words).is_ok() {
                continue;
            }
            let mut s = vec![parse(Path::new("t.bt"), &format!("scenario \"x\"\n  {step}\n")).unwrap()];
            let e = resolve(&mut s).unwrap_err();
            assert_eq!(e.line, 2);
            assert!(e.message.contains(said), "{step}: {}", e.message);
        }
    }

    #[test]
    fn mistakes_in_the_file_say_where_and_what_to_do() {
        let fails = |text: &str| -> ParseError {
            match parse(Path::new("t.bt"), text) {
                Err(e) => e,
                Ok(s) => resolve(&mut [s]).unwrap_err(),
            }
        };
        let e = fails("base http://a\nbase http://b\nscenario \"x\"\n  goto /\n");
        assert_eq!((e.line, e.message.as_str()), (2, "a second `base` (the first is at line 1); keep one"));
        let e = fails("scenario \"x\"\n  goto /\nscenario \"x\"\n  goto /\n");
        assert_eq!(e.message, "a second scenario named \"x\" (the first is at line 1); rename one");
        let e = fails("use nowhere.bt\nscenario \"x\"\n  goto /\n");
        assert_eq!(e.message, "use nowhere.bt: no such file (a name is found next to this .bt)");
        for step in ["expect js --file nofile.js", "optional expect js --file nofile.js", "if js --file nofile.js"] {
            let under = if step.starts_with("if") { "    " } else { "  " };
            let e = fails(&format!("scenario \"x\"\n  {step}\n{under}goto /\n"));
            assert_eq!(e.message, "js --file nofile.js: no such file (a script is found next to the .bt)", "{step}");
        }
        // `if expect …` is the same as `if …`.
        let with = parse(Path::new("t.bt"), "scenario \"x\"\n  if expect text \"Hi\"\n    goto /\n").unwrap();
        let without = parse(Path::new("t.bt"), "scenario \"x\"\n  if text \"Hi\"\n    goto /\n").unwrap();
        assert_eq!(
            format!("{:?}", with.scenarios[0].steps[0].action),
            format!("{:?}", without.scenarios[0].steps[0].action)
        );
    }

    #[test]
    fn lint_finds_variables_nothing_sets() {
        let text = "set a = 1\nflow f\n  save got = get text --selector h1\nscenario \"x\"\n  goto /${a}\n  goto /${row}\n  run f\n  expect ${got} is ${b:-2}\n  expect ${missing} is 1\n  save later = get title\n  goto /${later}\n";
        let s = parse(Path::new("t.bt"), text).unwrap();
        let un = unset_vars(&s, &s.scenarios[0], &[]);
        assert_eq!(un, [(6, "row".to_string()), (9, "missing".to_string())]);
        assert_eq!(unset_vars(&s, &s.scenarios[0], &["row".into(), "missing".into()]), []);
        assert_eq!(used_vars("$${x}-${y}-${z:-1}"), [("y".to_string(), false), ("z".to_string(), true)]);
    }

    #[test]
    fn a_quoted_target_is_visible_text_and_css_stays_css() {
        let words = |line: &str| match parse_step(line).unwrap() {
            Action::Command(w) => w,
            a => panic!("{a:?}"),
        };
        assert_eq!(words("click \"Buy now\""), ["click", "--text", "Buy now"]);
        assert_eq!(words("fill \"Email\" a@b.c"), ["fill", "--text", "Email", "a@b.c"]);
        assert_eq!(words("click Café"), ["click", "--text", "Café"]);
        for css in ["click button", "click #save", "click \".nav a\"", "click \"[data-x=1]\"", "click --text Go"] {
            assert_eq!(words(css), tokenize(css).unwrap(), "{css}");
        }
    }

    /// `lint` (and a run, before its browser starts) stops at a step
    /// navigera has no command for, wherever the step is.
    #[test]
    fn a_misspelled_step_is_a_file_error() {
        let check = |text: &str| {
            let mut suites = vec![parse(Path::new("t.bt"), text).unwrap()];
            resolve(&mut suites)
        };
        let e = check("scenario \"x\"\n  goto /\n  clik --text Go\n").unwrap_err();
        assert_eq!(e.line, 3);
        assert!(e.message.contains("unknown command \"clik\""), "{e}");
        assert!(e.message.contains("click"), "{e}");
        let e = check("flow f\n  optional clik --text Go\nscenario \"x\"\n  run f\n").unwrap_err();
        assert_eq!(e.line, 2);
        let e = check("scenario \"x\"\n  save t = gett title\n").unwrap_err();
        assert_eq!(e.line, 2, "{e}");
        // Variables stand in for values; `save … = json` is bitti's own.
        check("set w = 800\nscenario \"x\"\n  goto /${w}\n  click --text ${w}\n  save t = get title\n  save id = json $.id\n")
            .unwrap();
    }

    /// A steps file `use`d by other .bt files is one definition of each of
    /// its steps, however many files bring it in; the same pattern written
    /// in two places is still two matches.
    #[test]
    fn a_used_steps_file_defines_each_step_once() {
        let dir = std::env::temp_dir().join(format!("bt-steps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let steps = "step \"I open the cart\"\n  goto /cart\n";
        std::fs::write(dir.join("steps.bt"), steps).unwrap();
        let mut suites = vec![parse(&dir.join("steps.bt"), steps).unwrap()];
        for name in ["a.bt", "b.bt"] {
            let text = "use steps.bt\n\nscenario \"s\"\n  goto /\n";
            std::fs::write(dir.join(name), text).unwrap();
            suites.push(parse(&dir.join(name), text).unwrap());
        }
        let feature = "Feature: f\n  Scenario: s\n    When I open the cart\n";
        suites.push(crate::gherkin::parse(&dir.join("f.feature"), feature).unwrap());
        let mut twice = suites.clone();
        resolve(&mut suites).unwrap();
        assert!(
            matches!(&suites[3].scenarios[0].steps[0].action, Action::Run { flow, .. } if flow == "step \"I open the cart\"")
        );
        twice.push(parse(&dir.join("more.bt"), steps).unwrap());
        let e = resolve(&mut twice).unwrap_err();
        assert!(e.message.contains("matches more than one step"), "{}", e.message);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A steps file `use`d by many .bt files is parsed for each of them,
    /// and its `step` headers compile no pattern while parsed: compiling
    /// there cost one regex per step per user (seconds for a large suite).
    #[test]
    fn a_steps_file_used_by_many_files_is_quick() {
        let dir = std::env::temp_dir().join(format!("bt-many-uses-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut steps = String::new();
        for d in 0..100 {
            steps.push_str(&format!(
                "step \"I do thing {d} with {{value}} and {{other}}\"\n  click --text \"Go {d}\"\n"
            ));
        }
        std::fs::write(dir.join("steps.bt"), &steps).unwrap();
        let mut suites = vec![parse(&dir.join("steps.bt"), &steps).unwrap()];
        for k in 0..60 {
            let text = format!("use steps.bt\n\nscenario \"s{k}\"\n  goto /\n");
            suites.push(parse(&dir.join(format!("u{k}.bt")), &text).unwrap());
        }
        let started = std::time::Instant::now();
        resolve(&mut suites).unwrap();
        let took = started.elapsed();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(suites[60].flows.len(), 100);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    /// The run's step patterns are compiled once, not once per .feature
    /// file: a suite of hundreds of features started seconds late.
    #[test]
    fn many_features_resolve_their_steps_quickly() {
        let mut defs = String::new();
        for d in 0..100 {
            defs.push_str(&format!("step \"I do thing {d} with {{value}}\"\n  click --text \"Go {d}\"\n"));
        }
        let mut suites = vec![parse(Path::new("steps.bt"), &defs).unwrap()];
        for k in 0..200 {
            let text = format!("Feature: f{k}\n  Scenario: s{k}\n    When I do thing {} with \"v\"\n", k % 100);
            suites.push(crate::gherkin::parse(&PathBuf::from(format!("f{k}.feature")), &text).unwrap());
        }
        let started = std::time::Instant::now();
        resolve(&mut suites).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
        let step = &suites[200].scenarios[0].steps[0].action;
        assert!(
            matches!(step, Action::Run { flow, args } if flow == "step \"I do thing 99 with {value}\"" && args == &["v"])
        );
    }

    #[test]
    fn if_blocks_nest_by_indentation() {
        let text = "scenario \"x\"\n  goto /\n  if --selector \"#banner\" visible\n    click --selector \"#close\"\n    if text \"More\"\n      click --text More\n  expect title is t\n";
        let s = parse(Path::new("t.bt"), text).unwrap();
        let steps = &s.scenarios[0].steps;
        assert_eq!(steps.len(), 3);
        let Action::If { cond, steps: inner } = &steps[1].action else { panic!("{:?}", steps[1]) };
        assert_eq!(cond.timeout_ms, Some(0));
        assert_eq!(inner.len(), 2);
        assert!(matches!(&inner[1].action, Action::If { steps, .. } if steps.len() == 1));
        let e = parse(Path::new("t.bt"), "scenario \"x\"\n  goto /\n    click --text a\n").unwrap_err();
        assert_eq!(e.line, 3);
        let e = parse(Path::new("t.bt"), "scenario \"x\"\n  if text a\n  goto /\n").unwrap_err();
        assert!(e.message.contains("needs the steps"), "{e}");
    }

    #[test]
    fn graphql_requests_parse() {
        let s = parse(
            Path::new("/t/x.bt"),
            "scenario \"g\"\n  http POST /graphql --graphql \"query($id: ID!) { p(id: $id) { id } }\" --variables '{\"id\": \"1\"}' --operation Q\n  http POST /graphql --graphql-file q.graphql\n",
        )
        .unwrap();
        let Action::Http { graphql: Some(g), json, .. } = &s.scenarios[0].steps[0].action else { panic!() };
        assert_eq!(
            (g.query.as_str(), g.file, g.variables.as_deref(), g.operation.as_deref(), *json),
            ("query($id: ID!) { p(id: $id) { id } }", false, Some("{\"id\": \"1\"}"), Some("Q"), true)
        );
        let Action::Http { graphql: Some(g), .. } = &s.scenarios[0].steps[1].action else { panic!() };
        assert!(g.file && g.query.ends_with("q.graphql"));
        for bad in [
            "http POST /g --graphql x --json {}",
            "http POST /g --variables {}",
            "http POST /g --graphql x --variables [1]",
        ] {
            assert!(parse(Path::new("/t/x.bt"), &format!("scenario \"b\"\n  {bad}\n")).is_err(), "{bad}");
        }
    }

    #[test]
    fn http_takes_forms_queries_and_multipart_parts() {
        let text = "scenario \"s\"\n  http POST /login --form user=ada --form \"pw=${secret:PW}\" --query next=/home\n  http POST /up --multipart note=hi --multipart file=@data/a.png\n";
        let s = parse(Path::new("/t/x.bt"), text).unwrap();
        let Action::Http { form, query, .. } = &s.scenarios[0].steps[0].action else { panic!() };
        assert_eq!(form, &[("user".to_string(), "ada".to_string()), ("pw".into(), "${secret:PW}".into())]);
        assert_eq!(query, &[("next".to_string(), "/home".to_string())]);
        let Action::Http { multipart, .. } = &s.scenarios[0].steps[1].action else { panic!() };
        // A file part is found beside the .bt.
        assert_eq!(multipart[1].1, format!("@{}", Path::new("/t").join("data/a.png").display()));
        assert_eq!(multipart[0].1, "hi");
        for bad in ["http POST /a --form nope", "http POST /a --form a=1 --json '{}'", "http POST /a --multipart =x"] {
            assert!(parse(Path::new("x.bt"), &format!("scenario \"s\"\n  {bad}\n")).is_err(), "{bad}");
        }
    }

    #[test]
    fn retry_and_pause_are_steps() {
        let text = "scenario \"s\"\n  retry --timeout-ms 9000\n    http GET /job\n      expect json $.status is done\n  pause\n";
        let s = parse(Path::new("x.bt"), text).unwrap();
        let steps = &s.scenarios[0].steps;
        let Action::Retry { timeout_ms, steps: inner } = &steps[0].action else { panic!("{:?}", steps[0]) };
        assert_eq!(*timeout_ms, Some(9000));
        assert!(matches!(&inner[0].action, Action::Http { steps, .. } if steps.len() == 1));
        assert_eq!(steps[1].action, Action::Pause);
        assert!(s.has_pause(&s.scenarios[0]));
        let e = parse(Path::new("x.bt"), "scenario \"s\"\n  retry\n  goto /\n").unwrap_err();
        assert!(e.message.contains("`retry` needs the steps"), "{e}");
        assert!(parse(Path::new("x.bt"), "scenario \"s\"\n  retry --every 1\n    goto /\n").is_err());
    }

    #[test]
    fn a_scenario_of_http_and_exec_alone_needs_no_browser() {
        let text = "\
flow sign_in(u)
  http POST /login --form user=${u}
flow look()
  goto /account
auth api
  http POST /login --form user=a
auth ui
  goto /login
scenario \"api\"
  run sign_in ada
  as api
  http GET /me
    expect status 200
    expect json $.id is 7
  save id = json $.id
  expect ${id} is 7
  expect cookie sid
  if json $.admin is true
    http GET /admin
  retry
    exec ./check.sh
      expect exit 0
  print json $.id
  print \"id ${id}\"
  wait --ms 10
  optional expect time < 2s
  pause
scenario \"ui flow\"
  run look
scenario \"ui auth\"
  as ui
scenario \"a page check\"
  http GET /me
  expect text Hi
scenario \"a read\"
  save t = get title
scenario \"a page print\"
  print url
";
        let s = parse(Path::new("x.bt"), text).unwrap();
        let needs: Vec<bool> = s.scenarios.iter().map(|sc| s.scenario_needs_browser(sc)).collect();
        assert_eq!(needs, [false, true, true, true, true, true]);
        // A `before each` that opens a page puts every scenario in a browser.
        let s = parse(Path::new("x.bt"), "before each\n  goto /\nscenario \"s\"\n  http GET /me\n").unwrap();
        assert!(s.scenario_needs_browser(&s.scenarios[0]));
        let s = parse(Path::new("x.bt"), "before all\n  goto /\nscenario \"s\"\n  http GET /me\n").unwrap();
        assert!(!s.scenario_needs_browser(&s.scenarios[0]) && s.job_needs_browser(&s.scenarios[0]));
    }
}
