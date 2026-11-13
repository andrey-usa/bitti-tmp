//! The examples against navigera's bench site, and what a failure says.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

struct Site(Child, String);

impl Drop for Site {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// navigera's bench site, from the navigera checkout beside this one.
fn site() -> Option<Site> {
    let server = concat!(env!("CARGO_MANIFEST_DIR"), "/../navigera/bench/site/server.py");
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .args([server, "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site(child, url))
}

/// bitti's own small site (`tests/fixtures/site.py`).
fn own_site() -> Option<Site> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    let mut child = Command::new(python)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/site.py"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut line = String::new();
    BufReader::new(child.stdout.take()?).read_line(&mut line).ok()?;
    let url = line.trim().strip_prefix("LISTENING ")?.to_string();
    Some(Site(child, url))
}

/// Browser tests take turns: each bitti run is several Chromes
/// already, and a small CI machine running them all at once times out.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The test's turn with the browser (held while the guard lives), or none
/// without a browser to test with.
fn chrome() -> Option<std::sync::MutexGuard<'static, ()>> {
    let found = navigera::browser::resolve_executable_for(None, true).is_some();
    assert!(found || std::env::var_os("BITTI_REQUIRE_BROWSER").is_none(), "no Chrome/Chromium found");
    // A test that failed holding it doesn't stop the others.
    found.then(|| TURN.lock().unwrap_or_else(|e| e.into_inner()))
}

fn bitti(args: &[&str]) -> (i32, String) {
    let evidence = tempfile_dir();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(args)
        .args(["--evidence", &evidence])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run bitti");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

fn tempfile_dir() -> String {
    std::env::temp_dir().join(format!("bitti-test-{}", std::process::id())).display().to_string()
}

/// An Electron app, a fresh one per scenario: navigera's fixture app, from
/// the navigera checkout beside this one once `npm install` has run there
/// (CI: the electron job, with `BITTI_REQUIRE_ELECTRON`).
#[test]
fn an_electron_app_is_tested_scenario_by_scenario() {
    let app = concat!(env!("CARGO_MANIFEST_DIR"), "/../navigera/tests/fixtures/electron-app");
    let binary = if cfg!(target_os = "macos") {
        "Electron.app/Contents/MacOS/Electron"
    } else if cfg!(windows) {
        "electron.exe"
    } else {
        "electron"
    };
    let installed = std::path::Path::new(app).join("node_modules/electron/dist").join(binary).is_file();
    assert!(
        installed || std::env::var_os("BITTI_REQUIRE_ELECTRON").is_none(),
        "BITTI_REQUIRE_ELECTRON is set but {app} has no Electron: run `npm install` there"
    );
    if !installed {
        eprintln!("skipping: no Electron in {app}");
        return;
    }
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let (code, out) = bitti(&["run", "tests/fixtures/electron", "--electron", app, "--workers", "2"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("3 passed, 0 failed"), "{out}");
    assert!(out.contains("--workers 2 becomes 1"), "{out}");
}

/// An Electron app already running with a DevTools port: `--attach` runs
/// the scenarios in it, one after the other, and leaves it running.
#[test]
fn an_electron_app_already_running_is_tested_by_attaching() {
    use std::io::{BufRead, BufReader};
    let app = concat!(env!("CARGO_MANIFEST_DIR"), "/../navigera/tests/fixtures/electron-app");
    let binary = if cfg!(target_os = "macos") {
        "Electron.app/Contents/MacOS/Electron"
    } else if cfg!(windows) {
        "electron.exe"
    } else {
        "electron"
    };
    let electron = std::path::Path::new(app).join("node_modules/electron/dist").join(binary);
    assert!(
        electron.is_file() || std::env::var_os("BITTI_REQUIRE_ELECTRON").is_none(),
        "BITTI_REQUIRE_ELECTRON is set but {app} has no Electron: run `npm install` there"
    );
    if !electron.is_file() {
        eprintln!("skipping: no Electron in {app}");
        return;
    }
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut command = Command::new(&electron);
    command.arg("--remote-debugging-port=0");
    if cfg!(target_os = "linux") {
        // The CI runner can't give Chromium its sandbox.
        command.arg("--no-sandbox");
    }
    let mut child = command
        .arg(app)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the fixture app");
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let mut app_process = Kill(child);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            if let Some(at) = line.find("ws://") {
                let _ = tx.send(line[at..].trim().to_string());
            }
        }
    });
    let ws = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("the app's DevTools URL");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let (code, out) = bitti(&["run", "tests/fixtures/electron-attached", "--attach", &ws, "--workers", "2"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("3 passed, 0 failed"), "{out}");
    assert!(out.contains("--workers 2 becomes 1"), "{out}");
    assert!(app_process.0.try_wait().unwrap().is_none(), "the app still runs after the run");
}

/// Two workers tracing every step at once: each step that names a
/// screenshot has its own file, a JPEG (their captures go through one temp
/// name per capture, not per step index, so two threads can't trade them).
#[test]
fn two_workers_trace_every_step_without_trading_screenshots() {
    let (Some(_turn), Some(site)) = (chrome(), site()) else { return };
    let ev = std::env::temp_dir().join(format!("bt-trace2-{}", std::process::id()));
    let child = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", "examples/bench-site", "--base-url", &site.1, "--workers", "2", "--trace", "all"])
        .args(["--evidence", ev.to_str().unwrap()])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    let (mut shots, mut scenarios) = (0, 0);
    for dir in std::fs::read_dir(&ev).unwrap().filter_map(Result::ok).map(|e| e.path()) {
        let Ok(trace) = std::fs::read_to_string(dir.join("trace.jsonl")) else { continue };
        scenarios += 1;
        for line in trace.lines() {
            let step: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(shot) = step["shot"].as_str() {
                let jpg = std::fs::read(dir.join(shot)).unwrap_or_else(|_| panic!("{} names {shot}", dir.display()));
                assert_eq!(&jpg[..3], &[0xFF, 0xD8, 0xFF], "{shot} is a JPEG");
                shots += 1;
            }
        }
    }
    assert!(scenarios >= 10 && shots >= scenarios, "{scenarios} scenarios, {shots} shots: {text}");
    let leftovers = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("bitti-step-{pid}-")))
        .count();
    assert_eq!(leftovers, 0, "temp captures left behind");
    let _ = std::fs::remove_dir_all(&ev);
}

