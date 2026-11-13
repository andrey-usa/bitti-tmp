#!/usr/bin/env python3
"""Write bitti's benchmark docs from one TodoMVC bench run.

    gen_docs.py RUN_ID | results.json [--readme README.md]
                [--page docs/benchmarks.md] [--losses docs/losses.md]

RUN_ID: the run's `bench json i/N` notices are joined back into its results
through the checks API (no token, no artifact download); a path reads a file
bench.py --json wrote. Writes README.md's `## Benchmarks` (each tool's
default and one worker each, on Chrome, and what Playwright did better),
the full page and the losses page. No number in them is typed by hand.
"""
import json
import os
import re
import sys
import urllib.request

REPO = "andrey-usa/bitti"
SITE = "https://andrey-usa.github.io/bitti/"
GROUPS = {"serial": "one worker each", "default": "each tool's default workers"}
TIE = 0.02  # closer than 2% is a tie, not a loss: about what the same run varies by


def fetch(run):
    api = f"https://api.github.com/repos/{REPO}"

    def get(url):
        with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "gen_docs"})) as r:
            return json.load(r)

    parts, total = {}, 0
    for job in get(f"{api}/actions/runs/{run}/jobs?per_page=100")["jobs"]:
        for n in get(f"{api}/check-runs/{job['id']}/annotations?per_page=100"):
            m = re.fullmatch(r"bench json (\d+)/(\d+)", n.get("title") or "")
            if m:
                parts[int(m.group(1))] = n["message"].strip()
                total = int(m.group(2))
    if not total or sorted(parts) != list(range(1, total + 1)):
        sys.exit(f"run {run}: no complete `bench json` notices ({sorted(parts)} of {total})")
    doc = json.loads("".join(parts[i] for i in range(1, total + 1)))
    doc["run_id"] = doc.get("run_id") or str(run)
    return doc


