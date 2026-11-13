#!/usr/bin/env python3
"""Run one command and measure it: wall-clock time of the whole command and
the peak TOTAL RSS of its whole process tree, sampled from /proc every 20 ms.

Standard library only. Linux only.

- The sampler makes itself a child subreaper (prctl PR_SET_CHILD_SUBREAPER),
  so a process that double-forks or is orphaned (a browser helper, a crash
  handler) is re-parented to the sampler and stays inside the measured tree.
- Each sample walks /proc, finds every descendant of the sampler, and sums
  their resident set sizes (statm field 2 x page size). Shared pages are
  counted once per process, as "total RSS" does.
- Each process is classed by its executable: "browser" (anything under the
  browser directory, /opt/pw-browsers by default), "node", "bun", "bitti",
  "other". Every non-browser process seen is also listed once (pid, ppid,
  executable, command line) in "runner_processes", so a run records which
  runtime its runner and workers really were.
- Wall time runs from just before the command is spawned to its exit.
- CPU time is user+sys of the reaped tree (getrusage RUSAGE_CHILDREN delta).

Usage: memsample.py --out run.json [--log run.log] [--cwd DIR]
                    [--interval-ms 20] [--browser-dir /opt/pw-browsers]
                    [--unset NAME]... [--setenv NAME=VALUE]... -- cmd args...
"""
import argparse
import ctypes
import json
import os
import resource
import subprocess
import sys
import threading
import time

PAGE = os.sysconf("SC_PAGE_SIZE")
PR_SET_CHILD_SUBREAPER = 36


def set_subreaper():
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "prctl(PR_SET_CHILD_SUBREAPER)")


def snapshot():
    """{pid: ppid} for every process, read from /proc/<pid>/stat."""
    out = {}
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        try:
            with open(f"/proc/{name}/stat", "rb") as f:
                data = f.read()
        except OSError:
            continue
        # comm can hold spaces and parens: the fields after the last ')'.
        rest = data[data.rfind(b")") + 2:].split()
        out[int(name)] = int(rest[1])
    return out


def descendants(root, parents):
    children = {}
    for pid, ppid in parents.items():
        children.setdefault(ppid, []).append(pid)
    found, stack = [], [root]
    while stack:
        p = stack.pop()
        for c in children.get(p, ()):
            found.append(c)
            stack.append(c)
    return found


def pss_bytes(pid):
    """Proportional set size (shared pages split among their users)."""
    try:
        with open(f"/proc/{pid}/smaps_rollup", "rb") as f:
            for line in f:
                if line.startswith(b"Pss:"):
                    return int(line.split()[1]) * 1024
    except (OSError, IndexError, ValueError):
        pass
    return 0


def rss_bytes(pid):
    try:
        with open(f"/proc/{pid}/statm", "rb") as f:
            return int(f.read().split()[1]) * PAGE
    except (OSError, IndexError, ValueError):
        return 0


# The test runner's own processes, whatever runs them.
RUNNER_KINDS = ("node", "bun", "bitti")


