# difftastic-server — agent instructions

## What this is

A fork of [difftastic](https://github.com/Wilfred/difftastic) — a structural diff tool using tree-sitter ASTs. Binary name: `difft`. The fork adds server mode (HTTP/gRPC) on top.

## Build & test

MSRV: 1.85.0 (enforced by `rust-toolchain.toml`).

```sh
cargo build
cargo test          # unit + integration tests (tests/cli.rs)
cargo fmt           # respects .rustfmt.toml (imports_granularity = "module")
```

**Never** `cargo fix` or `cargo clippy` — the codebase is heavily lint-suppressed (`#![allow(...)]` in main.rs, build.rs) and clippy is not in the toolchain (only rustfmt is). Do not add clippy to the toolchain.

## Config via environment

All CLI flags also accept env vars. The full list is in `src/options.rs`.

| Env                                                | Default      | Note                                   |
| -------------------------------------------------- | ------------ | -------------------------------------- |
| `DFT_BYTE_LIMIT`                                   | 1000000      | Falls back to text diff if exceeded    |
| `DFT_GRAPH_LIMIT`                                  | 3000000      | Falls back if Dijkstra graph too large |
| `DFT_PARSE_ERROR_LIMIT`                            | 0            | Falls back if parse errors exceed      |
| `DFT_DISPLAY`                                      | side-by-side | Also: inline, json                     |
| `DFT_UNSTABLE`                                     | —            | Must be set to `yes` for JSON output   |
| `DFT_OVERRIDE` / `DFT_OVERRIDE_1..9`               | —            | Lang overrides: `GLOB:NAME`            |
| `DFT_OVERRIDE_BINARY` / `DFT_OVERRIDE_BINARY_1..9` | —            | Binary glob overrides                  |
| `DFT_STRIP_CR`                                     | on           | Auto-strip CRLF on Windows             |
| `DFT_DBG_KEEP_UNCHANGED`                           | —            | Debug: skip unchanged-node pruning     |
| `DFT_LOG`                                          | —            | Log level for `pretty_env_logger`      |

## Architecture

```
src/main.rs → options::parse_args() → Mode::Diff → diff_file_content()
                                                              ├─ guess_language (file ext + shebang)
                                                              ├─ tree_sitter_parser::parse() (AST)
                                                              ├─ unchanged::mark_unchanged() (prune)
                                                              ├─ dijkstra::mark_syntax() (graph diff)
                                                              ├─ sliders::fix_all_sliders() (refine)
                                                              └─ display::* (inline/side-by-side/json)
```

- **Parsers**: 70+ vendored tree-sitter parsers compiled in `build.rs` via `cc` crate. Source lives in `vendored_parsers/*-src/`. Changing parser sources triggers rebuild.
- **Allocator**: `tikv-jemallocator` on non-Windows, non-illumos, non-FreeBSD.
- **IDs**: `hashbrown::HashMap` with FxHasher aliased as `DftHashMap` in `src/hash.rs`.
- **Arena**: `bumpalo` for AST nodes; `typed_arena` for syntax trees.

## Package identity

- Cargo package name: `difftastic-server` (Cargo.toml says "difftastic-server")
- Binary: `difft`
- Features section in Cargo.toml is **empty** — server mode (HTTP/gRPC described in `docs/grpc-http-service-design.md`) is not yet implemented.

## Tests

- Integration: `tests/cli.rs` via `assert_cmd`. Uses `sample_files/` for fixtures.
- Unit: inline `#[cfg(test)] mod tests` in `src/main.rs` and `src/options.rs`.
- Regression: `just compare` runs `sample_files/compare_all.sh` which diffs every `*_1.*` / `*_2.*` pair and checksums the output against `sample_files/compare.expected`.

## Key conventions

- `rustfmt` only, no clippy in CI/toolchain.
- `.typos.toml` excludes `vendored_parsers/`, `sample_files/`, `demo_files/`.
- `.gitignore` excludes `/target`, `*.rs.bk`, `perf.data*`, `flamegraph.svg`, `.idea`.
- Justfile has shortcuts for doc (`just doc`), compare (`just compare`), release (`just release`), man page generation (`just man`), perf (`just perf`).

## git integration

Difftastic supports `GIT_EXTERNAL_DIFF` protocol (7 or 9 arguments). When a single arg is given and git env vars are present, it enters "unmerged file" mode.

**Language**
Always use English in code files(include config files, comments) and use Simplified Chinese in docs.
