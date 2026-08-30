# Conformance

Conformance testing executes the official OWS Conformance Test Kit against the
runtime. See [`docs/conformance.md`](../../docs/conformance.md).

Run the CTK with:

```sh
cargo run -p ows-runtime-cli -- conformance --ctk <path-to-ows-spec/ctk/features>
```
