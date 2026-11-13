# Contributing to bitti

## Building

bitti links navigera as a library, checked out beside it. CI builds
bitti's `next` against navigera's `next`, and `main` against `master`. A
release builds against the navigera tag it names:

```text
work/
  navigera/     git clone https://github.com/andrey-usa/navigera (master)
  bitti/    this repository
```

```console
$ cd bitti && cargo build --release
$ ./target/release/bitti run examples --base-url http://127.0.0.1:<port>
```

`examples/bench-site` runs against navigera's bench site
(`python3 ../navigera/bench/site/server.py --port 0`).

## Tests

```console
$ cargo test --locked
```

The tests start bitti's own fixture sites and navigera's bench site (Python
3, from `../navigera`), and drive the Chrome they find: `CHROME_BIN`, or a
system install. `expect download … compare` needs
[compara](https://github.com/andrey-usa/compara) on the `PATH` (`pip install
compara`). CI runs them on Linux, macOS and Windows, then the 40-scenario
suite of `bench/suite40` three times over, shuffled.

## Performance gate

`bench/gate/perf_gate.py` counts the instructions `bitti lint` takes on two
generated suites, the large one four times the small, with valgrind's
cachegrind: a count, where a timer would vary by two or three times on a
shared runner. The large suite may cost at most 5.5 times the small, and
each at most 5% more than the last release's binary. `gate.yml` runs it on
every push to `next` and on pull requests.

## Docs

The docs site (https://andrey-usa.github.io/bitti/) is written by
`docs-site/build.py` from `docs-site/site.toml`, which says which page comes
from which source: README sections, `docs/`, the skill, CHANGELOG.md and
the binary's `--help`. Nothing under `docs-site/src` is edited by hand. The
build stops on a link to a missing file or heading and on a source section
that is on no page; `--write` also rewrites the README's Documentation
section and `llms.txt`, which CI's docs job checks:

```console
$ cargo build --release
$ python3 docs-site/build.py --write
$ python3 docs-site/build.py --mdbook mdbook
```
