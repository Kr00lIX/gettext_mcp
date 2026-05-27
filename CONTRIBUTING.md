# Contributing to gettext-mcp

Thanks for your interest in contributing.

## Development Setup

### Prerequisites

- Rust 1.75+ with Cargo (MSRV is pinned in `Cargo.toml`)

### Build

```sh
cargo build
```

### Run with the web UI

The web UI is useful for exercising the store interactively while developing:

```sh
WEB_PORT=8787 cargo run -- examples/sample_fr.po
# open http://127.0.0.1:8787
```

For MCP-only iteration:

```sh
cargo run -- examples/sample_fr.po
```

## Tests

```sh
cargo test                        # full suite
cargo test store::tests           # parser + store
cargo test mcp_server::tests      # MCP tool surface
cargo test --test web_integration # web/store integration
```

### Test Layout

- Unit tests live next to the code they cover, under `#[cfg(test)] mod tests` in `src/store.rs` and `src/mcp_server.rs`.
- Integration tests live in `tests/` (currently `tests/web_integration.rs`).
- Round-trip parser tests build a `GettextFile`, serialize, re-parse, and assert equality — keep this guarantee intact for any parser/serializer change.

## Code Style

```sh
cargo fmt
cargo clippy -- -D warnings
```

Both run clean as a precondition for merge. Anything fancier (custom rustfmt settings, lint allowlists) lives in `rustfmt.toml` and crate-level `#![allow]` attributes — prefer fixing the warning over silencing it.

A few conventions worth knowing:

- **All logging to stderr.** stdout is the MCP stdio transport; a stray `println!` will corrupt the JSON-RPC stream. Use `tracing` macros.
- **No `unwrap()` in non-test code.** Propagate errors with `?` and the `thiserror`-derived types in `store.rs` / `mcp_server.rs`.
- **Metadata-preserving edits.** When modifying existing entries, use `update_entry()` rather than `upsert()` — the latter only round-trips msgstr and flags. See the relevant note in [`CLAUDE.md`](CLAUDE.md).

## Commit Conventions

- Short imperative subject (≤ 72 chars): "fix plural serialization for empty entries", not "fixed plural bug".
- Body explains *why*, not *what* — the diff already shows the what.
- One logical change per commit. Refactors and behavior changes go in separate commits when feasible.
- **Do not include `Co-Authored-By` lines.**

## PR Checklist

- [ ] `cargo fmt` is clean
- [ ] `cargo clippy -- -D warnings` is clean
- [ ] `cargo test` passes
- [ ] New behavior is covered by a test (parser, store, or MCP tool)
- [ ] Public API or tool surface changes are reflected in `README.md` / `CLAUDE.md`

For larger changes, opening an issue first to discuss the approach is appreciated but not required.
