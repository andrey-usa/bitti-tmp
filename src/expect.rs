//! `expect` lines: what to check, how to read it from the page, and how to
//! compare. Every expectation is retried until it holds or its timeout
//! runs out (see `runner`): an assertion waits for the page instead of failing at once.
//!
//! ```text
//! expect url ends-with /cart
//! expect title contains Shop
//! expect text "Thank you"              (visible text, frames included)
//! expect no text "Error"
//! expect --role button --name Pay enabled
//! expect --testid total text is "$36.00"
//! expect --label Email value contains @
//! expect --selector .card count >= 3
//! expect --selector #faq attr open is ""
//! expect --testid badge style color is "rgb(0, 128, 0)"
//! expect ${order} matches "^ORD-[0-9]+$"
//! expect request POST /api/orders json $.total is 36
//! expect download report-*.csv
//! expect no errors
//! expect lcp < 2.5s                      (also fcp, ttfb, load, inp in ms or s; cls)
//! expect accessible [--selector main] [--tags wcag2aa] [--ignore color-contrast]
//! expect not --selector .toast visible   (any expectation can be negated)
//! expect text "Saved" --soft             (noted, the scenario goes on, fails at the end)
//! ```

#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    Url,
    Title,
    /// Visible text anywhere on the page, or inside the `--within` target.
    Text {
        text: String,
        within: Option<Vec<String>>,
    },
    /// A variable (the word as written, `${…}`), interpolated at run time.
    Var(String),
    /// An element (navigera target flags) and what to read from it.
    Target {
        flags: Vec<String>,
        prop: Prop,
    },
    /// Page exceptions and console errors.
    Errors,
    /// A JavaScript expression in the page: truthy, or compared.
    Js(String),
    /// `expect js --file <f.js>`: a script in a file beside the `.bt` (found
    /// there as `eval --file` is), for a check too long to quote on one line.
    JsFile(String),
    /// `expect js --main <code>`: in an Electron app's main process (a
    /// function gets the `electron` module), with `run --electron`.
    JsMain(String),
    /// A request the page made (since the scenario started): method (or
    /// `*`), URL glob, an optional status, and what to compare: nothing
    /// (one was made), a value in its JSON response, or how many.
    Request {
        method: String,
        url: String,
        status: Option<String>,
        read: RequestRead,
    },
    /// A cookie of the browser context: present, or its value compared.
    Cookie(String),
    /// `storage local|session <key>`: present, or its value compared.
    Storage {
        area: String,
        key: String,
    },
    /// A finished download whose file name matches the glob.
    Download(String),
    /// `download <glob> compare <expected> <compara flags…>`: and compara
    /// finds it the same as the expected file (beside the `.bt`).
    DownloadCompare {
        glob: String,
        expected: String,
        flags: Vec<String>,
        /// `--report html,json,rows` (or `none`): what compara writes to the
        /// evidence directory; the run's `--compare-report` when not given.
        report: Option<Vec<String>>,
    },
    /// A console message (any level) compared: `console contains "x"`.
    Console,
    /// `ax [<target>] matches` and the tree indented under it (see
    /// [`crate::axtree`]): the lines as written, filled in at run time.
    Ax {
        flags: Vec<String>,
        tree: Vec<String>,
    },
    /// The last `http` answer: a value in its JSON, or a header.
    Http(HttpRead),
    /// `json [<$.path>] schema <file.json>`: the last `http` answer's JSON
    /// (or the value at the path) is valid against a JSON Schema beside
    /// the `.bt` (see [`crate::schema`]).
    JsonSchema {
        path: String,
        file: String,
    },
    /// `json [<$.path>] snapshot <name> [--ignore <$.path>,…]`: the last
    /// `http` answer's JSON (or the value at the path), the `--ignore`d
    /// values left out, is what its baseline beside the `.bt` holds
    /// (`<file>.bt.snapshots/<name>.json`).
    JsonSnapshot {
        path: String,
        name: String,
        ignore: Vec<String>,
    },
    /// `screenshot <name> [<target>]`: the page (or an element) as its
    /// baseline PNG beside the `.bt` file shows it.
    Screenshot(Shot),
    /// A measure of the current page's load (see [`METRICS`]): its value in
    /// milliseconds (`cls`: the layout shift score).
    Metric(String),
    /// `accessible`: axe-core finds no violation of the tagged rules in
    /// the page (or under a selector), the ignored rules aside.
    Accessible {
        selector: Option<String>,
        tags: Vec<String>,
        ignore: Vec<String>,
    },
}

/// The WCAG levels `expect accessible` checks unless `--tags` says.
pub const A11Y_TAGS: &[&str] = &["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"];

/// `expect <metric> <comparison> <value>`: the page's Core Web Vitals and
/// load times, as the browser measured them for the current document.
pub const METRICS: &[&str] = &["lcp", "fcp", "ttfb", "load", "cls", "inp"];

