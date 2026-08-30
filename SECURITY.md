# Security Policy

## Reporting a Vulnerability

Please report security vulnerabilities privately by opening a GitHub Security
Advisory or contacting the maintainers directly. Do not open a public issue for
security vulnerabilities.

## Security Model

The runtime is **deny-by-default** for security-sensitive capabilities. See
[`docs/security.md`](docs/security.md) for the full model, including runtime
policy, sandboxed expressions, network policy, and resource limits.

Key principles:

- Runtime expressions are evaluated by a sandboxed interpreter; there is no
  `eval`, shell, or arbitrary code execution.
- Outbound network is denied unless explicitly enabled and scoped.
- Script/container execution is denied unless explicitly enabled.
- Malformed workflow input never causes a panic.