class Classifier:
    """Read each time, not cached: a pid's executable changes on exec (a
    shell wrapper that execs the browser, npx that execs node)."""

    def __init__(self, browser_dir):
        self.browser_dir = browser_dir.rstrip("/") + "/"

    def __call__(self, pid):
        try:
            exe = os.readlink(f"/proc/{pid}/exe")
        except OSError:
            return "other"
        if exe.startswith(self.browser_dir):
            return "browser"
        base = os.path.basename(exe)
        if base.startswith("node"):
            return "node"
        if base == "bun":
            return "bun"
        if base == "bitti":
            return "bitti"
        return "other"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--log")
    ap.add_argument("--cwd")
    ap.add_argument("--interval-ms", type=float, default=20.0)
    ap.add_argument("--browser-dir", default="/opt/pw-browsers")
    ap.add_argument("--unset", action="append", default=[])
    ap.add_argument("--setenv", action="append", default=[])
    ap.add_argument("--pss-every", type=int, default=0,
                    help="also read PSS (smaps_rollup) every Nth sample; 0: never (the timed runs)")
    ap.add_argument("--meta", action="append", default=[], help="key=value stored in the JSON")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    if not cmd:
        ap.error("no command")

    env = dict(os.environ)
    for n in a.unset:
        env.pop(n, None)
    for kv in a.setenv:
        k, v = kv.split("=", 1)
        env[k] = v

    set_subreaper()
    me = os.getpid()
    classify = Classifier(a.browser_dir)
    interval = a.interval_ms / 1000.0

    peak = {"total": 0, "at": 0.0, "by_kind": {}, "procs": 0}
    peak_kind = {}
    max_procs = 0
    samples = 0
    sample_cost = 0.0
    seen_pids = set()
    runner_procs = {}
    pss_peak = {"total": 0, "by_kind": {}}
    pss_samples = 0
    pss_peak_kind = {}
    stop = threading.Event()
    t0_holder = {}

    def sample_once():
        nonlocal max_procs, samples, sample_cost, pss_samples
        s0 = time.perf_counter()
        pids = descendants(me, snapshot())
        by_kind = {}
        total = 0
        for pid in pids:
            r = rss_bytes(pid)
            if r == 0:
                continue
            k = classify(pid)
            by_kind[k] = by_kind.get(k, 0) + r
            total += r
            if k != "browser":
                # Keyed by (pid, kind): a pid caught before its exec (env ->
                # node) is listed again under its new executable.
                if (pid, k) not in runner_procs:
                    try:
                        exe = os.readlink(f"/proc/{pid}/exe")
                        with open(f"/proc/{pid}/cmdline", "rb") as f:
                            cl = f.read().replace(b"\0", b" ").decode(errors="replace").strip()
                        runner_procs[(pid, k)] = {"pid": pid, "kind": k, "exe": exe, "cmdline": cl[:300]}
                    except OSError:
                        pass
            seen_pids.add(pid)
        if a.pss_every and samples % a.pss_every == 0:
            pk, pt = {}, 0
            for pid in pids:
                v = pss_bytes(pid)
                if v:
                    k = classify(pid)
                    pk[k] = pk.get(k, 0) + v
                    pt += v
            pss_samples += 1
            pk_r = dict(pk)
            pk_r["runner"] = sum(v for k, v in pk.items() if k in RUNNER_KINDS)
            for k, v in pk_r.items():
                if v > pss_peak_kind.get(k, 0):
                    pss_peak_kind[k] = v
            if pt > pss_peak["total"]:
                pss_peak.update(total=pt, by_kind=pk)
        samples += 1
        sample_cost += time.perf_counter() - s0
        n = len(pids)
        max_procs = max(max_procs, n)
        by_kind_r = dict(by_kind)
        by_kind_r["runner"] = sum(v for k, v in by_kind.items() if k in RUNNER_KINDS)
        for k, v in by_kind_r.items():
            if v > peak_kind.get(k, 0):
                peak_kind[k] = v
        if total > peak["total"]:
            peak.update(total=total, at=time.perf_counter() - t0_holder["t0"], by_kind=by_kind, procs=n)

    def sampler():
        nxt = time.perf_counter()
        while not stop.is_set():
            sample_once()
            nxt += interval
            delay = nxt - time.perf_counter()
            if delay > 0:
                stop.wait(delay)
            else:
                nxt = time.perf_counter()

    log = open(a.log, "wb") if a.log else subprocess.DEVNULL
    ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.perf_counter()
    t0_holder["t0"] = t0
    proc = subprocess.Popen(cmd, cwd=a.cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
    th = threading.Thread(target=sampler, daemon=True)
    th.start()
    rc = proc.wait()
    wall = time.perf_counter() - t0
    stop.set()
    th.join()
    # What is left of the tree once the command has exited (orphans that
    # were re-parented to us). Wait up to 5 s for them, then note them.
    leftover_pids = descendants(me, snapshot())
    deadline = time.time() + 5
    while leftover_pids and time.time() < deadline:
        try:
            while os.waitpid(-1, os.WNOHANG)[0] > 0:
                pass
        except ChildProcessError:
            pass
        time.sleep(0.05)
        leftover_pids = descendants(me, snapshot())
    try:
        while os.waitpid(-1, os.WNOHANG)[0] > 0:
            pass
    except ChildProcessError:
        pass
    ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    if a.log:
        log.close()

    meta = dict(kv.split("=", 1) for kv in a.meta)
    mib = 1024 * 1024
    result = {
        **meta,
        "cmd": cmd,
        "cwd": a.cwd or os.getcwd(),
        "exit_code": rc,
        "wall_s": round(wall, 4),
        "cpu_user_s": round(ru1.ru_utime - ru0.ru_utime, 3),
        "cpu_sys_s": round(ru1.ru_stime - ru0.ru_stime, 3),
        "peak_rss_total_mib": round(peak["total"] / mib, 1),
        "peak_rss_at_s": round(peak["at"], 3),
        "peak_rss_by_kind_at_peak_mib": {k: round(v / mib, 1) for k, v in sorted(peak["by_kind"].items())},
        "peak_rss_per_kind_mib": {k: round(v / mib, 1) for k, v in sorted(peak_kind.items())},
        "procs_at_peak": peak["procs"],
        "max_procs": max_procs,
        "distinct_pids": len(seen_pids),
        "samples": samples,
        "interval_ms": a.interval_ms,
        "mean_sample_cost_ms": round(1000 * sample_cost / max(samples, 1), 3),
        "leftover_procs_after_exit": len(leftover_pids),
        "pss_every": a.pss_every,
        "pss_samples": pss_samples,
        "peak_pss_total_mib": round(pss_peak["total"] / mib, 1) if a.pss_every else None,
        "peak_pss_by_kind_at_peak_mib": {k: round(v / mib, 1) for k, v in sorted(pss_peak["by_kind"].items())} if a.pss_every else None,
        # Each part's own peak, whenever it came: "browser" (every process
        # under --browser-dir) and "runner" (the tool's own: node, bun, bitti).
        "peak_pss_per_kind_mib": {k: round(v / mib, 1) for k, v in sorted(pss_peak_kind.items())} if a.pss_every else None,
        "log": a.log,
        "runner_processes": list(runner_procs.values()),
    }
    with open(a.out, "w") as f:
        json.dump(result, f, indent=2)
    print(json.dumps({k: result[k] for k in ("exit_code", "wall_s", "peak_rss_total_mib", "peak_rss_by_kind_at_peak_mib", "samples")}))
    sys.exit(0)


if __name__ == "__main__":
    main()
