# Changelog

## [Unreleased]

### Added

- OWS runtime workspace with library-first architecture.
- `ows-runtime-core`: typed errors (ProblemDetails, WorkflowError), injectable
  clocks/RNG/UUIDs, `RuntimePolicy`, `ExecutionStore`, service/event/scheduler
  traits.
- `ows-runtime-expressions`: sandboxed jq-subset interpreter with strict/loose
  expression handling, interpolation, and property-based tests.
- `ows-runtime-dsl`: parsing (YAML/JSON), validation, and default normalization
  on top of the official `serverless_workflow_core` SDK.
- `ows-runtime`: scope-based execution engine, all twelve OWS task types,
  data-flow pipeline, flow directives, retries, timeouts, cancellation, and
  HTTP/OpenAPI function invokers.
- `ows-runtime-events`: CloudEvents, event matching, and an in-memory broker.
- `ows-runtime-scheduler`: scheduling abstractions and cron parsing.
- `ows-runtime-observability`: tracing and OWS lifecycle event emission.
- `ows-runtime-testing`: deterministic test helpers and fakes.
- `ows-runtime-cli`: `validate`, `compile`, `run`, `conformance` subcommands with
  a Gherkin CTK runner.

### Conformance

- 12/12 deterministic OWS CTK scenarios pass.
- Network-dependent scenarios are skipped by default and can be enabled with
  `--include-network`.
