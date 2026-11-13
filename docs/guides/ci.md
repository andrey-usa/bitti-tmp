# Run bitti in GitHub Actions or a container

## GitHub Actions

```yaml
- uses: andrey-usa/bitti@next
  with:
    paths: tests
    base-url: http://127.0.0.1:3000
    args: --workers auto --retries 1 --junit report.xml
```

It runs on the runner's Chrome (Linux, macOS, Windows). `version` picks a
release (`latest` by default, or a tag). With no release binary for the
runner, it builds bitti from the action's own commit (`source` asks for
that), with navigera at `navigera-ref`. Failures are annotations on the
lines that failed. The `evidence` output is the directory holding their
screenshots, for `actions/upload-artifact`.

## Container

`Dockerfile` builds an image with bitti and Debian's headless Chromium,
amd64 or arm64, from the directory holding both checkouts (see Building):

```console
$ docker build -f bitti/Dockerfile -t bitti .
$ docker run --rm --network host -v "$PWD/tests:/tests:ro" bitti run /tests --base-url http://127.0.0.1:3000
```

It runs as an unprivileged user (any uid, so OpenShift's random ones too),
with a read-only root filesystem if asked (`HOME=/tmp`, a `/tmp` to write
to, and a writable working directory: `.bitti/` with the last run,
timings and evidence goes there, `/work` by default), and without Chrome's own sandbox: a container without user
namespaces can't give Chrome one, and the container is the boundary.
`examples/kubernetes/job.yaml` is a Job: the tests from a ConfigMap, JUnit
and evidence in `/work/out`, a memory-backed `/dev/shm`.
