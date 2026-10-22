# Derive verification

Run parsing, diagnostics, and expansion tests from the main workspace:

```sh
cargo test -p pgorm-macros
```

The isolated fixture compiles both derives against the actual runtime while
renaming its dependency to the Rust keyword `type`. It checks attribute mapping,
parameter order, empty parameter lists, raw field identifiers, and application
type names that overlap with generated implementation names. It has its own
workspace to avoid a circular development dependency. Four compile-fail doctests
also exercise rejected primary keys, column names, generics, and generated fields:

```sh
cargo test --manifest-path pgorm-macros/tests/fixtures/renamed/Cargo.toml
```
