#!/usr/bin/env python3
"""bitti against Playwright Test, on Node and on Bun: Playwright's own TodoMVC suite.

    bench.py --bitti BIN --work DIR --browser NAME=PATH [--browser NAME=PATH]
             --browser-dir DIR [--bun BIN] [--groups serial,default] [--runs 5]
             [--pss-runs 2] [--json OUT] [--notices]

The 24 tests of Playwright's demo-todo-app.spec.ts (DIR/pw, see setup.py)
and their one-to-one bitti port (bt/), against the same local TodoMVC
(DIR/www, served on 127.0.0.1:8765) and the same browser binary, headless.

Groups: `serial` runs every tool on one worker; `default` lets each tool pick
its own (Playwright: half the CPUs; bitti: half the CPUs, at most 8). Each
group of each browser gets a warm-up round, not counted, then --runs rounds
in which the tools take turns, the order flipped each round, so a slow
minute on the machine falls on all of them. Every run must pass all 24
tests.

Every run goes through memsample.py: wall time of the whole command, CPU
(user and system) of the whole process tree, and the peak RSS of the runner
(bitti, or Playwright's node or bun processes) and of the browser (every
process under --browser-dir), each at its own peak. --pss-runs more rounds
read PSS too (shared pages split between the processes that share them);
their times are not used.

--json writes the results; --notices prints them as `bench json i/N`
notices for gen_docs.py to read back from the run.
"""
import argparse
import datetime
import json
import os
import platform
import re
import statistics
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TESTS = 24
UNSET = ["NODE_OPTIONS", "BUN_OPTIONS", "BUN_FEATURE_FLAG_DISABLE_STANDALONE_MADVISE"]
PW_ARGS = ["test", "--trace", "off", "--reporter", "list"]


def configs(a, browser):
    pw = os.path.join(a.work, "pw")
    cli = "./node_modules/.bin/playwright"
    bt = [a.bitti, "run", ".", "--chromium", browser]
    tools = {
        "bitti": (os.path.join(HERE, "bt"), bt, ["--workers", "1"]),
        "playwright-node": (pw, [cli] + PW_ARGS, ["--workers=1"]),
    }
    if a.bun:
        # --bun: Bun runs the CLI despite its node shebang, and the runner
        # forks its workers with Bun too.
        tools["playwright-bun"] = (pw, [a.bun, "--bun", cli] + PW_ARGS, ["--workers=1"])
    return tools


def passed(tool, log):
    if tool == "bitti":
        m = re.search(r"^(\d+) passed, (\d+) failed", log, re.M)
        return (int(m.group(1)), int(m.group(2))) if m else (0, -1)
    p = re.search(r"^\s*(\d+) passed", log, re.M)
    f = re.search(r"^\s*(\d+) failed", log, re.M)
    return (int(p.group(1)) if p else 0, int(f.group(1)) if f else 0)


