//! `bitti merge <dir|file>...`: the jsonl results of several runs (CI
//! shards) read back as one run.

use crate::report;
use crate::runner::{Failure, ScenarioResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const HELP: &str = "usage: bitti merge <dir|file.jsonl>... [--junit <file>] [--html <file>]\n\
                        \x20                      [--fail-on-flaky]\n\n\
                        Joins the results of shards (`run --shard i/n --reporter jsonl`, saved\n\
                        to a file each) into one console summary, one --junit and one --html, as\n\
                        if one run had done it. A directory is searched for .jsonl files. Steps\n\
                        and traces stay with the shards: the joined report has each scenario's\n\
                        status, time and failure. The time is the longest shard's, as shards\n\
                        run side by side. Exit: 0 all passed, 1 one failed, 2 nothing to read.\n";

/// What `merge` was asked for.
struct Opts {
    paths: Vec<PathBuf>,
    junit: Option<PathBuf>,
    html: Option<PathBuf>,
    fail_on_flaky: bool,
}

fn parse(words: &[String]) -> Result<Opts, String> {
    let mut o = Opts { paths: Vec::new(), junit: None, html: None, fail_on_flaky: false };
    let mut it = words.iter();
    while let Some(w) = it.next() {
        let mut val = |name: &str| it.next().map(PathBuf::from).ok_or_else(|| format!("{name} needs a value"));
        match w.as_str() {
            "--junit" => o.junit = Some(val("--junit")?),
            "--html" => o.html = Some(val("--html")?),
            "--fail-on-flaky" => o.fail_on_flaky = true,
            f if f.starts_with("--") => return Err(format!("unknown option {f:?} (bitti merge --help)")),
            p => o.paths.push(PathBuf::from(p)),
        }
    }
    if o.paths.is_empty() {
        return Err("bitti merge needs the directories or .jsonl files of the shards (bitti merge --help)".into());
    }
    Ok(o)
}

/// The .jsonl files at `path`: itself, or the ones under a directory.
fn files(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {}", path.display(), crate::plain_io(&e)))?;
    if meta.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("{}: {}", path.display(), crate::plain_io(&e)))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for e in entries {
            if e.is_dir() || e.extension().is_some_and(|x| x == "jsonl" || x == "ndjson") {
                files(&e, out)?;
            }
        }
    } else {
        out.push(path.to_path_buf());
    }
    Ok(())
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn failure_of(v: &Value) -> Option<Failure> {
    if !v.is_object() {
        return None;
    }
    let opt = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    Some(Failure {
        line: v.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
        text: text(v, "step"),
        message: text(v, "message"),
        at: opt("url").map(|u| (u, text(v, "title"))),
        hint: opt("note"),
        screenshot: opt("screenshot").map(PathBuf::from),
        nearest: v
            .get("nearest")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|n| {
                        (text(n, "element"), text(n, "locator"), n.get("score").and_then(Value::as_f64).unwrap_or(0.0))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        source: None,
    })
}

/// A scenario's `event: scenario` line as a result (steps are not in it).
fn scenario(v: &Value) -> ScenarioResult {
    let strings = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let status = text(v, "status");
    let mut failure = v.get("failure").and_then(failure_of);
    // A failed scenario without its failure object is still a failure.
    if status == "failed" && failure.is_none() {
        failure = Some(Failure {
            line: v.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
            text: String::new(),
            message: "failed (the shard's result carried no detail)".into(),
            at: None,
            hint: None,
            screenshot: None,
            nearest: Vec::new(),
            source: None,
        });
    }
    ScenarioResult {
        file: PathBuf::from(text(v, "file")),
        name: text(v, "name"),
        line: v.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
        tags: strings("tags"),
        ms: v.get("ms").and_then(Value::as_f64).unwrap_or(0.0),
        steps: Vec::new(),
        failure,
        attempt: v.get("attempt").and_then(Value::as_u64).unwrap_or(1) as u32,
        retried: v
            .get("retried")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(failure_of).collect())
            .unwrap_or_default(),
        saved: v
            .get("saved")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default(),
        notes: strings("notes"),
        worker: v.get("worker").and_then(Value::as_u64).unwrap_or(0) as usize,
        rep: 0,
        started: 0.0,
        trace: None,
        video: None,
        skipped: (status == "skipped")
            .then(|| v.get("skipped").and_then(Value::as_str).unwrap_or("skipped").to_string()),
    }
}

