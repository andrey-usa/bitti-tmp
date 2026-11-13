//! The README's Quick start, run as it is written: the scenario file of its
//! `text` block (its first line, `# tests/shop.bt`, names the file) is
//! written to a scratch folder, and each `$ bitti …` line of its console
//! block runs there, its output (stdout and stderr) compared with the lines
//! under it. The app is a local page with the button and the counter the
//! scenario names, served at a free port in place of `127.0.0.1:8000`.
//! Times are masked: they are the only parts of the output that change from
//! run to run.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_bitti");
const APP: &str = "http://127.0.0.1:8000";

fn chrome() -> bool {
    let found = navigera::browser::resolve_executable_for(None, true).is_some();
    assert!(found || std::env::var_os("BITTI_REQUIRE_BROWSER").is_none(), "no Chrome/Chromium found");
    found
}

/// A shop page whose "Add to cart" button counts into `#cart-count`, for as
/// long as the test runs.
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        let page = "<!doctype html><html><head><title>Shop</title></head><body>\
                    <button onclick=\"let c = document.getElementById('cart-count'); \
                    c.textContent = String(Number(c.textContent) + 1)\">Add to cart</button>\
                    <p>Cart: <span id=\"cart-count\">0</span></p></body></html>";
        for mut stream in listener.incoming().flatten() {
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
        }
    });
    base
}

/// A time, `119 ms` or `1.2 s` (bitti prints seconds past one), becomes
/// `<n> <unit>`, in brackets or not, so a slow runner's `1.2 s` matches the
/// README's `119 ms`; a Windows path separator becomes `/`.
fn mask(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').collect();
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let bare = word.trim_start_matches('(');
        let number = !bare.is_empty() && bare.bytes().all(|b| b.is_ascii_digit() || b == b'.');
        let unit = words.get(i + 1).map(|next| (next, next.trim_end_matches([')', ','].as_slice())));
        match unit {
            Some((next, u)) if number && ["ms", "s"].contains(&u) => {
                out.push(format!("{}<n>", &word[..word.len() - bare.len()]));
                out.push(format!("<unit>{}", &next[u.len()..]));
                i += 2;
            }
            _ => {
                out.push(word.replace('\\', "/"));
                i += 1;
            }
        }
    }
    out.join(" ")
}

#[test]
fn a_time_in_seconds_masks_like_one_in_milliseconds() {
    assert_eq!(mask("ok  (1.2 s)"), mask("ok  (119 ms)"));
    assert_eq!(mask("1 passed, 0 failed in 1.4 s"), "1 passed, 0 failed in <n> <unit>");
    assert_eq!(mask("run 3 tests"), "run 3 tests");
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bitti-readme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The body of the first ``` block of `kind` in `text`.
fn block<'a>(text: &'a str, kind: &str) -> &'a str {
    let fence = format!("```{kind}\n");
    let open = text.find(&fence).unwrap_or_else(|| panic!("the Quick start has a {kind} block")) + fence.len();
    &text[open..open + text[open..].find("```").unwrap()]
}

#[test]
fn the_readme_quick_start_runs_as_written() {
    if !chrome() {
        eprintln!("skipped: no Chrome/Chromium found");
        return;
    }
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .unwrap()
        // A Windows checkout has CRLF line ends.
        .replace("\r\n", "\n");
    let section = &readme[readme.find("\n## Quick start\n").expect("README.md has a Quick start")..];

    let scenario = block(section, "text");
    let (caption, body) = scenario.split_once('\n').unwrap();
    let file = caption.strip_prefix("# ").expect("the scenario block's first line names its file");
    assert!(body.contains(APP), "the scenario's base is {APP}");
    let dir = scratch();
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body.replace(APP, &serve())).unwrap();

    let lines: Vec<&str> = block(section, "console").lines().collect();
    let (mut i, mut ran) = (0, 0);
    while i < lines.len() {
        let command = lines[i].strip_prefix("$ ").unwrap_or_else(|| panic!("not a command: {}", lines[i]));
        i += 1;
        let start = i;
        while i < lines.len() && !lines[i].starts_with("$ ") {
            i += 1;
        }
        let expected: Vec<String> = lines[start..i].iter().map(|l| mask(l)).collect();
        let mut words = command.split_whitespace();
        assert_eq!(words.next(), Some("bitti"), "the Quick start runs bitti only: {command}");
        let out = Command::new(BIN).args(words).current_dir(&dir).stdin(Stdio::null()).output().unwrap();
        ran += 1;
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        let got: Vec<String> = text.lines().map(mask).collect();
        assert_eq!(got, expected, "README.md's Quick start: `{command}` printed {got:?}, not {expected:?}");
    }
    assert!(ran >= 1, "the Quick start ran no command");
    let _ = std::fs::remove_dir_all(&dir);
}
