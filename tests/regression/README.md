# Regression

Every discovered bug must get a permanent regression test. Regression tests live
in the crate that owns the bug (usually `crates/ows-runtime/tests/errors.rs` or
`crates/ows-runtime-expressions/tests/`). This directory may hold scenarios that
span multiple crates.
