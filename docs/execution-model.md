# Execution Model

This document describes how the runtime executes an OWS workflow, following the
data-flow and flow-directive semantics defined by the specification.

## Status phases

Both workflows and tasks occupy a [`Phase`](phase) from the OWS DSL:
`pending`, `running`, `waiting`, `suspended`, `cancelled`, `faulted`,
`completed`.

## Scope-based execution

A workflow is a sequence of tasks. Composite tasks (`do`, `for`, `fork`, `try`)
introduce nested scopes. The engine executes a scope as an explicit loop rather
than a single giant recursive function, making task transitions explicit and
supporting cancellation, retries, persistence and observability.

For each task in a scope the engine runs the OWS data-flow pipeline:

1. Evaluate the `if` guard. If skipped, the raw task input becomes the output.
2. Validate the raw task input against `input.schema`.
3. Transform task input with `input.from`.
4. Execute the task body (dispatch to the task implementation).
5. Transform task output with `output.as`.
6. Validate the transformed output against `output.schema`.
7. Update the workflow context with `export.as`.
8. Validate the exported context against `export.schema`.

After the pipeline, the task's `then` flow directive (or the directive produced
by the task itself, e.g. a `switch` case) determines the next transition:

- `continue` — run the next task in scope.
- `exit` — complete the current scope.
- `end` — complete the whole workflow.
- `string` — jump to the named task in the current scope.

Flow directives may only target tasks declared within their own scope.

## Structured concurrency

`fork` branches run concurrently using `tokio::task::JoinSet`. Every spawned
task has cancellation propagation, ownership, error handling and tracing
context. `fork.compete` selects the first branch to complete and aborts the
rest; otherwise all branch outputs are collected in declaration order.

## Cancellation

Cancellation is first-class and propagated through workflows, tasks and external
operations via a shared `Cancellation` token. Tasks that block (e.g. `wait`,
`listen`, retry backoff) race against cancellation notification with
`tokio::select!`. A cancelled workflow stops executing subsequent tasks.

## Timeouts

Workflow and task timeouts are implemented with asynchronous cancellation
(`tokio::time::timeout`). A timeout raises a problem with type
`https://open-workflow-specification.org/spec/1.0.0/errors/timeout` and status
`408`, distinct from ordinary failures.

## Retries

Retries are configured on `try.catch.retry`. The engine re-runs the try body
while the caught error matches the retry policy (`when`/`exceptWhen`), up to the
attempt count and duration limits, applying `delay` plus `constant`/`linear`/
`exponential` backoff and optional jitter. Retry timing uses the injectable
clock and RNG so it is deterministic in tests.

## Expressions

Runtime expressions are evaluated by a sandboxed `jq`-subset interpreter. The
`$context`, `$input`, `$output`, `$secrets`, `$task`, `$workflow`, `$runtime`
and `$authorization` arguments are provided per the spec's argument availability
table. Loop variables (`$each`, `$index`) are exposed via the context.

## Extensions, catalogs and secrets

- **Extensions** (`use.extensions`): each extension targets a task type (`extend`)
  and optionally a `when` guard evaluated against the task's scope input. Its
  `before` tasks run before the task body and its `after` tasks after the output
  transform. A `before` task with `then: exit` short-circuits the extended task
  body and returns the extension output. Extensions never extend themselves.
- **Catalogs** (`use.catalogs`): `Runtime::resolve_definition` fetches each
  catalog endpoint through the configured `CatalogResolver` and merges the
  imported functions/errors/retries/timeouts/authentications/extensions/secrets
  (including nested catalogs) into `use` before compilation. Offline resolvers
  (static/file) and an HTTP resolver are bundled.
- **Secrets** (`use.secrets`): declared secret names are resolved through the
  configured `SecretResolver` and exposed as `$secrets`. A reference to a secret
  that is not declared fails compilation.

## Scheduling

A workflow may declare a `schedule` (`every`, `after`, `cron`, or `on` events).
`Runtime::start_schedules` starts one loop per registered schedule: interval and
cron triggers sleep on the injected `Clock` and fire executions, while event
triggers subscribe to the `EventConsumer` and fire on matching events (with the
event envelope as `$workflow.input[0]`). The returned `ScheduleSet` is
cancellable and exposes a triggered counter.

## Checkpoint and resume

After each top-level task the engine persists a checkpoint (the next top-level
task index, the scope input and the current context) through the
`ExecutionStore`. `Runtime::resume` loads a non-terminal record, restores the
context and input, and continues from the checkpoint with the original
execution id.

## Execution identifiers

Each workflow execution and each task execution has a stable identifier. The
workflow execution id is generated through the injected `UuidGenerator` so it is
deterministic in tests.
