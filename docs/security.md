# Security

Workflow definitions can be supplied by untrusted users, so the runtime is
**deny-by-default** for security-sensitive capabilities. This document describes
the security model.

## Runtime policy

The [`RuntimePolicy`](crate::policy::RuntimePolicy) bounds resource usage and
gates security-sensitive capabilities:

| Capability | Default | Description |
|-----------|:---:|---|
| `max_execution_time` | `None` | Max wall-clock time for a workflow. |
| `max_task_time` | `None` | Max wall-clock time for a single task. |
| `max_parallel_tasks` | `1024` | Max concurrently running tasks. |
| `max_loop_iterations` | `1_000_000` | Max iterations in a single `for`. |
| `max_payload_size` | `None` | Max workflow payload size. |
| `allow_scripts` | `false` | Allow `run.script`/`run.shell`. |
| `allow_containers` | `false` | Allow `run.container`. |
| `allow_network` | `false` | Allow any outbound network. |
| `allowed_hosts` | `[]` | Allowed hosts (empty = none when `allow_network`). |
| `allowed_schemes` | `[]` | Allowed URL schemes (empty = none). |

## Sandboxed expressions

Runtime expressions are evaluated by a sandboxed `jq`-subset interpreter. There
is **no** `eval`, shell execution, or arbitrary code execution. Only workflow
definitions' expressions are evaluated; input and task data are never treated as
executable expressions (preventing injection attacks).

## Script and container execution

`run` processes are behind the [`ProcessRunner`](crate::service::ProcessRunner)
trait and are denied by default. A runtime operator must supply an adapter and
set `allow_scripts`/`allow_containers` to enable them.

## Network policy

The built-in HTTP/OpenAPI function invokers check `allow_network`, `allowed_hosts`
and `allowed_schemes` before making any request. By default `allow_network` is
`false`, so outbound calls are denied with a policy error.

## Secrets

Secrets are resolved from the workflow's declared `use.secrets` list and exposed
to expressions only as `$secrets`. Access to undeclared secrets raises a 403
`authorization` error.

## Resource limits

The engine enforces `max_loop_iterations` (loop protection) and the task/execution
timeouts to prevent infinite loops and unbounded CPU.

## Error handling

Errors never panic on malformed input. Malformed workflow definitions produce
typed parse/schema/semantic errors, not panics.
