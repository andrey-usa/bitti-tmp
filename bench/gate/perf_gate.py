#!/usr/bin/env python3
"""perf_gate: hold bitti's own work on a test suite to a count, not a timer.

    perf_gate.py --head BIN [--base BIN] [--max-ratio 5.5] [--max-growth 0.05]

Writes two generated suites, the large one four times the small in every
part: a steps file shared through `use` (its flows grow too), the `.bt`
files that use it, and `.feature` files whose steps match a fixed set of
step patterns with placeholders. Then counts the instructions of `bitti
lint` on each with valgrind's cachegrind, which repeats to a few hundredths
of a percent on a busy runner where time varies by two or three times.
`lint` reads, parses and resolves everything a run does before its browser
starts.

  scaling  large / small must stay under --max-ratio. Four times the input
           is about four times the work for a linear parse; a cost that
           grows with the product of two parts (each suite parsing the
           shared file again: 16x) breaks it.
  growth   with --base, the head's count at each size must stay within
           --max-growth (5%) of the base binary's on the same files.

Prints a Markdown table (for $GITHUB_STEP_SUMMARY); exits 1 on a bound
broken, 2 when valgrind is missing or lint fails.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile

PATTERNS = 6  # step patterns, the same at both sizes (each costs about 1.5M instructions to compile, once)


def write_suite(root, k):
    """A suite of size k: 40k shared flows, 10k .bt files, 2k .feature files."""
    os.makedirs(root, exist_ok=True)
    flows = []
    for i in range(40 * k):
        flows.append(f'flow open{i}\n  goto /p/{i}\n  expect --testid title{i} text is "Item {i}"\n')
    with open(os.path.join(root, "shared.bt"), "w") as f:
        f.write("set shop = demo\n\n" + "\n".join(flows))
    steps = []
    for p in range(PATTERNS):
        steps.append(f'step "I add {{qty}} of product {{name}} to list {p}"\n'
                     f'  fill --label Quantity ${{qty}}\n  click --role button --name "Add ${{name}}"\n')
    with open(os.path.join(root, "steps.bt"), "w") as f:
        f.write("\n".join(steps))
    for s in range(10 * k):
        body = ["use shared.bt\n"]
        for c in range(3):
            n = (s * 3 + c) % (40 * k)
            body.append(f'scenario "suite {s} case {c}"\n  run open{n}\n  click --role link --name "Next"\n'
                        f'  expect url ends-with /p/{n + 1}\n')
        with open(os.path.join(root, f"s{s}.bt"), "w") as f:
            f.write("\n".join(body))
    for ft in range(2 * k):
        lines = [f"Feature: Lists {ft}\n"]
        for c in range(10):
            p = (ft * 10 + c) % PATTERNS
            lines.append(f'  Scenario: list {ft} case {c}\n    When I add "{c + 1}" of product "w{c}" to list {p}\n'
                         f'    And I add "2" of product "x{c}" to list {(p + 1) % PATTERNS}\n')
        with open(os.path.join(root, f"f{ft}.feature"), "w") as f:
            f.write("\n".join(lines))


def count(binary, root):
    """Instructions of `bitti lint root` under cachegrind."""
    p = subprocess.run(
        ["valgrind", "--tool=cachegrind", "--cache-sim=no", "--cachegrind-out-file=/dev/null",
         binary, "lint", root],
        capture_output=True, text=True, env={**os.environ, "NO_COLOR": "1"},
    )
    if p.returncode != 0:
        print(f"lint failed ({p.returncode}) on {root}:\n{p.stdout[-800:]}{p.stderr[-800:]}")
        sys.exit(2)
    m = re.search(r"I\s+refs:\s+([\d,]+)", p.stderr)
    if not m:
        print(f"no instruction count in valgrind's output:\n{p.stderr[-800:]}")
        sys.exit(2)
    return int(m.group(1).replace(",", ""))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--head", required=True, help="the bitti binary under test")
    ap.add_argument("--base", help="the binary it is compared with (the release before it)")
    ap.add_argument("--max-ratio", type=float, default=5.5)
    ap.add_argument("--max-growth", type=float, default=0.05)
    a = ap.parse_args()
    if not shutil.which("valgrind"):
        print("valgrind is not installed")
        return 2

    with tempfile.TemporaryDirectory() as tmp:
        small, large = os.path.join(tmp, "small"), os.path.join(tmp, "large")
        write_suite(small, 1)
        write_suite(large, 4)
        h = (count(a.head, small), count(a.head, large))
        b = (count(a.base, small), count(a.base, large)) if a.base else None

    bad = []
    ratio = h[1] / h[0]
    if ratio > a.max_ratio:
        bad.append(f"four times the suite cost {ratio:.2f}x the instructions (bound {a.max_ratio}x)")
    cols = ["", "small", "large", "large / small"]
    rows = [["head", f"{h[0]:,}", f"{h[1]:,}", f"{ratio:.2f}x"]]
    if b:
        rows.append(["base", f"{b[0]:,}", f"{b[1]:,}", f"{b[1] / b[0]:.2f}x"])
        growth = [h[0] / b[0] - 1, h[1] / b[1] - 1]
        rows.append(["change", f"{growth[0]:+.2%}", f"{growth[1]:+.2%}", ""])
        for size, g in zip(("small", "large"), growth):
            if g > a.max_growth:
                bad.append(f"{size}: {g:+.2%} instructions against the base (bound +{a.max_growth:.0%})")

    print("Instructions of `bitti lint` on generated suites (cachegrind; large = 4x small):\n")
    print("| " + " | ".join(cols) + " |")
    print("|" + "---|" * len(cols))
    for r in rows:
        print("| " + " | ".join(r) + " |")
    print()
    for x in bad:
        print(f"FAIL: {x}")
    print("perf gate: " + ("FAILED" if bad else "ok"))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
