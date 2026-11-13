#!/usr/bin/env python3
"""Put together what the TodoMVC benchmark runs, from pinned sources.

    setup.py --work DIR [--playwright VERSION]

DIR/www/todomvc   tastejs/todomvc's React example at TODOMVC_COMMIT, with
                  app.patch: the test ids, the edit field's label and the
                  "Mark all as complete" text demo.playwright.dev has, and
                  todomvc-app-css 2.4.3 (with the old CSS the first item's
                  checkbox sits over the toggle-all input).
DIR/pw            Playwright's own demo-todo-app.spec.ts from the
                  create-playwright package (the newest version that ships
                  it), checked against its SHA-256, pointed at the local app;
                  playwright.config.ts beside this script; @playwright/test
                  at --playwright (default: the latest release).

Needs git and npm. Serve DIR/www on 127.0.0.1:8765, as the specs expect:
python3 -m http.server 8765 --bind 127.0.0.1 --directory DIR/www
"""
import argparse
import hashlib
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile

HERE = pathlib.Path(__file__).resolve().parent
TODOMVC = "https://github.com/tastejs/todomvc.git"
TODOMVC_COMMIT = "114bef269c4b1c2564a2f35dcbd22b627d5f7b27"
SPEC_PACKAGE = "create-playwright@1.17.137"
SPEC_SHA256 = "3b8679bfa675efc200f404b6bb77b367710a572d5c9d8c421b711f015244b596"
SPEC_URL = "https://demo.playwright.dev/todomvc"
LOCAL_URL = "http://127.0.0.1:8765/todomvc/"


def run(*cmd, cwd=None):
    subprocess.run(cmd, cwd=cwd, check=True)


def app(work):
    src = work / "todomvc-src"
    if src.exists():
        shutil.rmtree(src)
    run("git", "clone", "-q", "--filter=blob:none", "--no-checkout", TODOMVC, str(src))
    run("git", "sparse-checkout", "set", "examples/react", cwd=src)
    run("git", "checkout", "-q", TODOMVC_COMMIT, cwd=src)
    dest = work / "www" / "todomvc"
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(src / "examples" / "react", dest)
    run("patch", "-s", "-p1", "-d", str(dest), "-i", str(HERE / "app.patch"))


def spec(work):
    pw = work / "pw"
    (pw / "tests").mkdir(parents=True, exist_ok=True)
    out = subprocess.run(["npm", "pack", "--silent", SPEC_PACKAGE], cwd=work, check=True,
                         capture_output=True, text=True).stdout.split()[-1]
    with tarfile.open(work / out) as t:
        data = t.extractfile("package/assets/demo-todo-app.spec.ts").read()
    got = hashlib.sha256(data).hexdigest()
    if got != SPEC_SHA256:
        sys.exit(f"{SPEC_PACKAGE} demo-todo-app.spec.ts: sha256 {got}, expected {SPEC_SHA256}")
    text = data.decode()
    if text.count(SPEC_URL) != 1:
        sys.exit(f"the spec no longer names {SPEC_URL} once")
    (pw / "tests" / "demo-todo-app.spec.ts").write_text(text.replace(SPEC_URL, LOCAL_URL))
    shutil.copy(HERE / "playwright.config.ts", pw / "playwright.config.ts")
    return pw


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--work", required=True)
    ap.add_argument("--playwright", default="latest")
    a = ap.parse_args()
    work = pathlib.Path(a.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    app(work)
    pw = spec(work)
    (pw / "package.json").write_text('{"name": "todomvc-bench-pw", "private": true}\n')
    run("npm", "install", "--silent", "--no-audit", "--no-fund", f"@playwright/test@{a.playwright}", cwd=pw)
    version = subprocess.run(["node", "-p", "require('@playwright/test/package.json').version"], cwd=pw,
                             check=True, capture_output=True, text=True).stdout.strip()
    print(f"todomvc bench ready in {work}: the app at {TODOMVC_COMMIT[:12]}, the spec from {SPEC_PACKAGE}, "
          f"@playwright/test {version}")


if __name__ == "__main__":
    main()
