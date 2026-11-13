# Worked examples

The suites in the repository's `examples/bench-site/`, as they are: CI runs
them against navigera's bench site, a small shop.

```console
$ python3 ../navigera/bench/site/server.py --port 8000 &
$ bitti run examples/bench-site --base-url http://127.0.0.1:8000
```

## shop.bt

A cart, a search and a coupon field inside a web component.

```text
{{#include ../examples/bench-site/shop.bt}}
```

## account.bt

Signing in, and the orders table behind it.

```text
{{#include ../examples/bench-site/account.bt}}
```

## data.bt

Data rows, flows from another file, optional steps, page-state checks.

```text
{{#include ../examples/bench-site/data.bt}}
```

## flows/account.bt

The flows `data.bt` takes with `use`.

```text
{{#include ../examples/bench-site/flows/account.bt}}
```

## A Kubernetes Job

`examples/kubernetes/job.yaml`: the tests from a ConfigMap, JUnit and
evidence in `/work/out`, a memory-backed `/dev/shm`.

```yaml
{{#include ../examples/kubernetes/job.yaml}}
```