/// The scenarios in the shard files, and the longest shard's time.
pub fn read(paths: &[PathBuf]) -> Result<(Vec<ScenarioResult>, f64), String> {
    let mut found = Vec::new();
    for p in paths {
        files(p, &mut found)?;
    }
    let mut results = Vec::new();
    let (mut longest, mut summed) = (0f64, 0f64);
    for f in &found {
        let body = std::fs::read_to_string(f).map_err(|e| format!("{}: {}", f.display(), crate::plain_io(&e)))?;
        for line in body.lines() {
            // Other lines (GitHub annotations, console text) are not ours.
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            match v.get("event").and_then(Value::as_str) {
                Some("scenario") => {
                    let r = scenario(&v);
                    summed += r.ms;
                    results.push(r);
                }
                Some("summary") => longest = longest.max(v.get("ms").and_then(Value::as_f64).unwrap_or(0.0)),
                _ => {}
            }
        }
    }
    if results.is_empty() {
        return Err(format!("no scenario results in {} file{}", found.len(), if found.len() == 1 { "" } else { "s" }));
    }
    Ok((results, if longest > 0.0 { longest } else { summed }))
}

/// Runs `merge`: the text for stdout and the exit code.
pub fn main(words: &[String]) -> Result<(String, u8), String> {
    let o = parse(words)?;
    let (results, ms) = read(&o.paths)?;
    let mut out = String::new();
    for r in &results {
        out.push_str(&report::console_line(r));
    }
    out.push_str(&report::summary(&results, ms));
    out.push('\n');
    let write = |path: &Path, body: String| -> Result<(), String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, body).map_err(|e| format!("could not write {}: {e}", path.display()))
    };
    if let Some(p) = &o.junit {
        write(p, report::junit(&results, ms))?;
    }
    if let Some(p) = &o.html {
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        write(p, report::html(&results, ms, dir))?;
        out.push_str(&format!("report {}\n", p.display()));
    }
    let failed = results.iter().any(|r| !r.passed()) || (o.fail_on_flaky && results.iter().any(|r| r.flaky()));
    Ok((out, u8::from(failed)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str, status: &str, ms: f64) -> String {
        let failure = if status == "failed" {
            r#"{"line":7,"step":"expect text Done","message":"expected Done","url":"http://x/a","title":"A","note":null,"screenshot":null,"nearest":[]}"#
        } else {
            "null"
        };
        format!(
            r#"{{"event":"scenario","file":"a.bt","line":3,"name":"{name}","tags":["smoke"],"status":"{status}","ms":{ms},"attempt":1,"worker":0,"failure":{failure},"retried":[],"saved":{{}},"notes":[]}}"#
        )
    }

    #[test]
    fn shards_join_as_one_run() {
        let dir = std::env::temp_dir().join(format!("bitti-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shard-2")).unwrap();
        let one = format!("{}\n::error file=a.bt::noise\n{}\n{{\"event\":\"summary\",\"passed\":2,\"flaky\":0,\"failed\":0,\"ms\":900}}\n", line("one", "passed", 400.0), line("two", "passed", 500.0));
        let two = format!(
            "{}\n{{\"event\":\"summary\",\"passed\":0,\"flaky\":0,\"failed\":1,\"ms\":1200}}\n",
            line("three", "failed", 1200.0)
        );
        std::fs::write(dir.join("one.jsonl"), one).unwrap();
        std::fs::write(dir.join("shard-2/two.jsonl"), two).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a result").unwrap();
        let (results, ms) = read(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(ms, 1200.0, "the longest shard");
        assert_eq!(results.iter().filter(|r| !r.passed()).count(), 1);
        let bad = results.iter().find(|r| r.name == "three").unwrap();
        assert_eq!(bad.failure.as_ref().unwrap().at, Some(("http://x/a".into(), "A".into())));
        assert_eq!(bad.tags, ["smoke"]);
        let junit = dir.join("out/merged.xml");
        let (text, code) = main(&[dir.display().to_string(), "--junit".into(), junit.display().to_string()]).unwrap();
        assert_eq!(code, 1);
        assert!(text.contains("2 passed, 1 failed"), "{text}");
        let xml = std::fs::read_to_string(&junit).unwrap();
        assert_eq!(xml.matches("<testcase").count(), 3, "{xml}");
        assert!(xml.contains("tests=\"3\""), "{xml}");
        let empty = std::env::temp_dir().join(format!("bitti-merge-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(read(&[empty]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
