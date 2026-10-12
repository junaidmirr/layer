# `layer` ⚡

[![CI](https://github.com/your-username/layer/actions/workflows/ci.yml/badge.svg)](https://github.com/your-username/layer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![MCP Compliant](https://img.shields.io/badge/MCP-Model%20Context%20Protocol-green.svg)](https://modelcontextprotocol.io)

**High-performance AST code intelligence and Model Context Protocol (MCP) server for AI coding agents.**

`layer` indexes your entire repository in milliseconds, constructs an in-memory symbol and dependency graph, and provides surgical code navigation tools over standard input/output (stdio JSON-RPC). It eliminates the token waste and false-positive noise of raw `grep`/`cat` exploratory loops.

---

## 🚀 Why Layer?

When AI coding agents (Claude Code, Cursor, OpenCode, Codex, Aider) explore codebases using filesystem primitives (`grep`, `find`, `cat`), they suffer from two critical failure modes:

1. **Context Window Exhaustion**: Running `cat` on a 3,000-line module just to inspect a 40-line function consumes ~9,000 tokens per turn.
2. **Lexical False Positives**: Plain `grep` cannot distinguish between an actual struct constructor, a markdown changelog mention, a CSS class, or a doc-comment snippet.

**Layer bridges this gap with zero-overhead compiler intelligence:**
- ⚡ **Sub-80ms In-Memory Graph**: Indexes 1,000+ files in ~1.4 seconds with sub-millisecond query lookups.
- 🎯 **Surgical AST Navigation**: Retrieves exact function bodies (`get_symbol_body`), high-level outlines (`get_file_skeleton`), and line slices (`get_lines`) without loading megabytes of dead code.
- 🔗 **Relational Code Intelligence**: Trace exact 1-hop impact radii (`get_impact_radius`), caller functions grouped by symbol with call counts (`get_callers`), and AST references filtered from doc noise (`find_references(code_only=true)`).
- 🔄 **Real-Time Watcher & Version History**: In-memory debounced file watcher that preserves instant snapshots and colorized unified diffs across file edits, renames, and deletions.

---

## 📦 Installation

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (Rust 1.85+ / 2024 edition).

### Install via Cargo
```bash
cargo install --path .
```
This builds and installs the optimized release binary directly to `~/.cargo/bin/layer`.

To verify installation:
```bash
layer --version
```

---

## 🔌 MCP Client Configuration

`layer` runs as a local stdio Model Context Protocol (MCP) server. Configure it once in your favorite agentic IDE or CLI:

### 1. OpenCode (`~/.config/opencode/opencode.jsonc`)
```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "layer": {
      "type": "local",
      "command": ["layer", "mcp"]
    }
  },
  "tools": {
    "layer_*": true
  },
  "permission": {
    "layer_*": "allow"
  }
}
```

### 2. Claude Desktop (`claude_desktop_config.json`)
- **macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Windows**: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "layer": {
      "command": "layer",
      "args": ["mcp"]
    }
  }
}
```

### 3. Cursor (`.cursor/mcp.json`)
```json
{
  "mcpServers": {
    "layer": {
      "command": "layer",
      "args": ["mcp"]
    }
  }
}
```

---

## 🛠️ MCP Tool Reference

`layer` exposes 9 surgical tools directly into your AI agent's schema:

| Tool | Purpose | Typical Token Cost |
| :--- | :--- | :--- |
| `layer_find_symbol` | Locates symbol declaration, kind (`fn`, `struct`, `class`, `type`), line span, and signature. | ~150 tokens |
| `layer_get_symbol_body` | Retrieves the exact implementation body of a function/method without reading the rest of the file. | ~180 tokens |
| `layer_get_callers` | Returns distinct caller functions across crates, grouped by enclosing symbol with line spans and call counts. Excludes non-code docs. | ~280 tokens |
| `layer_find_references` | Finds all usages and call-sites. Supports `code_only: true` to strip schema JSON and changelogs. | ~400 tokens |
| `layer_get_impact_radius` | Returns 1-hop impact radius: callers, callees, definitions, and confidence rating. | ~450 tokens |
| `layer_get_file_skeleton` | Compact structural outline (imports, `pub use` re-exports, symbol signatures, line ranges) without bodies. | ~350 tokens |
| `layer_get_lines` | Retrieves an exact slice of lines (1-indexed, inclusive) in compact plain text. | ~100–300 tokens |
| `layer_get_tree` | Fast directory hierarchy summary with file counts and symbol totals. | ~250 tokens |
| `layer_search` | Subword embedding semantic search with cosine similarity scoring across code chunks. | ~350 tokens |

---

## 📊 Empirical Benchmarks Across Codebases

Layer has been tested in head-to-head comparisons against standard agent tooling (`grep`, `cat`, `ripgrep 15.2.0`) across real-world C++, Rust, and TypeScript codebases:

| Codebase & Stack | Scope & Task | Baseline | Layer | Net Impact |
| :--- | :--- | :--- | :--- | :--- |
| **Creo** (C++17 / Qt) | Symbol retrieval & call-site discovery | 18,400 tokens / 14 calls | **3,100 tokens / 6 calls** | **-83.2% tokens**, 100% CTest pass |
| **Creo UI** (React 18 / TS) | Refactor monolithic 2,000-line `App.tsx` | ~35,000 tokens (3 reads) | **3,148 tokens / 9 calls** | **-91.0% tokens**, 0 full-file reads |
| **Claude Code** (Sonnet 3.5) | 6-task cumulative agent development | $1.28 / 42 requests / 5m 48s | **$0.47 / 19 requests / 2m 13s** | **-63% cost**, **-55% turns**, **-62% time** |
| **Tauri** (Rust, 856 files) | 10-task suite vs `ripgrep 15.2` | 9,333 tokens / 15 calls | **4,091 tokens / 10 calls** | **+56.2% net tokens**, 100% precision |
| **Tauri** (IPC Security Audit) | Autonomous unprompted agent audit | ~28,000 tokens (cat/grep) | **~3,500 tokens (11 calls)** | **~87% token reduction**, 100% accuracy |

### The Hybrid Principle
* **Literal Regex & Path Globbing**: Native `grep` / `glob` remains optimal for unindexed literal string sweeps.
* **Relational Code Intelligence**: `layer_*` saves **70%–90% of tokens** whenever finding callers, inspecting function bodies, checking impact radius, or mapping dependencies.

👉 *See [`BENCHMARKS.md`](BENCHMARKS.md) for the complete task logs, recall tables, and methodology.*

---

## 💻 CLI & Interactive REPL

`layer` can also be used directly as a standalone developer CLI tool:

### 1. Interactive REPL
```bash
layer repl
```
```text
layer> give me file structure
layer> history
layer> diff #1
layer> all functions in src/scanner.rs
layer> lines 10 to 30 in src/models.rs
layer> search "token trim"
```

### 2. CLI Commands
```bash
# Query symbol callers and blast radius
layer callers WindowEffect
layer impact InvokeRequest

# View file change history and unified diffs
layer history
layer history src/graph.rs
layer diff 1

# Structural outlines & inspection
layer tree
layer symbols src/scanner.rs
layer lines src/graph.rs 10 50
layer file src/main.rs
```

---

## 🧪 Development & Testing

Ensure all quality gates pass before submitting contributions:

```bash
# 1. Format verification
cargo fmt --all -- --check

# 2. Strict Clippy lint check (zero warnings)
cargo clippy --all-targets --all-features -- -D warnings

# 3. Unit and integration test suite
cargo test --all-targets

# 4. Release build
cargo build --release
```

---

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request. For major changes, please open an issue first to discuss what you would like to change.

1. Fork the repository.
2. Create your feature branch (`git checkout -b feat/amazing-feature`).
3. Commit your changes (`git commit -m 'feat: add amazing feature'`).
4. Ensure `cargo fmt`, `cargo clippy`, and `cargo test` pass.
5. Push to the branch (`git push origin feat/amazing-feature`).
6. Open a Pull Request.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