#[test]
fn the_examples_pass_on_the_bench_site() {
    let (Some(_turn), Some(site)) = (chrome(), site()) else {
        return;
    };
    let junit = std::env::temp_dir().join(format!("bitti-junit-{}.xml", std::process::id()));
    let (code, out) =
        bitti(&["run", "examples/bench-site", "--base-url", &site.1, "--junit", &junit.display().to_string()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("10 passed, 0 failed"), "{out}");
    let xml = std::fs::read_to_string(&junit).unwrap();
    assert!(xml.contains("tests=\"10\" failures=\"0\""), "{xml}");
}

#[test]
fn a_failure_says_what_was_expected_seen_and_why() {
    let (Some(_turn), Some(site)) = (chrome(), site()) else {
        return;
    };
    let (code, out) = bitti(&["run", "tests/fixtures/failing.bt", "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("expected --selector #cart-count text is \"2\" within 600 ms; saw \"1\""), "{out}");
    assert!(out.contains("note: the step at line 10 (click --selector \"h1\") changed nothing"), "{out}");
    // A check that held in between: the click no longer explains the failure.
    assert!(out.contains("Something else that never shows"), "{out}");
    assert!(!out.contains("note: the step at line 15"), "{out}");
    assert!(out.contains("screenshot "), "{out}");
    assert!(
        out.contains("in flow open-docs, failing.bt:21 `expect title contains \"Nope\" --timeout-ms 300`"),
        "{out}"
    );
    assert!(out.contains("nearest on the page:"), "{out}");
    assert!(out.contains("1. button \"Accept all\""), "{out}");
    assert!(out.contains("--role button --name \"Accept all\""), "{out}");
    assert!(out.contains("0 passed, 5 failed"), "{out}");
}

#[test]
fn lint_and_list_need_no_browser() {
    let (code, out) = bitti(&["lint", "examples"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("4 files, 10 scenarios"), "{out}");
    let (code, out) = bitti(&["list", "examples", "--tag", "smoke"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.lines().count(), 2, "{out}");
    let bad = std::env::temp_dir().join(format!("bitti-bad-{}.bt", std::process::id()));
    std::fs::write(&bad, "scenario \"A\"\n  click 12\n").unwrap();
    let (code, out) = bitti(&["lint", &bad.display().to_string()]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains(":2: refs (12) change between runs"), "{out}");
}

#[test]
fn an_expect_is_woken_by_the_pages_change() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else {
        return;
    };
    // Text that arrives 200 ms in, text that goes 300 ms in (a negated
    // expect), a target's text that changes 200 ms in, plus a button
    // that enables, a box a script checks (no event: seen by the wake's
    // 25 ms cap, as the poll would), a target that appears, a negated
    // state expect, a box a click checks and a field that takes focus
    // (woken by their events): the waits resolve, well inside the timeout.
    let (code, out) = bitti(&["run", "tests/fixtures/wake.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("9 passed, 0 failed"), "{out}");
}

#[test]
fn workers_retries_and_reporters() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    // Retried: the first try fails, the second passes, and that is flaky.
    let (code, out) =
        bitti(&["run", "tests/fixtures/flaky.bt", "--base-url", &site.1, "--retries", "1", "--workers", "2"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("FLAKY tests/fixtures/flaky.bt:1  Fails once, then passes"), "{out}");
    assert!(out.contains("failed first:"), "{out}");
    assert!(out.contains("1 passed, 1 flaky, 0 failed"), "{out}");
    // --fail-on-flaky makes it fail the run; jsonl says what happened.
    let (_, out) =
        bitti(&["run", "tests/fixtures/flaky.bt", "--base-url", &site.1, "--grep", "Steady", "--reporter", "jsonl"]);
    let first: serde_json::Value = serde_json::from_str(out.lines().next().unwrap_or("")).unwrap_or_default();
    assert_eq!(first["event"], "scenario", "{out}");
    assert_eq!(first["status"], "passed", "{out}");
    let site2 = own_site().unwrap();
    let (code, out) =
        bitti(&["run", "tests/fixtures/flaky.bt", "--base-url", &site2.1, "--retries", "1", "--fail-on-flaky"]);
    assert_eq!(code, 1, "{out}");
    // GitHub annotations: the failing line, no retries.
    let site3 = own_site().unwrap();
    let (code, out) = bitti(&["run", "tests/fixtures/flaky.bt", "--base-url", &site3.1, "--reporter", "github"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("::error file=tests/fixtures/flaky.bt,line=3,title=Fails once%2C then passes::"), "{out}");
}

#[test]
fn a_totp_variable_is_the_authenticators_code_now() {
    // RFC 6238's key; Python works the code out on its own and takes this
    // window's or, across a boundary, the one before.
    let dir = std::env::temp_dir().join(format!("bt-totp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let secrets = dir.join("secrets.env");
    std::fs::write(&secrets, "SHOP_TOTP=otpauth://totp/Shop:ada?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Shop\n")
        .unwrap();
    std::fs::write(
        dir.join("check.py"),
        "import hmac, hashlib, struct, sys, time\nk = b'12345678901234567890'\n\
         def code(t):\n    h = hmac.new(k, struct.pack('>Q', t // 30), hashlib.sha1).digest(); o = h[19] & 15\n\
         \x20   return '%06d' % ((struct.unpack('>I', h[o:o + 4])[0] & 0x7fffffff) % 1000000)\n\
         now = int(time.time())\nsys.exit(0 if sys.argv[1] in (code(now), code(now - 30)) else 1)\n",
    )
    .unwrap();
    let file = dir.join("totp.bt");
    std::fs::write(
        &file,
        "scenario \"The code checks out\"\n  exec ${PY} check.py ${totp:SHOP_TOTP}\n    expect exit 0\n\n\
         scenario \"A missing secret says what to set\"\n  exec ${PY} check.py ${totp:NOPE_TOTP}\n",
    )
    .unwrap();
    let py = if cfg!(windows) { "PY=python" } else { "PY=python3" };
    let (code, out) = bitti(&["run", file.to_str().unwrap(), "--secrets", secrets.to_str().unwrap(), "--var", py]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1 passed, 1 failed"), "{out}");
    assert!(out.contains("set NOPE_TOTP (the environment, or --secrets)"), "{out}");
    assert!(!out.contains("GEZDGNBV"), "the key never shows: {out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_serial_scenario_runs_while_no_other_does() {
    // API-only scenarios (no browser): each logs when it starts and ends,
    // 300 ms apart. Four at two workers, one of them @serial. A file each:
    // two processes appending to one file can lose a line on Windows, where
    // an append is a seek and then a write, and a lost "end" read here as
    // scenarios that never overlapped.
    let dir = std::env::temp_dir().join(format!("bt-serial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("log.txt");
    std::fs::write(
        dir.join("mark.py"),
        "import sys, time\nlog = open(sys.argv[2] + '.' + sys.argv[1], 'w')\nlog.write(f'{time.time()} start {sys.argv[1]}\\n'); log.flush()\n\
         time.sleep(0.3)\nlog.write(f'{time.time()} end {sys.argv[1]}\\n')\n",
    )
    .unwrap();
    let step = |name: &str| format!("  exec ${{PY}} mark.py {name} ${{LOG}}\n");
    let body = format!(
        "scenario \"a\"\n{}\nscenario \"b\" @serial\n{}\nscenario \"c\"\n{}\nscenario \"d\"\n{}",
        step("a"),
        step("b"),
        step("c"),
        step("d")
    );
    let file = dir.join("serial.bt");
    std::fs::write(&file, body).unwrap();
    let py = if cfg!(windows) { "PY=python" } else { "PY=python3" };
    let log_var = format!("LOG={}", log.display());
    let (code, out) = bitti(&["run", file.to_str().unwrap(), "--workers", "2", "--var", py, "--var", &log_var]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("4 passed"), "{out}");
    let text: String = ["a", "b", "c", "d"]
        .iter()
        .map(|n| std::fs::read_to_string(format!("{}.{n}", log.display())).unwrap())
        .collect();
    let mut spans: std::collections::BTreeMap<String, (f64, f64)> = Default::default();
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        let t: f64 = w[0].parse().unwrap();
        let e = spans.entry(w[2].to_string()).or_insert((0.0, 0.0));
        if w[1] == "start" {
            e.0 = t
        } else {
            e.1 = t
        }
    }
    assert_eq!(spans.len(), 4, "{text}");
    for (name, s) in &spans {
        assert!(s.0 > 0.0 && s.1 >= s.0, "{name} has no start or no end {s:?}:\n{text}");
    }
    let b = spans["b"];
    for (name, s) in &spans {
        if name != "b" {
            assert!(s.1 <= b.0 || s.0 >= b.1, "{name} {s:?} ran while the serial one did {b:?}:\n{text}");
        }
    }
    // The others still ran side by side.
    let others: Vec<(f64, f64)> = spans.iter().filter(|(n, _)| *n != "b").map(|(_, s)| *s).collect();
    let overlap = others.iter().enumerate().any(|(i, x)| others[i + 1..].iter().any(|y| x.0 < y.1 && y.0 < x.1));
    assert!(overlap, "two workers ran nothing at once:\n{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shards_split_the_scenarios() {
    let (_, all) = bitti(&["list", "examples"]);
    let mut all: Vec<&str> = all.lines().collect();
    let mut parts: Vec<String> = Vec::new();
    for i in 1..=3 {
        let (code, out) = bitti(&["list", "examples", "--shard", &format!("{i}/3")]);
        assert_eq!(code, 0, "{out}");
        assert!(!out.trim().is_empty(), "shard {i}/3 is empty");
        parts.extend(out.lines().map(str::to_string));
    }
    let mut parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    all.sort();
    parts.sort();
    assert_eq!(all, parts, "the shards cover every scenario once");
    let (code, out) = bitti(&["list", "examples", "--shard", "4/3"]);
    assert_eq!(code, 2, "{out}");
}

#[test]
fn if_blocks_look_at_the_page_as_it_is() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/if.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn ax_trees_match_partially_and_say_where_they_dont() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/ax.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    let (code, out) = bitti(&["run", "tests/fixtures/ax-fail.bt", "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("expected ax --role form --name Checkout to match the tree under it"), "{out}");
    assert!(out.contains("nearest: `- combobox \"Country\" value=\"Canada\""), "{out}");
}

#[test]
fn exec_runs_programs_and_checks_what_they_said() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let py = if cfg!(windows) { "PY=python" } else { "PY=python3" };
    let (code, out) = bitti(&["run", "tests/fixtures/exec.bt", "--base-url", &site.1, "--var", py]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    let (code, out) = bitti(&["run", "tests/fixtures/exec-fail.bt", "--var", py]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("exited 2: db down"), "{out}");
    assert!(out.contains("still ran after 300 ms; stopped it"), "{out}");
    // A script named by its path, the run started from elsewhere.
    if cfg!(unix) {
        let (code, out) = bitti(&["run", "tests/fixtures/exec-path.bt"]);
        assert_eq!(code, 0, "{out}");
    }
}

#[test]
fn http_calls_share_cookies_with_the_browser() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let secrets = secrets_file();
    let (code, out) = bitti(&["run", "tests/fixtures/http.bt", "--base-url", &site.1, "--secrets", &secrets]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
    let (code, out) = bitti(&["run", "tests/fixtures/http-fail.bt", "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("/api/fail answered 500: db down"), "{out}");
    assert!(out.contains("GET http://127.0.0.1:9/: "), "{out}");
}

#[test]
fn feature_files_run_with_the_runs_steps_and_report_cucumber_json() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let report = std::env::temp_dir().join(format!("bt-cucumber-{}.json", std::process::id()));
    let (code, out) =
        bitti(&["run", "tests/fixtures/features", "--base-url", &site.1, "--cucumber", &report.display().to_string()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("3 passed, 0 failed"), "{out}");
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(j[0]["name"], "Signing in");
    assert_eq!(j[0]["elements"].as_array().map(Vec::len), Some(3));
    let first = &j[0]["elements"][0];
    assert_eq!(first["steps"][0]["keyword"], "Given ");
    assert!(first["steps"].as_array().unwrap().iter().all(|s| s["result"]["status"] == "passed"), "{first}");
    // Allure results: one per scenario, the feature as a label.
    let allure = std::env::temp_dir().join(format!("bt-allure-{}", std::process::id()));
    let (code, out) =
        bitti(&["run", "tests/fixtures/features", "--base-url", &site.1, "--allure", &allure.display().to_string()]);
    assert_eq!(code, 0, "{out}");
    let results: Vec<_> = std::fs::read_dir(&allure).unwrap().filter_map(Result::ok).map(|e| e.path()).collect();
    assert_eq!(results.iter().filter(|p| p.to_string_lossy().ends_with("-result.json")).count(), 3, "{results:?}");
    let one: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&results[0]).unwrap()).unwrap();
    assert!(one["labels"].to_string().contains("Signing in"), "{one}");
    std::fs::remove_dir_all(&allure).ok();
    // A failure: the step that failed, and the ones after it skipped.
    let (code, out) = bitti(&[
        "run",
        "tests/fixtures/features-fail.feature",
        "--base-url",
        &site.1,
        "--timeout-ms",
        "300",
        "--cucumber",
        &report.display().to_string(),
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Then the title is \"Nope\""), "{out}");
    let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    let statuses: Vec<&str> = j[0]["elements"][0]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["result"]["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["passed", "failed", "skipped"]);
    std::fs::remove_file(&report).ok();
}

#[test]
fn a_config_starts_the_app_and_picks_a_profile() {
    let Some(_turn) = chrome() else { return };
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("bt-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let py = if cfg!(windows) { "python" } else { "python3" };
    let site = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/site.py").replace('\\', "/");
    let config = format!(
        "base_url = \"http://127.0.0.1:{port}\"\npaths = [\"t.bt\"]\n[vars]\ngreeting = \"from the file\"\n\
         [server]\ncommand = \"{py} {site} --port {port}\"\nurl = \"http://127.0.0.1:{port}/login\"\nreuse = false\n\
         [env.other.vars]\ngreeting = \"from other\"\n"
    );
    std::fs::write(dir.join("bitti.toml"), config).unwrap();
    std::fs::write(
        dir.join("t.bt"),
        "scenario \"Config\"\n  goto /login\n  expect text \"Sign in\"\n  expect ${greeting} is ${want}\n",
    )
    .unwrap();
    let bad = format!(
        "[server]\ncommand = \"{py} -c \\\"print('boom'); raise SystemExit(4)\\\"\"\nurl = \"http://127.0.0.1:{port}/\"\n"
    );
    std::fs::write(dir.join("bad.toml"), bad).unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
            .arg("run")
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("run bitti");
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap_or(-1), text)
    };
    let (code, out) = run(&["--var", "want=from the file"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("/login answers ("), "{out}");
    let (code, out) = run(&["--env", "other", "--var", "want=from other"]);
    assert_eq!(code, 0, "{out}");
    // Stopped with the run.
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "the server is still up");
    let (code, out) = run(&["--config", "bad.toml", "t.bt"]);
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("exited") && out.contains("boom"), "{out}");
    let (code, out) = run(&["--env", "nope"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("no profile [env.nope] (there are: other)"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn screenshots_match_their_baselines() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-shots-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bt = dir.join("look.bt");
    std::fs::write(
        &bt,
        "scenario \"The page looks as it did\"\n  goto /login\n  \
         eval \"(document.body.innerHTML = '<h1>Checkout</h1><p id=clock>${run.id}</p><button id=b style=\\\"width:120px;height:30px;background:${color}\\\">Pay</button>', 1)\"\n  \
         expect screenshot pay --selector \"#b\"\n  expect screenshot page --mask \"#clock\"\n",
    )
    .unwrap();
    let evidence = dir.join("ev").display().to_string();
    let run = |extra: &[&str]| {
        let mut args =
            vec!["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", &evidence, "--timeout-ms", "500"];
        args.extend_from_slice(extra);
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).args(&args).output().expect("run bitti");
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap_or(-1), text)
    };
    // No baseline: written, and the step fails until a person has looked.
    let (code, out) = run(&["--var", "color=blue"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no baseline yet: wrote"), "{out}");
    let (code, out) = run(&["--var", "color=blue", "--update-screenshots"]);
    assert_eq!(code, 0, "{out}");
    let os = std::env::consts::OS;
    let shots = dir.join("look.bt.screenshots");
    let pay = std::fs::read(shots.join(format!("pay-{os}.png"))).unwrap();
    assert_eq!(&pay[1..4], b"PNG");
    // The same again matches, the masked clock changing every run.
    let (code, out) = run(&["--var", "color=blue"]);
    assert_eq!(code, 0, "{out}");
    // A different button doesn't, and says where to look.
    let (code, out) = run(&["--var", "color=red"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("pixels differ") && out.contains("pay-diff.png"), "{out}");
    assert!(dir.join("ev").join("look.bt-pay-actual.png").exists());
    // A share of the pixels is allowed by `--max-diff-ratio` on its own: the
    // whole button differs, so 100% lets the shots pass and 0.1% does not.
    let plain = std::fs::read_to_string(&bt).unwrap();
    let with_ratio = |r: &str| {
        let text = plain
            .replace(
                "expect screenshot pay --selector \"#b\"",
                &format!("expect screenshot pay --selector \"#b\" --max-diff-ratio {r}"),
            )
            .replace(
                "expect screenshot page --mask \"#clock\"",
                &format!("expect screenshot page --mask \"#clock\" --max-diff-ratio {r}"),
            );
        std::fs::write(&bt, text).unwrap();
    };
    with_ratio("0.001");
    let (code, out) = run(&["--var", "color=red"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("0.10% allowed"), "{out}");
    with_ratio("1");
    let (code, out) = run(&["--var", "color=red"]);
    assert_eq!(code, 0, "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn press_understands_control_or_meta() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-chord-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let want = if cfg!(target_os = "macos") {
        "window.mods.some(m => m[0] === 'k' && m[2] === true && m[1] === false)"
    } else {
        "window.mods.some(m => m[0] === 'k' && m[1] === true && m[2] === false)"
    };
    let bt = dir.join("chord.bt");
    std::fs::write(
        &bt,
        format!(
            "scenario \"A chord\"\n  goto /login\n  \
             eval \"(() => {{ window.mods = []; document.addEventListener('keydown', e => window.mods.push([e.key, e.ctrlKey, e.metaKey])); return 1; }})()\"\n  \
             press \"ControlOrMeta+k\"\n  \
             expect js \"{want}\"\n"
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", &dir.join("ev").display().to_string()])
        .args(["--timeout-ms", "3000"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn screenshots_wait_for_a_stable_render() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-stable-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // The box is there from load; its paint lands ${delay} ms later.
    let bt = dir.join("late.bt");
    std::fs::write(
        &bt,
        "scenario \"A late paint\"\n  goto /login\n  \
         eval \"(() => { document.body.innerHTML = '<div id=c style=width:80px;height:40px></div>'; setTimeout(() => { document.getElementById('c').style.background = 'rgb(0,0,255)'; }, ${delay}); return 1; })()\"\n  \
         expect screenshot box --selector \"#c\"\n",
    )
    .unwrap();
    let evidence = dir.join("ev").display().to_string();
    let run = |extra: &[&str]| {
        let mut args =
            vec!["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", &evidence, "--timeout-ms", "3000"];
        args.extend_from_slice(extra);
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).args(&args).output().expect("run bitti");
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap_or(-1), text)
    };
    // Baseline from the settled page, then a compare where the paint is late.
    let (code, out) = run(&["--var", "delay=0", "--update-screenshots"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = run(&["--var", "delay=300"]);
    assert_eq!(code, 0, "the compare must not see the blank state: {out}");
    // A baseline written while the paint is late waits for it too.
    std::fs::remove_dir_all(dir.join("late.bt.screenshots")).ok();
    let (code, out) = run(&["--var", "delay=300", "--update-screenshots"]);
    assert_eq!(code, 0, "{out}");
    let baseline =
        std::fs::read(dir.join("late.bt.screenshots").join(format!("box-{}.png", std::env::consts::OS))).unwrap();
    let mut reader = png::Decoder::new(&baseline[..]).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut buf).unwrap();
    let blue = buf.chunks(4).any(|p| p[0] == 0 && p[1] == 0 && p[2] == 255);
    assert!(blue, "the baseline caught the blank shell, not the painted box");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_screenshot_that_never_stabilises_fails_at_the_timeout() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-unstable-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bt = dir.join("busy.bt");
    std::fs::write(
        &bt,
        "scenario \"Never stops\"\n  goto /login\n  \
         eval \"(() => { document.body.innerHTML = '<div id=c style=width:80px;height:40px;background:rgb(0,0,255)></div>'; let n = 0; const tick = () => { document.getElementById('c').style.background = 'hsl(' + (120 + n++ * 7 % 120) + ',80%,50%)'; requestAnimationFrame(tick); }; tick(); return 1; })()\"\n  \
         expect screenshot box --selector \"#c\"\n",
    )
    .unwrap();
    let evidence = dir.join("ev").display().to_string();
    let run = |extra: &[&str]| {
        let mut args =
            vec!["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", &evidence, "--timeout-ms", "1500"];
        args.extend_from_slice(extra);
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).args(&args).output().expect("run bitti");
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap_or(-1), text)
    };
    // Baseline: a still red box, written by a second file and kept under
    // the busy file's screenshots name.
    let still = dir.join("still.bt");
    std::fs::write(
        &still,
        "scenario \"Still\"\n  goto /login\n  \
         eval \"(() => { document.body.innerHTML = '<div id=c style=width:80px;height:40px;background:rgb(255,0,0)></div>'; return 1; })()\"\n  \
         expect screenshot box --selector \"#c\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", still.to_str().unwrap(), "--base-url", &site.1, "--evidence", &evidence, "--timeout-ms", "1500"])
        .args(["--update-screenshots"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::rename(dir.join("still.bt.screenshots"), dir.join("busy.bt.screenshots")).unwrap();
    // The busy page against the still red baseline fails at the timeout
    // instead of hanging. Its hues stay between green and blue: a frame
    // near red would match the baseline within the 0.2 threshold.
    let began = std::time::Instant::now();
    let (code, out) = run(&[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("never settled"), "{out}");
    assert!(began.elapsed() < std::time::Duration::from_secs(30), "the run hung");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_waits_for_a_late_commit() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-latecheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A controlled checkbox: the click's default is cancelled, the state
    // commits 300 ms later (React's forecast-toggle shape).
    let bt = dir.join("late.bt");
    std::fs::write(
        &bt,
        "scenario \"A late commit\"\n  goto /login\n  \
         eval \"(() => { document.body.innerHTML = '<input type=checkbox id=c>'; document.getElementById('c').addEventListener('click', e => { e.preventDefault(); setTimeout(() => { document.getElementById('c').checked = true; }, 300); }); return 1; })()\"\n  \
         check --selector \"#c\"\n  \
         expect js \"document.getElementById('c').checked === true\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", &dir.join("ev").display().to_string()])
        .args(["--timeout-ms", "3000"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn element_screenshots_find_their_element_in_frames_and_scroll_containers() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-shots-el-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A button below the fold of a scroll box, another in an iframe further down.
    let solid = "width:80px;height:30px;border:0;background:rgb(0,0,255)";
    std::fs::write(
        dir.join("page.js"),
        format!(
            "() => {{ document.body.innerHTML = `<div style=\"height:60px;overflow:auto\"><div style=\"height:400px\"></div>\
             <button id=low style=\"{solid}\"></button></div><div style=\"height:300px\"></div>\
             <iframe id=f style=\"margin-left:40px;border:5px solid red\" srcdoc=\"<body style='margin:20px'><button id=framed style='{solid}'></button>\"></iframe>`; return 1; }}"
        ),
    )
    .unwrap();
    let bt = dir.join("el.bt");
    std::fs::write(
        &bt,
        format!(
            "scenario \"Elements\"\n  goto /login\n  eval --file \"{}\"\n  wait --selector \"#framed\"\n  \
             expect screenshot low --selector \"#low\"\n  expect screenshot framed --selector \"#framed\"\n\
             scenario \"A mask on nothing\"\n  goto /login\n  expect screenshot page --mask \"#not-there\"\n",
            dir.join("page.js").display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--update-screenshots", "--timeout-ms", "2000"])
        .args(["--evidence", dir.join("ev").to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("--mask \"#not-there\" matches nothing on the page"), "{text}");
    // Each crop is the button, solid blue to its corners.
    for name in ["low", "framed"] {
        let path = dir.join("el.bt.screenshots").join(format!("{name}-{}.png", std::env::consts::OS));
        let mut reader = png::Decoder::new(std::fs::File::open(&path).unwrap()).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let px = info.color_type.samples();
        let at = |x: u32, y: u32| {
            let i = ((y * info.width + x) as usize) * px;
            (buf[i], buf[i + 1], buf[i + 2])
        };
        let (w, h) = (info.width, info.height);
        for (x, y) in [(1, 1), (w - 2, 1), (w / 2, h / 2), (1, h - 2), (w - 2, h - 2)] {
            assert_eq!(at(x, y), (0, 0, 255), "{name} at {x},{y} of {w}x{h}: {}", path.display());
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_navigera_session_becomes_a_scenario_that_passes() {
    let (Some(_turn), Some(site)) = (chrome(), site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-from-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let name = dir.join("work.sock").display().to_string();
    let nv = |args: &[&str]| -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
            // This reads navigera's JSON line (its default is compact text).
            .env("NAVIGERA_OUTPUT", "json")
            .args(["navigera", "--session", &name])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("bitti navigera");
        let text = String::from_utf8_lossy(&out.stdout);
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or_else(|_| panic!("{args:?}: {text}"));
        assert!(v["ok"] == true, "{args:?}: {v}");
        v["result"].clone()
    };
    // What an agent does: look, act on refs.
    nv(&["start"]);
    nv(&["goto", &format!("{}/login", site.1)]);
    let tree = nv(&["ax"]).as_str().unwrap().to_string();
    let r = |text: &str| -> String {
        let line = tree.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} in\n{tree}"));
        line.split("ref=").nth(1).unwrap().split(|c: char| !c.is_ascii_digit()).next().unwrap().to_string()
    };
    nv(&["click", &r("button \"Accept all\"")]);
    nv(&["fill", &r("textbox \"Email\""), "ada@example.com"]);
    nv(&["fill", &r("textbox \"Password\""), "hunter2-not-in-the-file"]);
    nv(&["get", "value", "--label", "Email"]);
    nv(&["click", &r("link \"Shop\"")]);
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["from-session", &name, "--scenario", "Shops"])
        .output()
        .unwrap();
    nv(&["quit"]);
    let bt = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{bt}{}", String::from_utf8_lossy(&out.stderr));
    assert!(bt.contains("scenario \"Shops\"\n  goto /login\n"), "{bt}");
    assert!(bt.contains("click --role button --name \"Accept all\"\n"), "{bt}");
    assert!(bt.contains("fill --role textbox --name Email ada@example.com\n"), "{bt}");
    assert!(bt.contains("fill --role textbox --name Password ${secret:PASSWORD}\n"), "{bt}");
    assert!(!bt.contains("hunter2"), "{bt}");
    assert!(bt.contains("click --role link --name Shop\n  expect url ends-with /shop\n  expect text Shop\n"), "{bt}");
    assert!(!bt.contains("get value") && !bt.contains("left out"), "reads stay out, every ref found again: {bt}");
    // And it runs, from a fresh browser.
    let file = dir.join("shops.bt");
    std::fs::write(&file, &bt).unwrap();
    let secrets = dir.join("secrets.env");
    std::fs::write(&secrets, "PASSWORD=from-the-secrets-file\n").unwrap();
    let (code, text) =
        bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1, "--secrets", secrets.to_str().unwrap()]);
    assert_eq!(code, 0, "{bt}\n{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn over_mcp_an_agent_explores_writes_the_test_and_runs_it() {
    use std::io::Write as _;
    let (Some(_turn), Some(site)) = (chrome(), site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .arg("mcp")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut id = 0;
    let mut rpc = |method: &str, params: serde_json::Value| -> serde_json::Value {
        id += 1;
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("{method}: {line}"))
    };
    let text = |r: &serde_json::Value| r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
    let init = rpc("initialize", serde_json::json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "bitti", "{init}");
    rpc(
        "tools/call",
        serde_json::json!({"name": "browser_navigate", "arguments": {"url": format!("{}/login", site.1)}}),
    );
    let tree = text(&rpc("tools/call", serde_json::json!({"name": "browser_snapshot", "arguments": {}})));
    let line = tree.lines().find(|l| l.contains("button \"Accept all\"")).unwrap_or_else(|| panic!("{tree}"));
    let r: u64 =
        line.split("ref=").nth(1).unwrap().split(|c: char| !c.is_ascii_digit()).next().unwrap().parse().unwrap();
    rpc("tools/call", serde_json::json!({"name": "browser_click", "arguments": {"ref": r}}));
    let wrote = rpc(
        "tools/call",
        serde_json::json!({"name": "bitti_from_session", "arguments": {"scenario": "Consents", "out": "consent.bt"}}),
    );
    assert!(text(&wrote).contains("click --role button --name \"Accept all\""), "{wrote}");
    let ran = rpc(
        "tools/call",
        serde_json::json!({"name": "bitti_run", "arguments": {"paths": ["consent.bt"], "base_url": site.1}}),
    );
    assert_eq!(ran["result"]["isError"], false, "{}", text(&ran));
    assert!(text(&ran).ends_with("exit 0"), "{}", text(&ran));
    let failure = rpc("tools/call", serde_json::json!({"name": "bitti_failure", "arguments": {}}));
    assert!(text(&failure).contains("no failures"), "{failure}");
    drop(stdin);
    let _ = child.wait();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_recording_becomes_a_scenario_that_passes() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-record-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("order.bt");
    // The page uses itself (`?auto=1`): a field, a checkbox, a select, a link.
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["record", &format!("{}/form?auto=1", site.1), "--headless", "--for", "3"])
        .args(["--scenario", "Orders", "--out", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bt = std::fs::read_to_string(&file).unwrap();
    let want = "scenario \"Orders\"\n  goto /form?auto=1\n  fill --role textbox --name Email ada@example.com\n  \
                check --role checkbox --name \"Gift wrap\"\n  click --selector \"#tip >>> button\"\n  \
                select --role combobox --name Size Large\n  \
                click --role link --name \"Place order\"\n  expect url ends-with /done\n";
    assert!(
        bt.ends_with(want),
        "recorded:\n{bt}\nwanted it to end with:\n{want}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Run without the page using itself: the steps do it.
    std::fs::write(&file, bt.replace("?auto=1", "")).unwrap();
    let (code, text) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 0, "{bt}\n{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_click_that_takes_its_element_away_is_recorded_and_a_covered_click_is_fixed() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-bar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("buy.bt");
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["record", &format!("{}/bar?auto=1", site.1), "--headless", "--for", "3"])
        .args(["--scenario", "Buys", "--out", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bt = std::fs::read_to_string(&file).unwrap();
    // Accept was gone before `locate` could check it: its role and name, read at once, find it.
    assert!(bt.contains("  click --role button --name Accept\n  click --role button --name \"Buy now\"\n"), "{bt}");
    std::fs::write(&file, bt.replace("?auto=1", "")).unwrap();
    let (code, text) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 0, "{bt}\n{text}");
    // Without the Accept click, Buy is covered: `fix` puts it back as an
    // optional click before the step.
    std::fs::write(&file, "scenario \"Buys\"\n  goto /bar\n  click --role button --name \"Buy now\" --timeout-ms 500\n  expect text Bought\n").unwrap();
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_bitti")).args(args).current_dir(&dir).output().unwrap();
    let out = run(&["run", "buy.bt", "--base-url", &site.1]);
    assert!(!out.status.success());
    let list = String::from_utf8_lossy(&run(&["fix"]).stdout).to_string();
    assert!(list.contains("before: optional click --role button --name Accept   0.90"), "{list}");
    let out = run(&["fix", "--all"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let fixed = std::fs::read_to_string(&file).unwrap();
    assert!(
        fixed.contains("  goto /bar\n  optional click --role button --name Accept\n  click --role button"),
        "{fixed}"
    );
    let out = run(&["run", "buy.bt", "--base-url", &site.1]);
    assert!(out.status.success(), "{fixed}\n{}", String::from_utf8_lossy(&out.stdout));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_recording_extends_a_scenario_from_where_it_ends() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-extend-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("order.bt");
    let start = "# An order.\nscenario \"Opens the form\"\n  goto /form?auto=2\n  expect text Order\n\nscenario \"Other\"\n  goto /done\n";
    std::fs::write(&file, start).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["record", "--extend", file.to_str().unwrap(), "--scenario", "Opens the form"])
        .args(["--base-url", &site.1, "--headless", "--for", "4"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let bt = std::fs::read_to_string(&file).unwrap();
    let want = "# An order.\nscenario \"Opens the form\"\n  goto /form?auto=2\n  expect text Order\n  \
                fill --role textbox --name Email ada@example.com\n  check --role checkbox --name \"Gift wrap\"\n  \
                click --selector \"#tip >>> button\"\n  select --role combobox --name Size Large\n  \
                click --role link --name \"Place order\"\n  expect url ends-with /done\n\nscenario \"Other\"\n  goto /done\n";
    assert_eq!(bt, want);
    std::fs::write(&file, bt.replace("?auto=2", "")).unwrap();
    let (code, text) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 0, "{text}");
    // A scenario that fails before its end isn't extended.
    std::fs::write(&file, "scenario \"Broken\"\n  goto /form\n  expect text Nowhere --timeout-ms 200\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["record", "--extend", file.to_str().unwrap(), "--base-url", &site.1, "--headless", "--for", "1"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains("fails there, before its end"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_element_in_a_frame_gets_suggestions_from_the_frame() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-framed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("t.bt"),
        "scenario \"Framed\"\n  goto /framed\n  click --role button --name Subscribe --frame iframe --timeout-ms 300\n",
    )
    .unwrap();
    let bt = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).args(args).current_dir(&dir).output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr),
        )
    };
    let (code, out) = bt(&["run", "t.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1. button \"Subscibe\""), "{out}");
    let (code, out) = bt(&["fix", "t.bt:3", "--use", "1"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = bt(&["run", "t.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 0, "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn accessibility_is_checked_by_axe() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-a11y-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a11y.bt");
    std::fs::write(
        &file,
        "scenario \"Fails\"\n  goto /a11y\n  expect accessible --timeout-ms 300\n\n\
         scenario \"Ignored or scoped\"\n  goto /a11y\n  expect accessible --ignore image-alt,button-name,html-has-lang\n  \
         expect accessible --selector #ok\n  goto /done\n  expect accessible --ignore html-has-lang\n",
    )
    .unwrap();
    let (code, out) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1 passed") && out.contains("1 failed"), "{out}");
    assert!(out.contains("to pass axe (wcag2a, wcag2aa, wcag21a, wcag21aa)"), "{out}");
    assert!(out.contains("button-name (critical, 1: button)") && out.contains("image-alt (critical, 1: img)"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_metrics_are_budgets() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-metrics-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("budgets.bt");
    std::fs::write(
        &file,
        "scenario \"Within budget\"\n  goto /form\n  expect ttfb < 10s\n  expect fcp < 10s\n  expect lcp < 10s\n  \
         expect load < 10000ms\n  expect cls < 0.1\n\nscenario \"Over\"\n  goto /form\n  expect lcp < 0.001ms --timeout-ms 200\n",
    )
    .unwrap();
    let (code, out) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1 passed") && out.contains("1 failed"), "{out}");
    assert!(out.contains("expected lcp < 0.001 ms within 200 ms; saw "), "{out}");
    // INP: the slowest click so far, named; under what Chrome keeps, a pass.
    std::fs::write(
        &file,
        "scenario \"Quick enough\"\n  goto /clicks\n  click --text Quick\n  expect text Done\n  expect inp < 200ms\n\n\
         scenario \"Too slow\"\n  goto /clicks\n  click --text Slow\n  expect inp < 200ms --timeout-ms 300\n\n\
         scenario \"Finer than the browser keeps\"\n  goto /clicks\n  click --text Quick\n  expect inp < 50ms\n",
    )
    .unwrap();
    let (code, out) = bitti(&["run", file.to_str().unwrap(), "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("1 passed") && out.contains("2 failed"), "{out}");
    assert!(out.contains("expected inp < 200 ms within 300 ms; saw ") && out.contains("(#slow)"), "{out}");
    assert!(out.contains("a budget under 104 ms can't be checked"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_video_is_kept_for_a_failure_and_dropped_for_a_pass() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-video-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bt = dir.join("v.bt");
    std::fs::write(
        &bt,
        "scenario \"Passes\"\n  goto /login\n  expect url ends-with /login\n\n\
         scenario \"Fails\"\n  goto /login\n  eval \"document.body.style.background = 'red'\"\n  \
         expect text \"never there\" --timeout-ms 300\n",
    )
    .unwrap();
    let ev = dir.join("ev");
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--video", "on-failure", "--reporter", "jsonl"])
        .args(["--evidence", ev.to_str().unwrap(), "--html", dir.join("r.html").to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    let videos: Vec<std::path::PathBuf> = std::fs::read_dir(&ev)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path().join("video"))
        .filter(|v| v.is_dir())
        .collect();
    assert_eq!(videos.len(), 1, "only the failure keeps one: {videos:?} {text}");
    assert!(
        videos[0].to_string_lossy().contains("Fails") || videos[0].to_string_lossy().contains("fails"),
        "{videos:?}"
    );
    let player = std::fs::read_to_string(videos[0].join("index.html")).unwrap();
    assert!(player.contains("frame-00001.jpg"), "{player}");
    let jpg = std::fs::read(videos[0].join("frame-00001.jpg")).unwrap();
    assert_eq!(&jpg[..2], &[0xFF, 0xD8], "a JPEG");
    let failed = text.lines().find(|l| l.contains("\"status\":\"failed\"")).expect("the failure's line");
    assert!(failed.contains(r#"index.html""#) && failed.contains("video"), "{failed}");
    let passed = text.lines().find(|l| l.contains("\"status\":\"passed\"")).expect("the pass's line");
    assert!(passed.contains("\"video\":null"), "{passed}");
    let report = std::fs::read_to_string(dir.join("r.html")).unwrap();
    assert_eq!(report.matches(">video</a>").count(), 1, "{report}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_trace_records_each_step_and_reads_back() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-trace-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bt = dir.join("t.bt");
    std::fs::write(
        &bt,
        "scenario \"Traced\"\n  goto /login\n  eval \"(console.error('boom-trace'), fetch('/api/n'), 1)\"\n  \
         wait --ms 200\n  expect text \"never there\" --timeout-ms 300\n",
    )
    .unwrap();
    let ev = dir.join("ev");
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--trace", "on-failure"])
        .args(["--evidence", ev.to_str().unwrap(), "--html", dir.join("r.html").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let show = |extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).arg("trace").arg(&ev).args(extra).output().unwrap();
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
    };
    // The run directory lists its traces; the scenario's shows each step.
    let list = show(&[]);
    let scenario =
        std::fs::read_dir(&ev).unwrap().filter_map(Result::ok).find(|e| e.path().join("trace.jsonl").is_file());
    let scenario = scenario.expect("a trace.jsonl").path();
    let all = Command::new(env!("CARGO_BIN_EXE_bitti")).arg("trace").arg(&scenario).output().unwrap();
    let all = String::from_utf8_lossy(&all.stdout).to_string();
    assert!(all.starts_with("FAIL ") && all.contains("\"Traced\""), "{all}{list}");
    assert!(all.contains("console error: boom-trace"), "{all}");
    assert!(all.contains("net #") && all.contains("/api/n"), "{all}");
    assert!(all.contains("/login, title \"t\""), "{all}");
    let one = Command::new(env!("CARGO_BIN_EXE_bitti")).arg("trace").arg(&scenario).arg("5").output().unwrap();
    let one = String::from_utf8_lossy(&one.stdout).to_string();
    assert!(one.contains("FAIL") && one.contains("the page at the failure (ax.txt)"), "{one}");
    // Each step also kept the page as it was after it: an ax tree and a JPEG,
    // named in the trace, shown by `trace <dir> <line>` and in the HTML report.
    let first = Command::new(env!("CARGO_BIN_EXE_bitti")).arg("trace").arg(&scenario).arg("2").output().unwrap();
    let first = String::from_utf8_lossy(&first.stdout).to_string();
    assert!(first.contains("the page after this step (001-L2.ax.txt)") && first.contains("001-L2.jpg"), "{first}");
    let jpg = std::fs::read(scenario.join("001-L2.jpg")).unwrap();
    assert_eq!(&jpg[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
    let report = std::fs::read_to_string(dir.join("r.html")).unwrap();
    assert_eq!(report.matches("<details class=\"step ").count(), 4, "a row per step: {report}");
    assert!(report.contains("data:image/jpeg;base64,") && report.contains("ax tree"), "{report}");
    assert!(report.contains("never there") && !report.contains("<script"), "{report}");
    // `wait` left the page as the step before it did: it reuses that step's
    // files and says so; the failing step is always captured.
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(scenario.join("trace.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|j: &serde_json::Value| j.get("text").is_some())
        .collect();
    let step = |n: u64| lines.iter().find(|j| j["line"] == n).unwrap_or_else(|| panic!("no line {n}: {lines:?}"));
    let (wait, failing) = (step(4), step(5));
    assert!(wait["same_as"].is_u64() && wait["shot"] == step(wait["same_as"].as_u64().unwrap())["shot"], "{wait}");
    assert!(failing.get("same_as").is_none() && failing["shot"].is_string(), "{failing}");
    assert!(report.contains("the page is as it was after line"), "{report}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_download_is_compared_with_the_expected_file() {
    let exe = std::env::var("BITTI_COMPARA").unwrap_or_else(|_| "compara".into());
    if std::process::Command::new(&exe).arg("--version").output().is_err() {
        return; // compara isn't here
    }
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-compare-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("expected.csv"), "id,name\n1,Ada\n").unwrap();
    std::fs::write(dir.join("other.csv"), "id,name\n1,Bob\n").unwrap();
    let bt = dir.join("c.bt");
    let scenario = |name: &str, file: &str| {
        format!("scenario \"{name}\"\n  goto /m1\n  click --selector \"#dl\"\n  expect download \"report-*.csv\" compare {file} --key id\n")
    };
    let rows = "scenario \"Rows and a summary\"\n  goto /m1\n  click --selector \"#dl\"\n  \
                expect download \"report-*.csv\" compare other.csv --key id --report json,rows\n";
    std::fs::write(&bt, scenario("The same", "expected.csv") + &scenario("Not the same", "other.csv") + rows).unwrap();
    let ev = dir.join("ev");
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--evidence", ev.to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("1 passed, 2 failed"), "{text}");
    assert!(text.contains("differs from") && text.contains("changed 1") && text.contains("the report"), "{text}");
    // Each scenario's reports are beside its downloads.
    let names: Vec<String> = std::fs::read_dir(&ev)
        .unwrap()
        .flat_map(|d| std::fs::read_dir(d.unwrap().path()).into_iter().flatten())
        .map(|e| e.unwrap().path().strip_prefix(&ev).unwrap().display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("-compare.html")), "{names:?}");
    // The JSON summary and the rows, when asked for.
    assert!(text.contains("the summary") && text.contains("the rows"), "{text}");
    let rows = names.iter().find(|n| n.ends_with("-compare-rows")).expect("a rows directory");
    assert!(std::fs::read_to_string(ev.join(rows).join("changed.csv")).unwrap().contains("Ada,Bob"));
    let json = names.iter().find(|n| n.ends_with("-compare.json")).expect("a summary");
    let summary: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ev.join(json)).unwrap()).unwrap();
    assert_eq!(summary["counts"]["changed"], 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fix_puts_a_suggestion_into_the_failing_step() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-fix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("t.bt"),
        "scenario \"Signs in\"\n  goto /login\n  click --role button --name \"Sign on\" --timeout-ms 300\n",
    )
    .unwrap();
    let bt = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).args(args).current_dir(&dir).output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr),
        )
    };
    let (code, out) = bt(&["run", "t.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("fix: bitti fix t.bt:3 --use 1"), "{out}");
    let (_, list) = bt(&["fix"]);
    assert!(list.contains("t.bt:3") && list.contains("--role button --name \"Sign in\""), "{list}");
    let (code, out) = bt(&["fix", "t.bt:3", "--use", "1"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("+ click --role button --name \"Sign in\" --timeout-ms 300"), "{out}");
    let (code, out) = bt(&["run", "t.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 0, "{out}");
    // The run that passed left nothing to fix.
    let (code, out) = bt(&["fix", "t.bt:3"]);
    assert_eq!(code, 2, "{out}");
    // A step of a flow in a used file is fixed where it is written.
    std::fs::write(dir.join("lib.bt"), "flow sign_in()\n  click --role button --name \"Sign on\" --timeout-ms 300\n")
        .unwrap();
    std::fs::write(dir.join("u.bt"), "use lib.bt\n\nscenario \"Signs in\"\n  goto /login\n  run sign_in\n").unwrap();
    let (code, out) = bt(&["run", "u.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("fix: bitti fix lib.bt:2 --use 1"), "{out}");
    let (code, out) = bt(&["fix", "lib.bt:2", "--use", "1"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = bt(&["run", "u.bt", "--base-url", &site.1, "--evidence", "ev"]);
    assert_eq!(code, 0, "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A GET to the fixture site, its body.
fn fetch(site: &Site, path: &str) -> String {
    use std::io::{Read, Write};
    let hostport = site.1.trim_start_matches("http://");
    let mut s = std::net::TcpStream::connect(hostport).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()
}

fn secrets_file() -> String {
    let path = std::env::temp_dir().join(format!("bitti-secrets-{}.env", std::process::id()));
    std::fs::write(&path, "# test only\nBT_TEST_PW=\"hunter22\"\n").unwrap();
    path.display().to_string()
}

#[test]
fn the_newer_subjects_and_isolation_between_scenarios() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    for workers in ["1", "2"] {
        let (code, out) = bitti(&["run", "tests/fixtures/m1.bt", "--base-url", &site.1, "--workers", workers]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("5 passed, 1 failed"), "workers {workers}: {out}");
        assert!(out.contains("FAIL  tests/fixtures/m1.bt:23  Soft expectations fail it at the end"), "{out}");
        assert!(out.contains("2 soft expectation(s) didn't hold"), "{out}");
    }
    // Every run of a scenario downloads into a directory of its own: the
    // same file name each time, not `report-2026 (2).csv`.
    let repeat = ["--grep", "Subjects", "--repeat", "3", "--workers", "2"];
    let (code, out) = bitti(&[&["run", "tests/fixtures/m1.bt", "--base-url", &site.1][..], &repeat].concat());
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("3 passed, 0 failed"), "{out}");
}

#[test]
fn auth_runs_once_and_hooks_wrap_the_file() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let secrets = secrets_file();
    let (code, out) =
        bitti(&["run", "tests/fixtures/auth.bt", "--base-url", &site.1, "--workers", "2", "--secrets", &secrets]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("5 passed, 0 failed"), "{out}");
    // Two auths, each signed in once, for five scenarios on two workers.
    assert_eq!(fetch(&site, "/logins"), "2", "{out}");
    assert_eq!(fetch(&site, "/notes"), "setup,teardown", "{out}");
    // Again: `shopper` signs in anew, `keeper` (ttl 1h) is reused.
    let (code, out) = bitti(&["run", "tests/fixtures/auth.bt", "--base-url", &site.1, "--secrets", &secrets]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(fetch(&site, "/logins"), "3", "{out}");
    // A secret never shows in what the run prints.
    let (code, out) = bitti(&["run", "tests/fixtures/secret-fail.bt", "--base-url", &site.1, "--secrets", &secrets]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("saw \"***\""), "{out}");
    assert!(!out.contains("hunter22"), "{out}");
    // A file whose `before all` fails runs none of its scenarios.
    let (code, out) = bitti(&["run", "tests/fixtures/before-fail.bt", "--base-url", &site.1]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("before all: expected title is \"not this one\""), "{out}");
    let (_, out) = bitti(&["lint", "tests/fixtures/auth.bt", "--secrets", &secrets]);
    assert!(out.contains("ok: 1 file, 5 scenarios") && !out.contains("warning"), "{out}");
}

#[test]
fn handoff_waits_only_when_it_must() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/handoff.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    // Nobody at a screen here: it says so instead of waiting.
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        let (code, out) = bitti(&["run", "tests/fixtures/handoff-wait.bt", "--base-url", &site.1]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("handoff needs a person at a screen"), "{out}");
    }
}

#[test]
fn a_pause_leaves_the_failing_tab_to_navigera() {
    use std::io::Write;
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let mut child = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", "tests/fixtures/pause.bt", "--base-url", &site.1, "--pause-on-failure"])
        .args(["--evidence", &tempfile_dir()])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let err = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut seen = String::new();
    let ws = loop {
        let line = rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap_or_else(|_| panic!("no pause: {seen}"));
        seen.push_str(&line);
        seen.push('\n');
        if let Some(i) = line.find("--attach ") {
            break line[i + "--attach ".len()..].trim().to_string();
        }
    };
    assert!(ws.contains("/devtools/page/"), "{ws}");
    let config = navigera::protocol::SessionConfig { attach: Some(ws), ..Default::default() };
    let mut look = navigera::protocol::Driver::launch(&config).expect("attach to the paused tab");
    let here = look.run(&navigera::protocol::Command::Url {}).unwrap();
    assert_eq!(here["title"], "paused here", "{here}");
    look.session().close();
    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn repeat_and_shuffle_hunt_flakes() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    // One worker: the seed fixes the order scenarios start in, and with one
    // worker that is also the order they finish and print in.
    let one = ["--repeat", "3", "--shuffle", "42", "--workers", "1"];
    let (code, out) = bitti(&[&["run", "tests/fixtures/flaky.bt", "--base-url", &site.1][..], &one].concat());
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("shuffled with seed 42"), "{out}");
    assert!(out.contains("FLAKY 2/3  tests/fixtures/flaky.bt  Fails once, then passes"), "{out}");
    assert!(out.contains("    3/3  tests/fixtures/flaky.bt  Steady"), "{out}");
    assert!(out.contains("Steady #3"), "{out}");
    // The same seed, the same order.
    let order = |out: &str| -> Vec<String> {
        out.lines()
            .filter(|l| l.starts_with("PASS") || l.starts_with("FAIL"))
            .map(|l| l.split("  ").nth(2).unwrap_or("").to_string())
            .collect()
    };
    let (_, again) = bitti(&["list", "tests/fixtures/flaky.bt"]);
    assert_eq!(again.lines().count(), 2, "{again}");
    let site2 = own_site().unwrap();
    let (_, out2) = bitti(&[&["run", "tests/fixtures/flaky.bt", "--base-url", &site2.1][..], &one].concat());
    assert_eq!(order(&out), order(&out2), "{out}\n{out2}");
}

#[test]
fn set_values_fill_in_their_variables() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/set-expand.bt", "--base-url", &site.1, "--var", "WHO=ada"]);
    assert_eq!(code, 0, "{out}");
    // Annotations don't take the console's place.
    let (code, out) = bitti(&[
        "run",
        "tests/fixtures/set-expand.bt",
        "--base-url",
        &site.1,
        "--var",
        "WHO=ada",
        "--reporter",
        "github",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

#[test]
fn a_script_that_never_settles_is_bounded_by_the_step_and_an_optional_miss_shows() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = std::env::temp_dir().join(format!("bt-t125-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("never.js"), "new Promise(() => {})").unwrap();
    let bt = dir.join("t.bt");
    std::fs::write(
        &bt,
        "scenario \"Never\"\n  goto /login\n  expect js --file never.js --timeout-ms 1500\n\
         scenario \"Optional\"\n  goto /login\n  optional expect text \"Nothing here\" --timeout-ms 300\n",
    )
    .unwrap();
    let ev = dir.join("ev");
    let start = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", &site.1, "--trace", "all"])
        .args(["--evidence", ev.to_str().unwrap(), "--html", dir.join("r.html").to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(start.elapsed().as_secs() < 25, "{text}");
    assert!(text.contains("never.js did not finish within 1.5 s; a Promise that never settles?"), "{text}");
    assert!(!text.contains("Runtime.evaluate") && !text.contains("35000"), "{text}");
    let html = std::fs::read_to_string(dir.join("r.html")).unwrap();
    assert!(html.contains("class=\"step skip\"") && html.contains("optional, did not hold"), "{html}");
    let trace = |extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_bitti")).arg("trace").args(extra).output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
    };
    let opt = std::fs::read_dir(&ev)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir() && p.to_str().unwrap().contains("optional"))
        .expect("the optional scenario's trace");
    let o = opt.to_str().unwrap();
    let (_, shown) = trace(&[o]);
    assert!(shown.contains("skip") && shown.contains("optional expect text \"Nothing here\""), "{shown}");
    let (code, no_step) = trace(&[o, "99"]);
    assert_eq!(code, Some(0), "{no_step}");
    assert!(no_step.contains("no step at line 99 in this trace; it has lines"), "{no_step}");
    let (code, nan) = trace(&[o, "abc"]);
    assert_eq!(code, Some(2), "{nan}");
    assert!(nan.contains("abc: not a line number"), "{nan}");
    let (code, gone) = trace(&["/nonexist"]);
    assert_eq!(code, Some(2));
    assert_eq!(gone.trim(), "/nonexist: no such directory");
    std::fs::remove_dir_all(&dir).ok();
}

/// A run costs about the same for each scenario: 40 scenarios take about 4x
/// what 10 do (a little less, as the browser launch is paid once). A cost
/// per scenario that grows with the scenarios already run (a list scanned
/// per scenario, evidence or timings rewritten whole each time) shows as a
/// ratio of 16. Two sizes and a ratio, so a slow runner can't fail it; the
/// bound is the one navigera's scaling tests use.
#[test]
fn a_run_costs_the_same_for_each_scenario() {
    let Some(_turn) = chrome() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("bitti-linear-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let timed = |n: usize| {
        let file = dir.join(format!("s{n}.bt"));
        let text: String = (0..n)
            .map(|i| {
                format!(
                    "scenario \"s{i}\"\n  goto \"data:text/html,<p>hi {i}</p>\"\n  expect --selector \"p\" visible\n\n"
                )
            })
            .collect();
        std::fs::write(&file, text).unwrap();
        let started = std::time::Instant::now();
        let (code, out) = bitti(&["run", &file.display().to_string(), "--workers", "1"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains(&format!("{n} passed, 0 failed")), "{out}");
        started.elapsed()
    };
    let (small, large) = (timed(10), timed(40));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(0.001);
    assert!(ratio < 8.0, "10 scenarios {small:?}, 40 {large:?}: {ratio:.1}x");
    std::fs::remove_dir_all(&dir).ok();
}

/// A run whose browser never starts says so and exits 3 ("the browser or the
/// app server didn't start"), with the scenarios counted as not run, not
/// the 0 of a run that passed.
#[test]
fn a_browser_that_never_starts_is_an_exit_code_not_a_pass() {
    let dir = std::env::temp_dir().join(format!("bt-nobrowser-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bt = dir.join("t.bt");
    std::fs::write(&bt, "scenario \"A\"\n  goto /\nscenario \"B\"\n  goto /\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", bt.to_str().unwrap(), "--base-url", "http://127.0.0.1:9"])
        .env("CHROME_BIN", dir.join("no-such-chrome"))
        .output()
        .expect("run bitti");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{text}");
    assert!(text.contains("the browser didn't start") && text.contains("2 scenarios not run"), "{text}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn routes_replay_a_har_and_change_real_responses() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/har.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("2 passed, 0 failed"), "{out}");
}

#[test]
fn a_permissions_step_grants_denies_and_resets() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&["run", "tests/fixtures/permissions.bt", "--base-url", &site.1]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed"), "{out}");
}

/// Reads a child's stdout and stderr as lines on one channel.
fn lines_of(child: &mut Child) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let tx2 = tx.clone();
    std::thread::spawn(move || BufReader::new(out).lines().map_while(Result::ok).for_each(|l| drop(tx.send(l))));
    std::thread::spawn(move || BufReader::new(err).lines().map_while(Result::ok).for_each(|l| drop(tx2.send(l))));
    rx
}

/// Waits until every needle has been in a line, up to `secs`; stdout and
/// stderr come on two pipes, so their lines can arrive in either order.
fn wait_all(rx: &std::sync::mpsc::Receiver<String>, seen: &mut Vec<String>, needles: &[&str], secs: u64) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !needles.iter().all(|n| seen.iter().any(|l| l.contains(n))) {
        let Some(left) = end.checked_duration_since(std::time::Instant::now()) else { return false };
        match rx.recv_timeout(left) {
            Ok(l) => seen.push(l),
            Err(_) => return false,
        }
    }
    true
}

#[test]
fn watch_runs_again_only_the_file_that_changed() {
    let Some(_turn) = chrome() else { return };
    let dir = std::env::temp_dir().join(format!("bitti-watch-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let py = if cfg!(windows) { "python" } else { "python3" };
    let scenario = |name: &str| format!("scenario \"{name}\"\n  exec {py} -c \"print(1)\"\n    expect exit 0\n");
    std::fs::write(dir.join("a.bt"), scenario("alpha")).unwrap();
    std::fs::write(dir.join("b.bt"), scenario("beta")).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["run", "a.bt", "b.bt", "--watch", "--evidence", &tempfile_dir()])
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run bitti --watch");
    let rx = lines_of(&mut child);
    let mut seen = Vec::new();
    let first = wait_all(&rx, &mut seen, &["watching 2 files", "2 passed"], 60);
    assert!(first, "{seen:#?}");
    assert!(seen.iter().any(|l| l.contains("PASS") && l.contains("alpha")), "{seen:#?}");
    assert!(seen.iter().any(|l| l.contains("PASS") && l.contains("beta")), "{seen:#?}");
    seen.clear();
    // The size changes too, so a coarse file clock cannot hide the edit.
    std::fs::write(dir.join("b.bt"), scenario("beta again")).unwrap();
    let second = wait_all(&rx, &mut seen, &["changed: ", "beta again", "1 passed", "watching 2 files"], 60);
    // Anything else the rerun printed (an alpha would be a wrong rerun).
    while let Ok(l) = rx.recv_timeout(std::time::Duration::from_millis(300)) {
        seen.push(l);
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(second, "{seen:#?}");
    assert!(seen.iter().any(|l| l.starts_with("changed: ") && l.contains("b.bt") && !l.contains("a.bt")), "{seen:#?}");
    assert!(seen.iter().any(|l| l.contains("PASS") && l.contains("beta again")), "{seen:#?}");
    assert!(!seen.iter().any(|l| l.contains("alpha")), "the untouched file ran again: {seen:#?}");
}

#[test]
fn merge_joins_the_jsonl_of_shards_into_one_report() {
    let Some(_turn) = chrome() else { return };
    let Some(site) = own_site() else { return };
    let dir = std::env::temp_dir().join(format!("bitti-merge-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut total = 0;
    for i in 1..=2 {
        let shard = format!("{i}/2");
        let (_, out) = bitti(&[
            "run",
            "tests/fixtures/failing.bt",
            "tests/fixtures/if.bt",
            "--base-url",
            &site.1,
            "--shard",
            &shard,
            "--reporter",
            "jsonl",
        ]);
        let jsonl: String = out.lines().filter(|l| l.starts_with('{')).map(|l| format!("{l}\n")).collect();
        total += jsonl.matches("\"event\":\"scenario\"").count();
        std::fs::write(dir.join(format!("shard-{i}.jsonl")), jsonl).unwrap();
    }
    assert!(total >= 2, "{total}");
    let junit = dir.join("merged.xml");
    let html = dir.join("merged.html");
    let merged = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(["merge", dir.to_str().unwrap(), "--junit", junit.to_str().unwrap(), "--html", html.to_str().unwrap()])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run bitti merge");
    let code = merged.status.code().unwrap_or(-1);
    let out = String::from_utf8_lossy(&merged.stdout).to_string() + &String::from_utf8_lossy(&merged.stderr);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("failed in"), "{out}");
    let xml = std::fs::read_to_string(&junit).unwrap();
    assert_eq!(xml.matches("<testcase").count(), total, "{xml}");
    assert!(std::fs::read_to_string(&html).unwrap().contains("<html"), "no html");
    let _ = std::fs::remove_dir_all(&dir);
}

/// bitti in `dir` (its own `.bitti/`, so its last run is its own).
fn bitti_in(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bitti"))
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .expect("run bitti");
    let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

/// A directory of the test's own.
fn own_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bitti-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Scenarios of `http` steps alone never start a browser: they pass with
/// one that can't start, in well under a second each.
#[test]
fn api_scenarios_run_with_no_browser() {
    let Some(site) = own_site() else { return };
    let secrets = secrets_file();
    let nothing = "/nonexistent/chrome";
    let started = std::time::Instant::now();
    let (code, out) = bitti(&[
        "run",
        "tests/fixtures/api/api.bt",
        "--base-url",
        &site.1,
        "--secrets",
        &secrets,
        "--chromium",
        nothing,
        "--workers",
        "3",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("8 passed, 1 skipped, 0 failed"), "{out}");
    assert!(out.contains("(no browser)"), "{out}");
    assert!(out.contains("SKIP  tests/fixtures/api/api.bt:44  Not written yet  (@fixme)"), "{out}");
    // print: a part of the answer, pretty; a line with values filled in.
    assert!(out.contains("\"city\": \"Oslo\""), "{out}");
    assert!(out.contains("signed in as 200"), "{out}");
    // The secret never shows.
    assert!(!out.contains("hunter22"), "{out}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());

    let (code, out) = bitti(&["run", "tests/fixtures/api/api-fail.bt", "--base-url", &site.1, "--chromium", nothing]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("0 passed, 6 failed"), "{out}");
    assert!(out.contains("the GraphQL answer has 1 error(s): Cannot query field 'nope' on type 'Query'"), "{out}");
    assert!(
        out.contains("api-fail.bt.snapshots/search.json differs: $.n: 0, was 1; $.q: \"boots\", was \"shoes\""),
        "{out}"
    );
    // What came back, under the check that read it.
    assert!(out.contains("expected json $.q is \"shoes\"; saw \"boots\""), "{out}");
    assert!(out.contains("last http: GET ") && out.contains("/api/search?q=boots -> 200 ("), "{out}");
    assert!(out.contains("{\"q\": \"boots\", \"n\": 0}"), "{out}");
    assert!(
        out.contains("to match schema items.schema.json; it doesn't: $[1].price: expected number, got string \"5\""),
        "{out}"
    );
    assert!(out.contains("retry: still failing after 2 tries"), "{out}");
    assert!(out.contains("/api/profile answered 401: {\"error\": \"signed out\"}"), "{out}");
}

/// One file of both kinds: the page scenario starts signed in from an
/// auth that signed in over `http` alone.
#[test]
fn an_api_sign_in_carries_into_the_browser() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let secrets = secrets_file();
    let (code, out) =
        bitti(&["run", "tests/fixtures/api/mixed.bt", "--base-url", &site.1, "--secrets", &secrets, "-j", "2"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("2 passed, 0 failed"), "{out}");
    assert!(!out.contains("(no browser)"), "{out}");
}

/// Tag expressions, `@only`, `--grep-invert`, `--max-failures` and
/// `--last-failed`, on scenarios that need no browser.
#[test]
fn run_control_picks_and_stops() {
    let Some(site) = own_site() else { return };
    let dir = own_dir("control");
    std::fs::write(
        dir.join("t.bt"),
        "scenario \"one\" @smoke\n  http GET /api/search --query q=1\n  expect json $.q is 1\n\
         scenario \"two\" @smoke @slow\n  http GET /api/search --query q=2\n  expect json $.q is 3\n\
         scenario \"three\" @wip\n  http GET /api/search --query q=3\n  expect json $.q is 4\n\
         scenario \"four\"\n  http GET /api/search --query q=4\n  expect json $.q is 4\n",
    )
    .unwrap();
    let names = |out: &str| -> Vec<String> {
        out.lines()
            .filter_map(|l| l.split("  ").nth(1).map(|n| n.split(" @").next().unwrap_or(n).to_string()))
            .collect()
    };
    let (_, out) = bitti_in(&dir, &["list", "t.bt", "--tag", "smoke and not slow"], &[]);
    assert_eq!(names(&out), ["one"], "{out}");
    let (_, out) = bitti_in(&dir, &["list", "t.bt", "--tag", "not (smoke or wip)", "--tag", "wip"], &[]);
    assert_eq!(names(&out), ["three", "four"], "{out}");
    let (_, out) = bitti_in(&dir, &["list", "t.bt", "--grep-invert", "o"], &[]);
    assert_eq!(names(&out), ["three"], "{out}");
    let (code, out) = bitti_in(&dir, &["list", "t.bt", "--tag", "smoke and"], &[]);
    assert_eq!(code, 2, "{out}");

    // One failure stops the run: the rest are not started. (One worker: with
    // more, a scenario already running when the first fails still finishes.)
    let (code, out) = bitti_in(&dir, &["run", "t.bt", "--base-url", &site.1, "-x", "--workers", "1"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("2 scenarios not run: 1 scenario failed (--max-failures 1)"), "{out}");
    // The whole run, then only what failed in it.
    let (code, _) = bitti_in(&dir, &["run", "t.bt", "--base-url", &site.1], &[]);
    assert_eq!(code, 1);
    let (code, out) = bitti_in(&dir, &["run", "t.bt", "--base-url", &site.1, "--last-failed"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("0 passed, 2 failed"), "{out}");
    assert!(out.contains("FAIL  t.bt:4  two") && out.contains("FAIL  t.bt:7  three"), "{out}");
    std::fs::write(dir.join("t.bt"), "scenario \"one\"\n  http GET /api/search --query q=1\n  expect json $.q is 1\n")
        .unwrap();
    let (code, out) = bitti_in(&dir, &["run", "t.bt", "--base-url", &site.1, "--last-failed"], &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("no scenario failed in the last run"), "{out}");

    // @only focuses a run, unless it is forbidden (as under CI).
    std::fs::write(dir.join("t.bt"), "scenario \"one\"\n  wait --ms 1\nscenario \"two\" @only\n  wait --ms 1\n")
        .unwrap();
    let (code, out) = bitti_in(&dir, &["run", "t.bt", "--allow-only"], &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 passed, 0 failed") && out.contains("two"), "{out}");
    let (code, out) = bitti_in(&dir, &["run", "t.bt"], &[("CI", "true")]);
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("@only is on 1 scenario") && out.contains("t.bt:3  two"), "{out}");
}

/// `--slowmo` waits before each action; a `pause` step stops the run with
/// the browser open to a second session.
#[test]
fn slowmo_and_a_pause_step() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let dir = own_dir("slowmo");
    std::fs::write(
        dir.join("s.bt"),
        "scenario \"three actions\"\n  goto /\n  click --text Late\n  goto /done\n  expect text Thanks\n",
    )
    .unwrap();
    let ms = |out: &str| -> f64 {
        let line = out.lines().find(|l| l.contains("\"event\":\"scenario\"")).unwrap_or_else(|| panic!("{out}"));
        serde_json::from_str::<serde_json::Value>(line).unwrap()["ms"].as_f64().unwrap()
    };
    let (code, out) =
        bitti_in(&dir, &["run", "s.bt", "--base-url", &site.1, "--slowmo", "300", "--reporter", "jsonl"], &[]);
    assert_eq!(code, 0, "{out}");
    assert!(ms(&out) >= 900.0, "three actions, 300 ms before each: {}", ms(&out));

    std::fs::write(dir.join("p.bt"), "scenario \"stops for a look\"\n  goto /done\n  pause\n  expect text Thanks\n")
        .unwrap();
    let (code, out) = bitti_in(&dir, &["run", "p.bt", "--base-url", &site.1, "-j", "2"], &[("BITTI_PAUSE_S", "1")]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("a `pause` step waits for Enter: --workers 2 becomes 1"), "{out}");
    assert!(out.contains("PAUSED  p.bt:3  the browser stays as the steps before left it"), "{out}");
    assert!(out.contains("--attach ws://"), "{out}");
}

/// A JSON snapshot: written the first time (and the check fails, so a
/// person reads it), the same answer then holds, a changed one fails until
/// `--update-snapshots` takes it.
#[test]
fn json_snapshots_are_written_then_held() {
    let Some(site) = own_site() else { return };
    let dir = own_dir("snapshot");
    let bt = "scenario \"s\"\n  http GET /api/search --query q=${q:-a} --query n=1\n  expect json snapshot search --ignore $.n\n";
    std::fs::write(dir.join("s.bt"), bt).unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec!["run", "s.bt", "--base-url", site.1.as_str()];
        args.extend_from_slice(extra);
        bitti_in(&dir, &args, &[])
    };
    let (code, out) = run(&[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no baseline yet: wrote it, read it and run again:"), "{out}");
    let baseline = dir.join("s.bt.snapshots/search.json");
    assert_eq!(std::fs::read_to_string(&baseline).unwrap(), "{\n  \"q\": \"a\"\n}\n");
    assert_eq!(run(&[]).0, 0);
    let (code, out) = run(&["--var", "q=b"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("differs: $.q: \"b\", was \"a\""), "{out}");
    let (code, out) = run(&["--var", "q=b", "--update-snapshots"]);
    assert_eq!(code, 0, "{out}");
    assert!(std::fs::read_to_string(&baseline).unwrap().contains("\"b\""));
}

/// `clock` steps: a page's time at the scenario's command, and the next
/// scenario on the same worker back on real time.
#[test]
fn a_scenario_controls_the_pages_clock() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    for workers in ["1", "2"] {
        let (code, out) = bitti(&["run", "tests/fixtures/clock.bt", "--base-url", &site.1, "--workers", workers]);
        assert_eq!(code, 0, "workers {workers}: {out}");
        assert!(out.contains("2 passed, 0 failed"), "workers {workers}: {out}");
    }
}

/// `--scenario-timeout-ms`: a scenario's steps stop when its time is up,
/// a step's own `--timeout-ms` included; `@slow` gets three times as long.
#[test]
fn a_scenario_stops_when_its_time_is_up() {
    let (Some(_turn), Some(site)) = (chrome(), own_site()) else { return };
    let (code, out) = bitti(&[
        "run",
        "tests/fixtures/timeout.bt",
        "--base-url",
        &site.1,
        "--scenario-timeout-ms",
        "1500",
        "--workers",
        "1",
    ]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("2 passed, 1 failed"), "{out}");
    assert!(out.contains("FAIL  tests/fixtures/timeout.bt:1  Waits longer than its scenario may"), "{out}");
    // In the long wait, or (a slow machine's goto) before it.
    assert!(out.contains("the scenario's 1.5 s (--scenario-timeout-ms 1500) ran out"), "{out}");
    assert!(out.contains("PASS  tests/fixtures/timeout.bt:10  Slow, so three times the time"), "{out}");
    // The wait was cut to what the scenario had left, not its own 10 s.
    // (Read from what bitti says, not from a clock here: the browser's
    // start is in this test's time and takes 10 s and more on a loaded
    // Windows runner.)
    let cut = out.split("within ").nth(1).and_then(|s| s.split(" ms").next()).and_then(|n| n.parse::<u64>().ok());
    assert!(cut.map_or(out.contains("out of time before this step"), |ms| ms <= 1500), "{out}");
    // `--global-timeout-ms`: the scenario running then stops, the rest don't start, the run fails.
    // The time has to run out while a scenario runs, so after the browser
    // is up: when it ran out before that (all three not run, which is
    // right too), the run is tried again with twice the time.
    for seconds in [1, 2, 4, 8] {
        let ms = (seconds * 1000).to_string();
        let (code, out) = bitti(&[
            "run",
            "tests/fixtures/timeout.bt",
            "--base-url",
            &site.1,
            "--global-timeout-ms",
            &ms,
            "--workers",
            "1",
        ]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains(&format!("not run: the run's {seconds} s ran out (--global-timeout-ms {ms})")), "{out}");
        if out.contains("3 scenarios not run") && seconds < 8 {
            continue;
        }
        assert!(out.contains(&format!("the run's {seconds} s (--global-timeout-ms {ms}) ran out")), "{out}");
        assert!(out.contains("2 scenarios not run"), "{out}");
        break;
    }
}
