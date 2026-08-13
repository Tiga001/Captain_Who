# Multi-Agent Release Gate

This document freezes the finite fault-injection and pressure boundary for the Agent collaboration
feature. It is a release test contract, not a claim about production capacity and not a second
runtime design.

Run the deterministic gate from the repository root:

```bash
pnpm test:multi-agent-release
```

It never contacts a paid model or executes a real MCP, Skill, Command, or filesystem side effect.
The tests use local fake providers, fake clocks, SQLite fixtures under temporary directories, and
strict protocol fixtures. `--profile-only` and `--smoke-only` are available for diagnosis; a release
requires the default combined invocation plus the repository-wide `pnpm check`.

## Reproducible profile and thresholds

The gate prints one `ROUND6_ENV` record, per-step wall times, OS resource statistics for the heavy
steps, domain `ROUND6_METRIC` records, and a final result. Defaults are hard minimums: environment
overrides may increase them but cannot silently reduce them.

| Dimension                   |                                                                                      Release profile |
| --------------------------- | ---------------------------------------------------------------------------------------------------: |
| Tree boundary               |                                                       64 nodes accepted; node 65 rejected atomically |
| Runtime fan-out             |                                                          32 Agents, process-wide concurrency limit 4 |
| Durable collaboration facts | 10,000 ordinary Mailbox messages, a Result per spawned child, and more than 10,000 root-local events |
| Logical turns               |                                       500 complete turns followed by a durable compaction and reload |
| Crash/restart cycles        |                                                              20 expired claimed-Wake recovery cycles |
| Subscription lifecycle      |                                           1,000 subscribe/unsubscribe cycles in renderer and Preload |

The canonical Mailbox deliberately caps one recipient at 1,024 unbound facts (ordinary messages at
960, with space reserved for Tasks/Results). The pressure fixture therefore distributes 10,000
messages across the configured tree. A root can retain only the bounded Result reserve, so the test
uses every spawned child to exercise the real result outbox while the 10,000-message and event
thresholds exercise bulk indexing. Reducing these facts would test a configuration the product
cannot actually admit and is forbidden by the script.

Correctness thresholds are exact: no lost fact, duplicate logical result/card, concurrent Turn for
one Agent, cross-Conversation stream, escaped SQLite busy error, permanent lease, or ghost active
Wake. All work must converge through deterministic barriers. Wall time, peak resident memory,
user/system CPU, maximum enqueue latency, aggregate verification-query latency, total paginated
event catch-up latency, maximum event-page latency, database size, and maximum reopen latency are
recorded as regression observations; the repository has no formal production capacity budget, so
these numbers must not be advertised as capacity guarantees.

## Finite fault-point inventory

| ID  | Boundary                                                                                        | Required evidence                                                                                             |
| --- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| F1  | `ensure_root`; template/model snapshot; child Conversation/Agent/task/projection/Wake           | injected failure leaves no partial child fact; same request retries to one identity                           |
| F2  | Mailbox enqueue; lease claim; FIFO projection; acknowledgement; deferred Wake                   | transaction rollback, half-open lease fencing, ordered recovery, one projection                               |
| F3  | Dispatcher reserve/claim; Turn admission; Runtime start; lease renew; shutdown                  | one Turn per Agent, global limit, FIFO progress, no ghost capacity                                            |
| F4  | Provider/Tool/MCP/Skill/Command before, during, or after an external effect                     | safe retry only before effect; durable result or explicit `outcome_unknown` afterwards                        |
| F5  | terminal trace/Usage followed by result outbox                                                  | injected outbox failure rolls the whole settlement back; recovery observes rather than replays Runtime        |
| F6  | result outbox followed by parent deferred Wake and post-commit notification                     | injected Wake failure rolls back result; notification loss is repaired from SQLite without duplicate effect   |
| F7  | `wait_agent` check/register/recheck, receipt freeze, ToolResult/model-context precommit, cursor | result/timeout/steer/interrupt races settle once and another wait sees remaining facts                        |
| F8  | Command Session wait registration/settlement versus Agent wait                                  | neither domain wakes or consumes the other; each remains queryable                                            |
| F9  | child Approval create, root projection, decision CAS, original checkpoint resume                | restart and duplicate/expired decisions are idempotent; Runtime resumes only after durable Waiting-to-Running |
| F10 | collaboration event commit, notification, renderer catch-up and observer live overlay           | duplicate/out-of-order/gap/resync/reload converge to DB; exact identities prevent stream crossing             |

## Endpoint inventory

The feature exposes exactly six model Harness tools: `spawn_agent`, `send_message`,
`followup_task`, `wait_agent`, `list_agents`, and `interrupt_agent`. There is no `wait_any` or
seventh collaboration tool.

The strict collaboration RPC surface is limited to tree/detail/location/observer/event reads,
template CRUD, and root Approval list/decision methods under `agent.collaboration.*`. Renderer
notifications are `collaboration.event`, `collaboration.observerEvent`, and
`collaboration.resync`, with matching Main/Preload allowlists. Security regression also covers the
legacy Conversation, history/search, message/UI/draft, start/steer/cancel/provider transition,
fork, Command Session, context, attachment/Artifact, project deletion, model-settings, and Approval
entry points touched while making child Conversations read-only.

## Recovery and schema policy

Canonical storage is version 8 and is accepted only when its normalized SQLite catalog fingerprint
matches the frozen value. This is a development repository: version 7 or older, an unversioned
non-empty database, a partial catalog, or a tampered current schema returns the stable
`development_storage_schema_reset_required` error and never rewrites the source. Tests construct
all old or failing databases under `tempfile`/`mktemp`; live user databases are never opened,
copied, renamed, migrated, or deleted by the gate. The explicit reset command stages and verifies a
new canonical database before publication and retains a recoverable backup if publication fails.

## Release interpretation

The multi-Agent feature is releasable only when this gate, the full repository `pnpm check`, the
packaged startup smoke, schema/reset tests, and independent P0/P1 audit all pass on the same final
tree. A failed step means **not releasable**. Isolated reruns may diagnose a flaky test but cannot
replace a clean complete rerun; sleeps, weakened authorization, removed assertions, or in-memory
substitutes are not acceptable fixes.
