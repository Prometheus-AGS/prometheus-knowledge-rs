# Deployment evidence

Recorded 2026-09-20 on the macOS release host.

## Installed artifact

`cargo build --locked --release -p pk-learning-worker` exited 0. The source and
installed binaries matched:

```text
6a2c259c09ebde59f087e1e2140e195b38d9637e94dd2683fef4f3c06576125b  target/release/prometheus-learning-worker
6a2c259c09ebde59f087e1e2140e195b38d9637e94dd2683fef4f3c06576125b  /Users/gqadonis/.local/bin/prometheus-learning-worker
```

The installed command reported `prometheus-learning-worker 1.8.0`.

## Backlog progress

`launchctl kickstart -k gui/501/ai.prometheus.learning-worker` started one
worker. At 2026-09-20T18:19:40Z, `launchctl print` reported PID 21327 running.
The durable queue changed from:

```text
memory accepted: 344
memory completed: 1932
```

to this observed state at 2026-09-20T18:21:16Z:

```text
memory accepted: 335
memory completed: 1941
```

The nine-operation movement proves that valid receipts exceeding the former
ten-second ceiling now advance through the installed production binary.

## Remaining limitation

The historical backlog was still draining when this evidence was recorded.
This change proves forward progress; it does not claim a doctor-clean queue.