def workers(tool, log, group):
    if group == "serial":
        return 1
    if tool == "bitti":
        return max(1, min(8, (os.cpu_count() or 2) // 2, TESTS))
    m = re.search(r"using (\d+) workers?", log)
    return int(m.group(1)) if m else None


def one(a, browser, bdir, tool, cfg, group, tag, pss):
    cwd, cmd, serial = cfg
    cmd = cmd + (serial if group == "serial" else [])
    out = os.path.join(a.out, f"{tag}.json")
    log = os.path.join(a.out, f"{tag}.log")
    sampler = [sys.executable, os.path.join(HERE, "memsample.py"), "--out", out, "--log", log, "--cwd", cwd,
               "--browser-dir", a.browser_dir, "--setenv", f"BENCH_CHROMIUM={browser}"]
    for n in UNSET:
        sampler += ["--unset", n]
    if pss:
        sampler += ["--pss-every", "5"]
    subprocess.run(sampler + ["--"] + cmd, check=True, stdout=subprocess.DEVNULL)
    r = json.load(open(out))
    text = open(log, errors="replace").read()
    ok, bad = passed(tool, text)
    if r["exit_code"] != 0 or ok != TESTS or bad:
        sys.exit(f"{tag}: exit {r['exit_code']}, {ok} passed, {bad} failed (of {TESTS}); log: {log}\n{text[-3000:]}")
    r["workers"] = workers(tool, text, group)
    print(f"{tag}: {r['wall_s']:.2f} s, runner {r['peak_rss_per_kind_mib'].get('runner', 0):.0f} MiB, "
          f"browser {r['peak_rss_per_kind_mib'].get('browser', 0):.0f} MiB", flush=True)
    return r


def med(xs):
    return round(statistics.median(xs), 3) if xs else None


def version(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.strip().splitlines()[0]
    except (OSError, IndexError, subprocess.TimeoutExpired):
        return None


def notices(doc, title="bench json"):
    text = json.dumps(doc, separators=(",", ":"))
    parts, i = [], 0
    while i < len(text):
        j = min(i + 3500, len(text))
        while j < len(text) and j > i + 1 and (text[j - 1] == " " or text[j] == " "):
            j -= 1
        parts.append(text[i:j])
        i = j
    if len(parts) > 10:
        sys.exit(f"::error::{title}: {len(parts)} notices needed, GitHub keeps 10 per step")
    for n, p in enumerate(parts, 1):
        print(f"::notice title={title} {n}/{len(parts)}::" + p.replace("%", "%25").replace("\n", "%0A"))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--bitti", required=True)
    ap.add_argument("--work", required=True, help="the directory setup.py filled")
    ap.add_argument("--browser", action="append", required=True, help="NAME=PATH, e.g. chrome=/path/to/chrome")
    ap.add_argument("--browser-dir", required=True, help="every process under it counts as the browser")
    ap.add_argument("--bun")
    ap.add_argument("--groups", default="serial,default")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--pss-runs", type=int, default=2)
    ap.add_argument("--out", default="bench-out")
    ap.add_argument("--json")
    ap.add_argument("--notices", action="store_true")
    a = ap.parse_args()
    a.bitti = os.path.abspath(a.bitti)
    a.work = os.path.abspath(a.work)
    os.makedirs(a.out, exist_ok=True)
    pw = os.path.join(a.work, "pw")
    browsers = dict(b.split("=", 1) for b in a.browser)
    doc = {
        "generated_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "run_id": os.environ.get("GITHUB_RUN_ID"), "commit": os.environ.get("GITHUB_SHA"),
        "tests": TESTS, "runs": a.runs, "pss_runs": a.pss_runs,
        "env": {"cpu": next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), ""),
                "cores": os.cpu_count(),
                "memory_gb": next((int(l.split()[1]) // 1024 // 1024 for l in open("/proc/meminfo") if l.startswith("MemTotal")), 0),
                "os": platform.platform(terse=True)},
        "versions": {
            "bitti": version([a.bitti, "--version"]),
            "playwright": json.load(open(os.path.join(pw, "node_modules", "@playwright", "test", "package.json")))["version"],
            "node": version(["node", "--version"]),
            "bun": version([a.bun, "--version"]) if a.bun else None,
            "browsers": {n: version([p, "--version"]) for n, p in browsers.items()},
        },
        "rows": [],
    }
    for bname, bpath in browsers.items():
        tools = configs(a, bpath)
        for group in a.groups.split(","):
            names = list(tools)
            got = {n: [] for n in names}
            pss = {n: [] for n in names}
            for rnd in range(1 + a.runs + a.pss_runs):
                order = names if rnd % 2 == 0 else names[::-1]
                for n in order:
                    kind = "warm" if rnd == 0 else ("pss" if rnd > a.runs else "timed")
                    r = one(a, bpath, a.browser_dir, n, tools[n], group, f"{bname}-{group}-{n}-{rnd}-{kind}", kind == "pss")
                    (got if kind == "timed" else pss if kind == "pss" else {}).setdefault(n, []).append(r)
            for n in names:
                rs, ps = got[n], pss[n]
                walls = [r["wall_s"] for r in rs]
                kind = lambda r, k: r["peak_rss_per_kind_mib"].get(k, 0)
                pkind = lambda r, k: (r["peak_pss_per_kind_mib"] or {}).get(k, 0)
                doc["rows"].append({
                    "browser": bname, "group": group, "tool": n, "workers": rs[0]["workers"],
                    "wall_s": {"median": med(walls), "min": min(walls), "max": max(walls), "all": walls},
                    "cpu_s": med([r["cpu_user_s"] + r["cpu_sys_s"] for r in rs]),
                    "rss_mib": {"runner": med([kind(r, "runner") for r in rs]),
                                "browser": med([kind(r, "browser") for r in rs]),
                                "total": med([r["peak_rss_total_mib"] for r in rs])},
                    "pss_mib": {"runner": med([pkind(r, "runner") for r in ps]),
                                "browser": med([pkind(r, "browser") for r in ps]),
                                "total": med([r["peak_pss_total_mib"] or 0 for r in ps])} if ps else None,
                })
    if a.json:
        with open(a.json, "w") as f:
            json.dump(doc, f, indent=1)
    if a.notices:
        notices(doc)


if __name__ == "__main__":
    main()
