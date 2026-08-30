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

## Execution identifiers

Each workflow execution and each task execution has a stable identifier. The
workflow execution id is generated through the injected `UuidGenerator` so it is
deterministic in tests.