def main():
    args = sys.argv[1:]
    opts = {}
    for flag in ("--readme", "--page", "--losses"):
        if flag in args:
            i = args.index(flag)
            opts[flag] = args[i + 1]
            del args[i:i + 2]
    if not args:
        sys.exit(__doc__)
    doc = json.load(open(args[0])) if os.path.exists(args[0]) else fetch(args[0])
    run = doc.get("run_id")
    if not run:
        sys.exit("these results name no run: the docs cite one for every number")
    v, env = doc["versions"], doc["env"]
    link = f"[bench run {run}](https://github.com/{REPO}/actions/runs/{run})"
    date = doc["generated_utc"][:10]
    machine = f"{env['cpu']}, {env['cores']} cores, {env['memory_gb']} GB"
    names = {"bitti": v["bitti"] or "bitti",
             "playwright-node": f"Playwright Test {v['playwright']}, Node {(v['node'] or '').lstrip('v')}",
             "playwright-bun": f"Playwright Test {v['playwright']}, Bun {v['bun']}"}
    label = {"chrome": "Chrome for Testing", "chrome-headless-shell": "chrome-headless-shell"}
    browsers = {b: f"{label.get(b, b)} {(re.search(r'[0-9][0-9.]+', ver or '') or [''])[0]}".strip()
                for b, ver in v["browsers"].items()}

    def rows(browser, groups):
        return [r for g in groups for r in doc["rows"] if r["browser"] == browser and r["group"] == g]

    def table(rs, pss):
        head = ["| tool | workers | time, median (range) | CPU | runner memory | browser memory |",
                "|---|---|---|---|---|---|"]
        fastest = {g: min(r["wall_s"]["median"] for r in rs if r["group"] == g) for g in {r["group"] for r in rs}}
        out, last = list(head), None
        for r in rs:
            if r["group"] != last:
                out.append(f"| **{GROUPS.get(r['group'], r['group'])}** | | | | | |")
                last = r["group"]
            w = r["wall_s"]
            t = f"{w['median']:.2f} s"
            t = f"**{t}**" if w["median"] == fastest[r["group"]] else t
            mem = lambda k: (f"{r['rss_mib'][k]:,.0f} MiB" + (f" ({r['pss_mib'][k]:,.0f} PSS)" if pss and r.get("pss_mib") else ""))
            out.append(f"| {names[r['tool']]} | {r['workers']} | {t} ({w['min']:.2f}-{w['max']:.2f}) | "
                       f"{r['cpu_s']:.1f} s | {mem('runner')} | {mem('browser')} |")
        return out

    losses = []
    for r in doc["rows"]:
        if r["tool"] == "bitti":
            continue
        b = next(x for x in doc["rows"] if x["tool"] == "bitti" and x["browser"] == r["browser"] and x["group"] == r["group"])
        where = f"{browsers[r['browser']]}, {GROUPS.get(r['group'], r['group'])}"
        for what, get, fmt in (("time", lambda x: x["wall_s"]["median"], lambda x: f"{x:.2f} s"),
                               ("CPU", lambda x: x["cpu_s"], lambda x: f"{x:.1f} s"),
                               ("runner memory", lambda x: x["rss_mib"]["runner"], lambda x: f"{x:,.0f} MiB"),
                               ("browser memory", lambda x: x["rss_mib"]["browser"], lambda x: f"{x:,.0f} MiB"),
                               ("browser PSS", lambda x: (x.get("pss_mib") or {}).get("browser"), lambda x: f"{x:,.0f} MiB")):
            mine, theirs = get(b), get(r)
            if mine is not None and theirs is not None and theirs < mine * (1 - TIE):
                losses.append(f"{what} on {where}: {names[r['tool']]} {fmt(theirs)}, bitti {fmt(mine)}")
    loss_line = ("Where Playwright did better in this run (by more than 2%): " + "; ".join(losses) + "."
                 if losses else "Playwright did better than bitti by more than 2% on no measure in this run.")
    first = next(iter(browsers))
    what = (f"Playwright's own TodoMVC suite (`demo-todo-app.spec.ts`, {doc['tests']} tests) and its "
            f"one-to-one bitti port, on the same local app and the same browser binary, headless, every "
            f"run passing all {doc['tests']} tests")
    note = ("Time is the whole command, the median of {runs} runs taken in turns; CPU is user and system "
            "time of every process. Runner memory is bitti's own process, or Playwright's node or bun "
            "processes; browser memory is every browser process; each is its peak RSS, median of the runs.").format(runs=doc["runs"])

    readme = ["## Benchmarks", "",
              f"{what}, on GitHub's `ubuntu-24.04` ({machine}), {link}, {date}, on {browsers[first]}:", ""
              ] + table(rows(first, ["default", "serial"]), False) + ["", note, "", loss_line, "",
              f"chrome-headless-shell, PSS, every run and the method: [Benchmarks]({SITE}benchmarks/results.html).", ""]
    if "--readme" in opts:
        p = opts["--readme"]
        text = open(p).read()
        new = "\n".join(readme)
        if "## Benchmarks" in text:
            start = text.index("## Benchmarks")
            nxt = text.find("\n## ", start + 1)
            text = text[:start] + new + ("\n" + text[nxt + 1:] if nxt >= 0 else "")
        else:  # first time: before Documentation
            i = text.index("## Documentation")
            text = text[:i] + new + "\n" + text[i:]
        open(p, "w").write(text)

    page = ["# Benchmarks", "", f"Numbers: {link}, {date}.", "",
            f"{what}. Run on GitHub's `ubuntu-24.04`: {machine}, {env['os']}.", "", note,
            "PSS, in brackets, splits each shared page between the processes that share it: the memory "
            f"a tool really takes. It comes from {doc['pss_runs']} more runs whose times are not used.", ""]
    for b in browsers:
        page += [f"## {browsers[b]}", ""] + table(rows(b, ["default", "serial"]), True) + [""]
    page += ["## Every run", "", "| browser | workers | tool | each run's time |", "|---|---|---|---|"]
    for r in doc["rows"]:
        page.append(f"| {browsers[r['browser']]} | {GROUPS.get(r['group'], r['group'])} | {names[r['tool']]} | "
                    + ", ".join(f"{x:.2f} s" for x in r["wall_s"]["all"]) + " |")
    page += ["", "## Versions and method", "",
             f"bitti built from the run's commit ({v['bitti']}); @playwright/test {v['playwright']} from npm, "
             f"its latest at the run; Node {v['node']}; Bun {v['bun']}, Playwright's CLI run with `bun --bun`, "
             "so its workers are Bun too.", "",
             "The app is tastejs/todomvc's React example at the last commit before it was replaced, with the "
             "test ids, the edit field's label and the toggle text demo.playwright.dev has, and a newer "
             "todomvc-app-css (`bench/todomvc/app.patch`). The spec is create-playwright's, checked against "
             "its SHA-256, with only its URL changed. Playwright runs create-playwright's template config, "
             "chromium only, trace off, the list reporter. The port has a scenario for every test and an "
             "`expect` for every assertion; where bitti has no list form or chained locator, the check is "
             "written out item by item (`bench/todomvc/bt/`).", "",
             "Every tool starts from a warm-up round that is not counted; then the tools take turns, the "
             "order flipped each round. Each run is sampled from `/proc` every 20 ms "
             "(`bench/todomvc/memsample.py`), the whole process tree, with orphaned browser processes kept "
             "in it.", "",
             "To run it yourself, with bitti built, Node, Bun, and a Chrome for Playwright and bitti:", "",
             "```sh",
             "python3 bench/todomvc/setup.py --work /tmp/todomvc",
             "python3 -m http.server 8765 --bind 127.0.0.1 --directory /tmp/todomvc/www &",
             "python3 bench/todomvc/bench.py --bitti target/release/bitti --work /tmp/todomvc \\",
             "  --browser chrome=/path/to/chrome --browser-dir /path/to --bun \"$(command -v bun)\" --json results.json",
             "python3 bench/todomvc/gen_docs.py results.json --page bench.md",
             "```", ""]
    if "--page" in opts:
        open(opts["--page"], "w").write("\n".join(page))
    lose = ["# Where bitti is not ahead", "", f"Numbers: {link}, {date}.", "",
            "Each measure of the benchmarks on which Playwright beat bitti in this run by more than 2%, "
            "with the same browser and the same workers setting: time, CPU, runner and browser memory, "
            "browser PSS. Closer than that is a tie.", ""]
    lose += [f"- {x[0].upper() + x[1:]}." for x in losses] or ["- None in this run."]
    if "--losses" in opts:
        open(opts["--losses"], "w").write("\n".join(lose) + "\n")
    print(f"benchmark docs from run {run}: {len(doc['rows'])} rows, {len(losses)} losses")


if __name__ == "__main__":
    main()
