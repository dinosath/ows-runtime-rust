# Contributing

Thanks for contributing to the OWS runtime!

## Development

Prerequisites: a recent stable Rust toolchain (see `rust-toolchain.toml`).

```sh
cargo build --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --all-features
```

## Guidelines

- Keep the core engine independent of HTTP frameworks, databases, brokers and
  cloud providers; add capabilities as adapters behind the core traits.
- Preserve the OWS specification semantics. If a behavior is in doubt, the
  specification wins.
- Add regression tests for every bug you fix.
- Ensure `cargo test --workspace --no-default-features` and
  `cargo test --workspace --all-features` both pass.
- Run `cargo fmt --all` and keep `clippy -D warnings` clean.

## Conformance

If you change execution semantics, run the CTK:

```sh
cargo run -p ows-runtime-cli -- conformance --ctk <path-to-ows-spec/ctk/features>
```

## Commits

Use clear, conventional commit messages describing the change and its rationale.