/// A metric's limit as a number: milliseconds for the times (`2.5s`,
/// `800ms`, a bare number is ms), the score itself for `cls`.
fn metric_value(metric: &str, raw: &str) -> Result<String, String> {
    let bad = || match metric {
        "cls" => format!("cls takes a score like 0.1, not {raw:?}"),
        _ => format!("{metric} takes a time like 2.5s or 800ms, not {raw:?}"),
    };
    let (num, scale) = match (metric, raw.strip_suffix("ms"), raw.strip_suffix('s')) {
        ("cls", _, _) => (raw, 1.0),
        (_, Some(n), _) => (n, 1.0),
        (_, None, Some(n)) => (n, 1000.0),
        _ => (raw, 1.0),
    };
    let v: f64 = num.trim().parse().map_err(|_| bad())?;
    if !v.is_finite() || v < 0.0 {
        return Err(bad());
    }
    Ok((v * scale).to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub name: String,
    /// navigera target flags; none for the viewport.
    pub flags: Vec<String>,
    pub full_page: bool,
    /// Colour distance two pixels may have and still match (0..1).
    pub threshold: f64,
    /// How many pixels may differ, and what share of them (0..1); each one
    /// given has to hold. With neither, none may.
    pub max_pixels: Option<u64>,
    pub max_ratio: Option<f64>,
    /// CSS selectors painted over before the shot (a clock, an ad).
    pub masks: Vec<String>,
}

impl Shot {
    /// Whether `pixels` differing of `total` is within what the step allows:
    /// each limit given (`--max-diff-pixels`, `--max-diff-ratio`) holds, and
    /// with none given no pixel may differ.
    pub fn allows(&self, pixels: u64, total: u64) -> bool {
        let by_count = self.max_pixels.is_none_or(|n| pixels <= n);
        let by_ratio = self.max_ratio.is_none_or(|r| pixels as f64 <= r * total as f64);
        let exact = self.max_pixels.is_none() && self.max_ratio.is_none();
        by_count && by_ratio && (!exact || pixels == 0)
    }

    /// What the step allows, for the failure line.
    pub fn allowed(&self) -> String {
        match (self.max_pixels, self.max_ratio) {
            (None, None) => "0 pixels".into(),
            (Some(n), None) => format!("{n} pixels"),
            (None, Some(r)) => format!("{:.2}%", r * 100.0),
            (Some(n), Some(r)) => format!("{n} pixels and {:.2}%", r * 100.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HttpRead {
    /// `json <$.path>`.
    Json(String),
    /// `json <$.path> count`: how many items the array (or keys the
    /// object) at the path has.
    Count(String),
    /// `header <name>` (any case).
    Header(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestRead {
    Made,
    /// `json <$.path>`: a value in the last matching response's JSON.
    Json(String),
    /// `body`: the last matching response's body, as text.
    Body,
    Count,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    /// visible, hidden, enabled, disabled, checked, unchecked, focused,
    /// editable, empty.
    State(String),
    Text,
    Value,
    Attr(String),
    /// A computed CSS property.
    Style(String),
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Matcher {
    Is,
    Contains,
    StartsWith,
    EndsWith,
    Matches,
    Gt,
    Ge,
    Lt,
    Le,
    /// A state or the presence of text: no value to compare.
    Holds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    pub negate: bool,
    pub subject: Subject,
    pub matcher: Matcher,
    pub value: String,
    pub timeout_ms: Option<u64>,
    /// `--soft`: a failure is noted and the scenario goes on; it fails at
    /// the end.
    pub soft: bool,
}

const STATES: &[&str] =
    &["visible", "hidden", "enabled", "disabled", "checked", "unchecked", "focused", "editable", "empty"];

/// The words of a JSON path: `$`, `.key`, `[0]`, `["key"]`.
pub fn json_path(path: &str) -> Result<Vec<JsonStep>, String> {
    let bad = || format!("bad JSON path {path:?} (like $.items[0].name)");
    let mut rest = path.strip_prefix('$').ok_or_else(bad)?;
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            if end == 0 {
                return Err(bad());
            }
            out.push(JsonStep::Key(r[..end].to_string()));
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').ok_or_else(bad)?;
            let inner = r[..end].trim();
            if let Ok(i) = inner.parse::<usize>() {
                out.push(JsonStep::Index(i));
            } else if inner.len() >= 2
                && (inner.starts_with('"') && inner.ends_with('"') || inner.starts_with('\'') && inner.ends_with('\''))
            {
                out.push(JsonStep::Key(inner[1..inner.len() - 1].to_string()));
            } else {
                return Err(bad());
            }
            rest = &r[end + 1..];
        } else {
            return Err(bad());
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonStep {
    Key(String),
    Index(usize),
}

/// The value at a JSON path.
pub fn json_value_at<'a>(v: &'a serde_json::Value, path: &[JsonStep]) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for st in path {
        cur = match st {
            JsonStep::Key(k) => cur.get(k)?,
            JsonStep::Index(i) => cur.get(*i)?,
        };
    }
    Some(cur)
}

/// The value at a JSON path, as text: strings as they are, the rest as JSON.
pub fn json_at(v: &serde_json::Value, path: &[JsonStep]) -> Option<String> {
    let cur = json_value_at(v, path)?;
    Some(match cur {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

fn matcher(word: &str) -> Option<Matcher> {
    Some(match word {
        "is" | "==" => Matcher::Is,
        "contains" => Matcher::Contains,
        "starts-with" => Matcher::StartsWith,
        "ends-with" => Matcher::EndsWith,
        "matches" => Matcher::Matches,
        ">" => Matcher::Gt,
        ">=" => Matcher::Ge,
        "<" => Matcher::Lt,
        "<=" => Matcher::Le,
        _ => return None,
    })
}

/// `<matcher> <value>` and nothing after it.
fn tail(words: &[String], what: &str) -> Result<(Matcher, String), String> {
    let m = words.first().and_then(|w| matcher(w)).ok_or_else(|| {
        format!(
            "after {what}: is, contains, starts-with, ends-with, matches, >, >=, <, <= (got {:?})",
            words.first().map(String::as_str).unwrap_or("nothing")
        )
    })?;
    match words.len() {
        2 => {
            // A `${var}` (a flow's or a step block's argument) is checked once
            // it is filled in.
            if m == Matcher::Matches && !words[1].contains("${") {
                regex::Regex::new(&words[1]).map_err(|e| format!("bad regular expression {:?}: {e}", words[1]))?;
            }
            Ok((m, words[1].clone()))
        }
        1 => Err(format!("{} needs a value", words[0])),
        _ => Err(format!("one value after {} (quote it if it has spaces): {:?}", words[0], &words[2..])),
    }
}

/// Parse the words after `expect`.
pub fn parse(words: &[String]) -> Result<Expect, String> {
    let mut e = parse_inner(words)?;
    if words.iter().any(|w| w == "--soft") {
        e.soft = true;
    }
    Ok(e)
}

fn parse_inner(words: &[String]) -> Result<Expect, String> {
    let mut words: Vec<String> = words.iter().filter(|w| *w != "--soft").cloned().collect();
    let soft = false;
    let mut timeout_ms = None;
    if let Some(i) = words.iter().position(|w| w == "--timeout-ms") {
        let v = words.get(i + 1).ok_or("--timeout-ms needs a number")?;
        timeout_ms = Some(v.parse::<u64>().map_err(|_| format!("--timeout-ms takes milliseconds, not {v:?}"))?);
        words.drain(i..i + 2);
    }
    let mut negate = false;
    if words.first().map(String::as_str) == Some("not") {
        negate = true;
        words.remove(0);
    }
    if words.first().map(String::as_str) == Some("no") {
        negate = !negate;
        words.remove(0);
        return match words.first().map(String::as_str) {
            Some("errors") if words.len() == 1 => {
                // `no errors`: the error list is empty.
                Ok(Expect {
                    negate: !negate,
                    subject: Subject::Errors,
                    matcher: Matcher::Holds,
                    value: String::new(),
                    timeout_ms,
                    soft,
                })
            }
            Some("text") if words.len() >= 2 => Ok(Expect {
                negate,
                subject: Subject::Text { text: words[1].clone(), within: within(&words[2..])? },
                matcher: Matcher::Holds,
                value: String::new(),
                timeout_ms,
                soft,
            }),
            Some("cookie") if words.len() == 2 => Ok(Expect {
                negate,
                subject: Subject::Cookie(words[1].clone()),
                matcher: Matcher::Holds,
                value: String::new(),
                timeout_ms,
                soft,
            }),
            Some("request") if words.len() == 3 => Ok(Expect {
                negate,
                subject: Subject::Request {
                    method: words[1].to_uppercase(),
                    url: words[2].clone(),
                    status: None,
                    read: RequestRead::Made,
                },
                matcher: Matcher::Holds,
                value: String::new(),
                timeout_ms,
                soft,
            }),
            Some("download") if words.len() == 2 => Ok(Expect {
                negate,
                subject: Subject::Download(words[1].clone()),
                matcher: Matcher::Holds,
                value: String::new(),
                timeout_ms,
                soft,
            }),
            Some("console") if words.len() == 2 => Ok(Expect {
                negate,
                subject: Subject::Console,
                matcher: Matcher::Contains,
                value: words[1].clone(),
                timeout_ms,
                soft,
            }),
            _ => Err("after `no`: `errors`, `text \"<text>\"`, `cookie <name>`, `request <METHOD> <url>`, `download <glob>` or `console \"<text>\"`".into()),
        };
    }
    let first = words.first().cloned().ok_or(
        "expect what? (url, title, text, a target, ${var}, js, request, cookie, storage, download, console, no errors, ax, screenshot, exit, stdout, stderr, status, body, json, header, time)",
    )?;
    // A subject with an optional comparison after it: present/truthy without
    // one, gone with `absent`.
    let optional_tail = |rest: &[String], what: &str| -> Result<(Matcher, String), String> {
        if rest.is_empty() {
            Ok((Matcher::Holds, String::new()))
        } else {
            tail(rest, what)
        }
    };
    let absent = match first.as_str() {
        "cookie" => words.len() == 3,
        "storage" => words.len() == 4,
        _ => false,
    };
    if absent && words.last().map(String::as_str) == Some("absent") {
        words.pop();
        negate = !negate;
    }
    let (subject, matcher, value) = match first.as_str() {
        // `eval` reads as navigera's own command does.
        // What the last `exec` or `http` gave: it doesn't change, so it isn't
        // waited for.
        "exit" | "stdout" | "stderr" | "status" | "body" => {
            timeout_ms = Some(timeout_ms.unwrap_or(0));
            let (m, v) = match &words[1..] {
                [n] if matches!(first.as_str(), "exit" | "status") && n.parse::<i64>().is_ok() => {
                    (Matcher::Is, n.clone())
                }
                rest => tail(rest, &first)?,
            };
            let from = if matches!(first.as_str(), "status" | "body") { "http" } else { "exec" };
            (Subject::Var(format!("${{{from}.{first}}}")), m, v)
        }
        "json" | "header" => {
            timeout_ms = Some(timeout_ms.unwrap_or(0));
            let what = words
                .get(1)
                .ok_or_else(|| {
                    format!("expect {first} needs a {}", if first == "json" { "$.path" } else { "header name" })
                })?
                .clone();
            // `expect json schema f.json`: the whole answer.
            let (what, rest) =
                if first == "json" && what == "schema" { ("$".to_string(), &words[1..]) } else { (what, &words[2..]) };
            // `expect json snapshot n`: the whole answer.
            let (what, rest) =
                if first == "json" && what == "snapshot" && rest.first().map(String::as_str) != Some("snapshot") {
                    ("$".to_string(), &words[1..])
                } else {
                    (what, rest)
                };
            if first == "json" && rest.first().map(String::as_str) == Some("snapshot") {
                json_path(&what)?;
                let usage = "expected `expect json [<$.path>] snapshot <name> [--ignore <$.path>,…]`";
                let name = rest.get(1).ok_or(usage)?.clone();
                if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                    return Err(format!(
                        "a snapshot name is letters, digits, - _ or . (it names the file), not {name:?}"
                    ));
                }
                let mut ignore = Vec::new();
                let mut more = rest[2..].iter();
                while let Some(flag) = more.next() {
                    if flag != "--ignore" {
                        return Err(format!("{usage} (got {flag:?})"));
                    }
                    let list = more.next().ok_or("--ignore needs a $.path (or several, comma-separated)")?;
                    for p in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        if !crate::schema::valid_ignore(p) {
                            return Err(format!("--ignore takes $.paths like $.id or $.items[*].id, not {p:?}"));
                        }
                        ignore.push(p.to_string());
                    }
                }
                (Subject::JsonSnapshot { path: what, name, ignore }, Matcher::Holds, String::new())
            } else if first == "json" && rest.first().map(String::as_str) == Some("schema") {
                json_path(&what)?;
                let [_, file] = rest else {
                    return Err("expected `expect json [<$.path>] schema <file.json>`".into());
                };
                (Subject::JsonSchema { path: what, file: file.clone() }, Matcher::Holds, String::new())
            } else if first == "json" && rest.first().map(String::as_str) == Some("count") {
                json_path(&what)?;
                let (m, v) = tail(&rest[1..], "count")?;
                (Subject::Http(HttpRead::Count(what)), m, v)
            } else {
                let read = if first == "json" {
                    json_path(&what)?;
                    HttpRead::Json(what)
                } else {
                    HttpRead::Header(what)
                };
                let (m, v) = optional_tail(rest, &first)?;
                (Subject::Http(read), m, v)
            }
        }
        // How long the last `http` exchange took.
        "time" => {
            timeout_ms = Some(timeout_ms.unwrap_or(0));
            let (m, v) = tail(&words[1..], "time")?;
            if !matches!(m, Matcher::Lt | Matcher::Le | Matcher::Gt | Matcher::Ge) {
                return Err("expect time takes <, <=, > or >= and a time (`expect time < 500ms`)".into());
            }
            let v = if v.contains("${") { v } else { metric_value("time", &v)? };
            (Subject::Var("${http.ms}".into()), m, v)
        }
        "screenshot" => {
            let usage = "expect screenshot <name> [<target>] [--full-page] [--threshold 0.2] [--max-diff-pixels <n>] [--max-diff-ratio <r>] [--mask <css>]…";
            let name = words.get(1).ok_or(usage)?.clone();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
                return Err(format!(
                    "a screenshot name is letters, digits, - _ or . (it names the file), not {name:?}"
                ));
            }
            let mut shot = Shot {
                name,
                flags: Vec::new(),
                full_page: false,
                threshold: 0.2,
                max_pixels: None,
                max_ratio: None,
                masks: Vec::new(),
            };
            let mut rest = words[2..].iter();
            while let Some(f) = rest.next() {
                if f == "--full-page" {
                    shot.full_page = true;
                    continue;
                }
                let v = rest.next().ok_or_else(|| format!("{f} needs a value ({usage})"))?.clone();
                let num = |v: &str| v.parse::<f64>().map_err(|_| format!("{f} takes a number, not {v:?}"));
                match f.as_str() {
                    "--threshold" => shot.threshold = num(&v)?.clamp(0.0, 1.0),
                    "--max-diff-pixels" => shot.max_pixels = Some(num(&v)? as u64),
                    "--max-diff-ratio" => shot.max_ratio = Some(num(&v)?),
                    "--mask" => shot.masks.push(v),
                    f if f.starts_with("--") => shot.flags.extend([f.to_string(), v]),
                    other => return Err(format!("{usage} (got {other:?})")),
                }
            }
            (Subject::Screenshot(shot), Matcher::Holds, String::new())
        }
        "ax" => {
            let usage = "expect ax [<target>] matches, then the tree it expects indented under it";
            if words.last().map(String::as_str) != Some("matches") {
                return Err(usage.into());
            }
            let flags = words[1..words.len() - 1].to_vec();
            if !flags.len().is_multiple_of(2) || !flags.iter().step_by(2).all(|f| f.starts_with("--")) {
                return Err(format!("{usage} (a target like --role form --name Checkout)"));
            }
            (Subject::Ax { flags, tree: Vec::new() }, Matcher::Holds, String::new())
        }
        "js" | "eval" if words.get(1).map(String::as_str) == Some("--file") => {
            let file = words.get(2).ok_or("expect js --file <script.js>: the file's name")?.clone();
            let (m, v) = optional_tail(&words[3..], "js")?;
            (Subject::JsFile(file), m, v)
        }
        "js" | "eval" if words.get(1).map(String::as_str) == Some("--main") => {
            let code = words.get(2).ok_or("expect js --main \"<expression or function>\": the code to run")?.clone();
            let (m, v) = optional_tail(&words[3..], "js")?;
            (Subject::JsMain(code), m, v)
        }
        "js" | "eval" => {
            let code = words.get(1).ok_or("expect js \"<expression>\", or expect js --file <script.js>")?.clone();
            let (m, v) = optional_tail(&words[2..], "js")?;
            (Subject::Js(code), m, v)
        }
        "request" => {
            // request <METHOD> <url glob> [status <n>] [json <$.path> | body | count <comparison> <value>]
            let usage =
                "expect request <METHOD> <url glob> [status <n>] [json <$.path> | body | count] [<comparison> <value>]";
            let method = words.get(1).ok_or(usage)?.to_uppercase();
            let url = words.get(2).ok_or(usage)?.clone();
            let mut rest = &words[3..];
            let mut status = None;
            if rest.first().map(String::as_str) == Some("status") {
                status = Some(rest.get(1).ok_or("status needs a number")?.clone());
                rest = &rest[2..];
            }
            match rest.first().map(String::as_str) {
                None => {
                    (Subject::Request { method, url, status, read: RequestRead::Made }, Matcher::Holds, String::new())
                }
                Some("json") => {
                    let path = rest.get(1).ok_or("json needs a path, like $.items[0].id")?.clone();
                    json_path(&path)?;
                    let (m, v) = tail(&rest[2..], "the JSON path")?;
                    (Subject::Request { method, url, status, read: RequestRead::Json(path) }, m, v)
                }
                Some("body") => {
                    let (m, v) = tail(&rest[1..], "the body")?;
                    (Subject::Request { method, url, status, read: RequestRead::Body }, m, v)
                }
                Some("count") => {
                    let (m, v) = tail(&rest[1..], "count")?;
                    if !v.contains("${") {
                        v.parse::<f64>().map_err(|_| format!("count compares with a number, not {v:?}"))?;
                    }
                    (Subject::Request { method, url, status, read: RequestRead::Count }, m, v)
                }
                Some(other) => {
                    return Err(format!(
                        "after the request: `status <n>`, `json <$.path> …`, `body …`, `count …` or nothing (got {other:?})"
                    ))
                }
            }
        }
        "download" => match words.len() {
            2 => (Subject::Download(words[1].clone()), Matcher::Holds, String::new()),
            n if n >= 4 && words[2] == "compare" => {
                let mut flags = words[4..].to_vec();
                let mut report = None;
                if let Some(i) = flags.iter().position(|f| f == "--report") {
                    let v = flags.get(i + 1).ok_or("--report takes html, json, rows (comma-separated) or none")?;
                    report = Some(report_kinds(v)?);
                    flags.drain(i..i + 2);
                }
                (
                    Subject::DownloadCompare { glob: words[1].clone(), expected: words[3].clone(), flags, report },
                    Matcher::Holds,
                    String::new(),
                )
            }
            _ => {
                return Err(
                    "expect download <file name glob> [compare <expected file> --key <columns> [compara flags…]]"
                        .into(),
                )
            }
        },
        "console" => {
            let (m, v) = tail(&words[1..], "console")?;
            (Subject::Console, m, v)
        }
        "cookie" => {
            let name = words.get(1).ok_or("expect cookie <name> [<comparison> <value>]")?.clone();
            let (m, v) = optional_tail(&words[2..], "cookie")?;
            (Subject::Cookie(name), m, v)
        }
        "storage" => {
            let usage = "expect storage local|session <key> [<comparison> <value>]";
            let area = words.get(1).filter(|a| *a == "local" || *a == "session").ok_or(usage)?.clone();
            let key = words.get(2).ok_or(usage)?.clone();
            let (m, v) = optional_tail(&words[3..], "storage")?;
            (Subject::Storage { area, key }, m, v)
        }
        "url" => {
            let (m, v) = tail(&words[1..], "url")?;
            (Subject::Url, m, v)
        }
        "accessible" => {
            let usage = "expect accessible [--selector <css>] [--tags wcag2a,wcag2aa,…] [--ignore <rule>,…]";
            let (mut selector, mut tags, mut ignore) = (None, Vec::new(), Vec::new());
            let mut rest = words[1..].iter();
            while let Some(w) = rest.next() {
                let list = |v: Option<&String>| -> Result<Vec<String>, String> {
                    let v = v.ok_or_else(|| format!("{w} needs a comma-separated list, like {w} wcag2a,wcag2aa"))?;
                    Ok(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                };
                match w.as_str() {
                    "--selector" => selector = Some(rest.next().ok_or("--selector needs a CSS selector")?.clone()),
                    "--tags" => tags.extend(list(rest.next())?),
                    "--ignore" => ignore.extend(list(rest.next())?),
                    _ => return Err(format!("{usage} (got {w:?})")),
                }
            }
            if tags.is_empty() {
                tags = A11Y_TAGS.iter().map(|t| t.to_string()).collect();
            }
            (Subject::Accessible { selector, tags, ignore }, Matcher::Holds, String::new())
        }
        metric if METRICS.contains(&metric) => {
            let (m, v) = tail(&words[1..], metric)?;
            if !matches!(m, Matcher::Lt | Matcher::Le | Matcher::Gt | Matcher::Ge) {
                return Err(format!(
                    "expect {metric} <, <=, > or >= a value (like `expect {metric} < {}`)",
                    if metric == "cls" { "0.1" } else { "2.5s" }
                ));
            }
            (Subject::Metric(metric.to_string()), m, metric_value(metric, &v)?)
        }
        "title" => {
            let (m, v) = tail(&words[1..], "title")?;
            (Subject::Title, m, v)
        }
        "text" => match words.len() {
            2.. => {
                (Subject::Text { text: words[1].clone(), within: within(&words[2..])? }, Matcher::Holds, String::new())
            }
            _ => return Err("expect text needs the text to look for: `expect text \"Thanks\"`".into()),
        },
        w if w.starts_with("${") => {
            let (m, v) = tail(&words[1..], w)?;
            (Subject::Var(w.to_string()), m, v)
        }
        w if w.starts_with("--") => {
            // Target flags: each takes a value.
            let mut i = 0;
            let mut flags = Vec::new();
            while i < words.len() && words[i].starts_with("--") {
                let v = words.get(i + 1).ok_or_else(|| format!("{} needs a value", words[i]))?;
                flags.push(words[i].clone());
                flags.push(v.clone());
                i += 2;
            }
            let rest = &words[i..];
            match rest.first().map(String::as_str) {
                Some(s) if STATES.contains(&s) && rest.len() == 1 => {
                    (Subject::Target { flags, prop: Prop::State(s.to_string()) }, Matcher::Holds, String::new())
                }
                Some("text") => {
                    let (m, v) = tail(&rest[1..], "text")?;
                    (Subject::Target { flags, prop: Prop::Text }, m, v)
                }
                Some("value") => {
                    let (m, v) = tail(&rest[1..], "value")?;
                    (Subject::Target { flags, prop: Prop::Value }, m, v)
                }
                Some("attr") => {
                    let name = rest.get(1).ok_or("attr needs a name")?.clone();
                    let (m, v) = tail(&rest[2..], "attr")?;
                    (Subject::Target { flags, prop: Prop::Attr(name) }, m, v)
                }
                Some("style") => {
                    let name = rest.get(1).ok_or("style needs a CSS property, like color")?.clone();
                    let (m, v) = tail(&rest[2..], "style")?;
                    (Subject::Target { flags, prop: Prop::Style(name) }, m, v)
                }
                Some("count") => {
                    if !flags.iter().step_by(2).all(|f| f == "--selector") || flags.len() != 2 {
                        return Err("count takes one --selector <css>".into());
                    }
                    let (m, v) = tail(&rest[1..], "count")?;
                    if !v.contains("${") {
                        v.parse::<f64>().map_err(|_| format!("count compares with a number, not {v:?}"))?;
                    }
                    (Subject::Target { flags, prop: Prop::Count }, m, v)
                }
                other => {
                    let said = match other {
                        Some(w) => format!("{w:?} isn't one"),
                        None => "nothing was said".to_string(),
                    };
                    return Err(format!(
                        "after the target, say what to check, e.g. `visible` or `text is \"Saved\"` ({said}). \
                         States: {}; or text, value, attr <name>, style <property> or count, then a comparison",
                        STATES.join(", "),
                    ));
                }
            }
        }
        other => {
            return Err(format!(
                "expect what? {other:?} (url, title, text, a target like --role button, ${{var}}, no errors)"
            ))
        }
    };
    Ok(Expect { negate, subject, matcher, value, timeout_ms, soft })
}

/// `html,json,rows` or `none`: which of compara's reports to keep.
pub fn report_kinds(v: &str) -> Result<Vec<String>, String> {
    let kinds: Vec<String> =
        v.split(',').map(|r| r.trim().to_string()).filter(|r| r != "none" && !r.is_empty()).collect();
    match kinds.iter().find(|r| !matches!(r.as_str(), "html" | "json" | "rows")) {
        Some(bad) => Err(format!("a compare report is html, json or rows (comma-separated), or none; not {bad:?}")),
        None => Ok(kinds),
    }
}

/// `--within <target flags>` after `text "<t>"`: where to look.
fn within(rest: &[String]) -> Result<Option<Vec<String>>, String> {
    match rest.first().map(String::as_str) {
        None => Ok(None),
        Some("--within") => {
            let flags = &rest[1..];
            if flags.is_empty()
                || !flags.len().is_multiple_of(2)
                || !flags.iter().step_by(2).all(|f| f.starts_with("--"))
            {
                return Err(
                    "--within takes a target: --within --testid toast, --within --role dialog --name Cart".into()
                );
            }
            Ok(Some(flags.to_vec()))
        }
        Some(other) => {
            Err(format!("after the text: `--within <target>` or nothing (quote text with spaces; got {other:?})"))
        }
    }
}

/// `*` matches any run of characters (`/api/orders/*`); everything else is
/// literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(k) => rest = &rest[k + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// Does `actual` satisfy `m` against `expected`?
pub fn compare(actual: &str, m: Matcher, expected: &str) -> bool {
    let num = |s: &str| s.trim().parse::<f64>().ok();
    match m {
        Matcher::Is => actual == expected || matches!((num(actual), num(expected)), (Some(a), Some(b)) if a == b),
        Matcher::Contains => actual.contains(expected),
        Matcher::StartsWith => actual.starts_with(expected),
        Matcher::EndsWith => actual.ends_with(expected),
        Matcher::Matches => regex::Regex::new(expected).is_ok_and(|r| r.is_match(actual)),
        Matcher::Gt | Matcher::Ge | Matcher::Lt | Matcher::Le => match (num(actual), num(expected)) {
            (Some(a), Some(b)) => match m {
                Matcher::Gt => a > b,
                Matcher::Ge => a >= b,
                Matcher::Lt => a < b,
                _ => a <= b,
            },
            _ => false,
        },
        Matcher::Holds => true,
    }
}

/// `compare` for a value read from JSON: an object or an array written as
/// JSON is compared as JSON, `is` whole (key order and `1` vs `1.0` aside)
/// and `contains` as a part of it (the keys it names, with values that
/// match the same way; for an array, items that some item matches).
/// Anything else is compared as text.
pub fn compare_json(actual: &str, m: Matcher, expected: &str) -> bool {
    let structured = |s: &str| {
        let t = s.trim_start();
        (t.starts_with('{') || t.starts_with('[')).then(|| serde_json::from_str::<serde_json::Value>(s).ok()).flatten()
    };
    match (m, structured(expected), serde_json::from_str::<serde_json::Value>(actual)) {
        (Matcher::Is, Some(want), Ok(got)) => json_equal(&got, &want),
        (Matcher::Contains, Some(want), Ok(got)) => json_contains(&got, &want),
        _ => compare(actual, m, expected),
    }
}

fn json_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => x.as_f64() == y.as_f64(),
        (Array(x), Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_equal(p, q)),
        (Object(x), Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| json_equal(v, w)))
        }
        _ => a == b,
    }
}

/// Whether `got` holds everything `want` says.
fn json_contains(got: &serde_json::Value, want: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match (got, want) {
        (Object(g), Object(w)) => w.iter().all(|(k, v)| g.get(k).is_some_and(|gv| json_contains(gv, v))),
        (Array(g), Array(w)) => w.iter().all(|wv| g.iter().any(|gv| json_contains(gv, wv))),
        // `[…] contains {…}`: one of its items does.
        (Array(g), Object(_)) => g.iter().any(|gv| json_contains(gv, want)),
        _ => json_equal(got, want),
    }
}

impl Matcher {
    pub fn word(self) -> &'static str {
        match self {
            Matcher::Is => "is",
            Matcher::Contains => "contains",
            Matcher::StartsWith => "starts-with",
            Matcher::EndsWith => "ends-with",
            Matcher::Matches => "matches",
            Matcher::Gt => ">",
            Matcher::Ge => ">=",
            Matcher::Lt => "<",
            Matcher::Le => "<=",
            Matcher::Holds => "",
        }
    }
}

#[cfg(test)]
mod tests {
    fn shot(max_pixels: Option<u64>, max_ratio: Option<f64>) -> Shot {
        Shot { name: "s".into(), flags: vec![], full_page: false, threshold: 0.2, max_pixels, max_ratio, masks: vec![] }
    }

    #[test]
    fn a_screenshot_allows_what_its_limits_allow() {
        // Neither limit: exact.
        assert!(shot(None, None).allows(0, 1000));
        assert!(!shot(None, None).allows(1, 1000));
        // A ratio alone (0.1% of 10 000 pixels is 10): just under, on it, just over.
        let r = shot(None, Some(0.001));
        assert!(r.allows(9, 10_000) && r.allows(10, 10_000) && !r.allows(11, 10_000));
        // A count alone.
        let n = shot(Some(5), None);
        assert!(n.allows(5, 10_000) && !n.allows(6, 10_000));
        // Both: each has to hold.
        let both = shot(Some(5), Some(0.001));
        assert!(both.allows(5, 10_000));
        assert!(!both.allows(6, 10_000), "over the count");
        assert!(!shot(Some(50), Some(0.001)).allows(11, 10_000), "over the ratio");
        assert_eq!(both.allowed(), "5 pixels and 0.10%");
        assert_eq!(r.allowed(), "0.10%");
        assert_eq!(n.allowed(), "5 pixels");
    }

    use super::*;

    fn p(line: &str) -> Result<Expect, String> {
        parse(&crate::suite::tokenize(line).unwrap())
    }

    #[test]
    fn subjects_and_matchers_parse() {
        let e = p("accessible --selector main --ignore color-contrast,region").unwrap();
        assert_eq!(
            e.subject,
            Subject::Accessible {
                selector: Some("main".into()),
                tags: A11Y_TAGS.iter().map(|t| t.to_string()).collect(),
                ignore: vec!["color-contrast".into(), "region".into()],
            }
        );
        assert!(p("accessible --strict").unwrap_err().contains("--tags"));
        let e = p("lcp < 2.5s").unwrap();
        assert_eq!((e.subject, e.matcher, e.value.as_str()), (Subject::Metric("lcp".into()), Matcher::Lt, "2500"));
        assert_eq!(p("ttfb <= 800ms").unwrap().value, "800");
        assert_eq!(p("inp < 200ms").unwrap().subject, Subject::Metric("inp".into()));
        assert_eq!(p("load < 3000").unwrap().value, "3000");
        assert_eq!(p("cls < 0.1").unwrap().value, "0.1");
        assert!(p("lcp is 2s").unwrap_err().contains("<, <="));
        assert!(p("fcp < fast").unwrap_err().contains("2.5s or 800ms"));
        let e = p("url ends-with /cart").unwrap();
        assert_eq!((e.subject, e.matcher, e.value.as_str()), (Subject::Url, Matcher::EndsWith, "/cart"));
        let e = p("text \"Thank you\"").unwrap();
        assert_eq!(e.subject, Subject::Text { text: "Thank you".into(), within: None });
        let e = p("no text Error").unwrap();
        assert!(e.negate);
        let e = p("--role button --name \"Place order\" enabled").unwrap();
        assert_eq!(
            e.subject,
            Subject::Target {
                flags: vec!["--role".into(), "button".into(), "--name".into(), "Place order".into()],
                prop: Prop::State("enabled".into())
            }
        );
        let e = p("--selector .card count >= 3 --timeout-ms 800").unwrap();
        assert_eq!((e.matcher, e.value.as_str(), e.timeout_ms), (Matcher::Ge, "3", Some(800)));
        let e = p("--selector #faq attr open is \"\"").unwrap();
        assert!(matches!(e.subject, Subject::Target { prop: Prop::Attr(ref a), .. } if a == "open"));
        let e = p("not --selector .toast visible").unwrap();
        assert!(e.negate);
        let e = p("no errors").unwrap();
        assert_eq!((e.subject, e.negate), (Subject::Errors, false));
        let e = p("${order} matches \"^ORD-[0-9]+$\"").unwrap();
        assert_eq!(e.subject, Subject::Var("${order}".into()));
    }

    #[test]
    fn mistakes_say_what_was_expected() {
        assert!(p("url sounds-like x").unwrap_err().contains("after url"));
        assert!(p("title is").unwrap_err().contains("needs a value"));
        assert!(p("--testid total text is $36 dollars").unwrap_err().contains("quote it"));
        assert!(p("--role button pressed").unwrap_err().contains("say what to check"));
        assert!(p("--role button count is 2").unwrap_err().contains("--selector"));
        assert!(p("url matches \"(\"").unwrap_err().contains("regular expression"));
        assert!(p("text a b").is_err());
    }

    #[test]
    fn page_state_subjects() {
        let e = p("js \"document.forms.length\" >= 1").unwrap();
        assert_eq!((e.subject, e.matcher), (Subject::Js("document.forms.length".into()), Matcher::Ge));
        let e = p("eval \"String(1 + 1)\" is 2").unwrap();
        assert_eq!(e.subject, Subject::Js("String(1 + 1)".into()));
        let e = parse(&["js".into(), "--main".into(), "({ app }) => app.getName()".into(), "is".into(), "x".into()])
            .unwrap();
        assert_eq!((e.subject, e.matcher), (Subject::JsMain("({ app }) => app.getName()".into()), Matcher::Is));
        let e = parse(&["js".into(), "--file".into(), "totals.js".into(), "is".into(), "true".into()]).unwrap();
        assert_eq!((e.subject, e.matcher), (Subject::JsFile("totals.js".into()), Matcher::Is));
        let e = p("js \"window.ready === true\"").unwrap();
        assert_eq!(e.matcher, Matcher::Holds);
        let e = p("request post /api/orders status 201").unwrap();
        assert_eq!(
            e.subject,
            Subject::Request {
                method: "POST".into(),
                url: "/api/orders".into(),
                status: Some("201".into()),
                read: RequestRead::Made
            }
        );
        let e = p("no request GET /api/tracking*").unwrap();
        assert!(e.negate);
        let e = p("cookie session").unwrap();
        assert_eq!((e.subject, e.matcher), (Subject::Cookie("session".into()), Matcher::Holds));
        let e = p("no cookie session").unwrap();
        assert!(e.negate);
        let e = p("storage local cart contains sku-1").unwrap();
        assert_eq!(e.subject, Subject::Storage { area: "local".into(), key: "cart".into() });
        assert!(p("storage disk x").is_err());
        assert!(p("request POST /x status").is_err());
        assert!(glob("/api/orders/*", "/api/orders/12"));
        assert!(glob("*/cart", "http://x/cart"));
        assert!(glob("/a*c*e", "/abcde"));
        assert!(!glob("/api/orders", "/api/orders/12"));
    }

    #[test]
    fn more_subjects() {
        let e = p("text Saved --within --testid toast --soft").unwrap();
        assert_eq!(
            e.subject,
            Subject::Text { text: "Saved".into(), within: Some(vec!["--testid".into(), "toast".into()]) }
        );
        assert!(e.soft);
        assert!(p("no text Oops --within --role dialog").unwrap().negate);
        assert!(p("text a --within").is_err());
        let e = p("--testid badge style color is \"rgb(0, 128, 0)\"").unwrap();
        assert!(matches!(e.subject, Subject::Target { prop: Prop::Style(ref c), .. } if c == "color"));
        assert!(
            matches!(p("--label Notes empty").unwrap().subject, Subject::Target { prop: Prop::State(ref s), .. } if s == "empty")
        );
        let e = p("request POST /api/orders status 201 json $.items[0].sku is sku-1").unwrap();
        assert!(
            matches!(e.subject, Subject::Request { read: RequestRead::Json(ref j), ref status, .. } if j == "$.items[0].sku" && status.as_deref() == Some("201"))
        );
        assert_eq!((e.matcher, e.value.as_str()), (Matcher::Is, "sku-1"));
        let e = p("request GET /api/items* count >= 2").unwrap();
        assert!(matches!(e.subject, Subject::Request { read: RequestRead::Count, .. }));
        assert!(p("request GET /x json total is 1").unwrap_err().contains("JSON path"));
        assert!(p("request GET /x count is many").is_err());
        let e = p("request POST /api/cart status 200 body contains \"added\"").unwrap();
        assert!(
            matches!(e.subject, Subject::Request { read: RequestRead::Body, ref status, .. } if status.as_deref() == Some("200"))
        );
        assert_eq!((e.matcher, e.value.as_str()), (Matcher::Contains, "added"));
        assert!(p("request GET /x body").is_err());
        let e = p("cookie session absent").unwrap();
        assert!(e.negate && e.matcher == Matcher::Holds);
        assert!(p("storage local cart absent").unwrap().negate);
        assert!(!p("cookie mode is absent").unwrap().negate);
        assert_eq!(p("download report-*.csv").unwrap().subject, Subject::Download("report-*.csv".into()));
        assert!(p("no download *.exe").unwrap().negate);
        let e = p("console contains ready").unwrap();
        assert_eq!((e.subject, e.matcher), (Subject::Console, Matcher::Contains));
        assert!(p("no console Warning").unwrap().negate);
        let path = json_path("$.a[1][\"b c\"].d").unwrap();
        let v = serde_json::json!({"a": [0, {"b c": {"d": 7}}]});
        assert_eq!(json_at(&v, &path).as_deref(), Some("7"));
        assert_eq!(json_at(&v, &json_path("$.a[5]").unwrap()), None);
        assert!(json_path("a.b").is_err());
    }

    #[test]
    fn comparisons() {
        assert!(compare("$36.00", Matcher::Is, "$36.00"));
        assert!(compare("3", Matcher::Is, "3.0"));
        assert!(compare("12", Matcher::Ge, "3"));
        assert!(!compare("abc", Matcher::Gt, "3"));
        assert!(compare("ORD-123", Matcher::Matches, "^ORD-[0-9]+$"));
        assert!(compare("http://x/cart", Matcher::EndsWith, "/cart"));
    }

    #[test]
    fn json_counts_schemas_and_times() {
        let w = |s: &str| crate::suite::tokenize(s).unwrap();
        let e = parse(&w("json $.items count >= 2")).unwrap();
        assert_eq!(
            (e.subject, e.matcher, e.value.as_str()),
            (Subject::Http(HttpRead::Count("$.items".into())), Matcher::Ge, "2")
        );
        let e = parse(&w("json $.user schema user.json")).unwrap();
        assert_eq!(e.subject, Subject::JsonSchema { path: "$.user".into(), file: "user.json".into() });
        let e = parse(&w("json schema all.json")).unwrap();
        assert_eq!(e.subject, Subject::JsonSchema { path: "$".into(), file: "all.json".into() });
        let e = parse(&w("time < 1.5s")).unwrap();
        assert_eq!((e.subject, e.value.as_str(), e.timeout_ms), (Subject::Var("${http.ms}".into()), "1500", Some(0)));
        assert_eq!(parse(&w("time <= 800ms")).unwrap().value, "800");
        assert!(parse(&w("time is 5")).is_err());
        let e = parse(&w("json snapshot me --ignore $.id,$.items[*].at --ignore $.n")).unwrap();
        assert_eq!(
            e.subject,
            Subject::JsonSnapshot {
                path: "$".into(),
                name: "me".into(),
                ignore: vec!["$.id".into(), "$.items[*].at".into(), "$.n".into()]
            }
        );
        let e = parse(&w("json $.user snapshot user")).unwrap();
        assert!(matches!(e.subject, Subject::JsonSnapshot { ref path, .. } if path == "$.user"));
        assert!(parse(&w("json snapshot a/b")).is_err());
        assert!(parse(&w("json snapshot me --ignore id")).is_err());
        assert!(parse(&w("json $.a schema")).is_err());
    }

    #[test]
    fn json_values_compare_as_json() {
        let got = r#"{"role":"shopper","address":{"city":"Oslo","zip":"0150"},"tags":["a","b"],"n":2}"#;
        assert!(compare_json(
            got,
            Matcher::Is,
            r#"{"n": 2.0, "tags": ["a", "b"], "address": {"zip": "0150", "city": "Oslo"}, "role": "shopper"}"#
        ));
        assert!(!compare_json(got, Matcher::Is, r#"{"role": "shopper"}"#));
        assert!(compare_json(got, Matcher::Contains, r#"{"address": {"city": "Oslo"}, "tags": ["b"]}"#));
        assert!(!compare_json(got, Matcher::Contains, r#"{"address": {"city": "Bergen"}}"#));
        let list = r#"[{"sku":"a","qty":1},{"sku":"b","qty":2}]"#;
        assert!(compare_json(list, Matcher::Contains, r#"{"sku": "b"}"#));
        assert!(compare_json(list, Matcher::Contains, r#"[{"qty": 2}, {"sku": "a"}]"#));
        assert!(
            !compare_json(list, Matcher::Is, r#"[{"sku":"b","qty":2},{"sku":"a","qty":1}]"#),
            "order counts for is"
        );
        // Text stays text.
        assert!(compare_json("shopper", Matcher::Contains, "shop"));
        assert!(compare_json("[1, 2]", Matcher::Contains, "1, 2"));
    }
}
