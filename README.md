<img src="https://i.ibb.co/yFFyLDcy/Layer-Smarter-Context-Faster-Agents-1.png" width="200" />

[![CI](https://github.com/junaidmirr/layer/actions/workflows/ci.yml/badge.svg)](https://github.com/junaidmirr/layer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![MCP Compliant](https://img.shields.io/badge/MCP-9%20Tools%20%7C%2017%20CLI%20Commands-green.svg)](https://modelcontextprotocol.io)

**High-performance AST code intelligence, relational call graph, and Model Context Protocol (MCP) server for AI coding agents.**

`layer` indexes your entire repository in milliseconds, builds an in-memory symbol and dependency graph (`petgraph`), and exposes **9 surgical MCP tools** and **17 CLI commands** over stdio JSON-RPC. It eliminates the token waste and false-positive noise of raw `grep` and `cat` exploratory loops.

---

## 🚀 Why Layer?

When AI coding agents (Claude Code, Cursor, OpenCode, Codex, Windsurf) explore repositories using basic shell primitives (`grep`, `find`, `cat`), they hit two systemic bottlenecks:

1. **Context Window Exhaustion**: Running `cat` on a 3,000-line file just to inspect a 40-line method consumes ~9,000 tokens in a single turn.
2. **Lexical False Positives**: Plain `grep` cannot distinguish between an actual function call, a markdown changelog mention, a CSS class, or a doc-comment example.

**Layer solves this with zero-dependency compiled Rust intelligence:**
- ⚡ **Sub-80ms In-Memory Graph**: Indexes 850+ files in ~1.3 seconds with sub-millisecond symbol and graph lookups.
- 🎯 **Surgical AST Navigation**: Retrieves exact function bodies (`get_symbol_body`), structural outlines (`get_file_skeleton`), and line slices (`get_lines`) without reading entire files.
- 🔗 **Relational Code Intelligence**: Computes 1-hop blast radii (`get_impact_radius`), groups callers by enclosing function with call counts (`get_callers`), and filters non-code noise (`find_references(code_only=true)`).
- 🔄 **Real-Time Watcher & Diff History**: Debounced file watcher that snapshots edits, renames, and deletions in memory with colorized unified diffs.
- 🔒 **Hardened Security & Concurrency**: Enforces canonical workspace confinement, blocks credential files (`.env`, `.pem`, `id_rsa`), and uses 64-bit FNV-1a content hashing for monotonic version gating.

---

## 🌐 Supported Languages (15+ Languages)

Layer supports multi-language polyglot repositories out of the box across three scanning tiers:

| Tier | Languages & Extensions | Extracted Constructs |
| :--- | :--- | :--- |
| **Tier 1: Dedicated AST Scanners** | **Rust** (`.rs`)<br>**TypeScript / JavaScript** (`.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`)<br>**Python** (`.py`, `.pyw`)<br>**Go** (`.go`) | Functions, async/const/unsafe fns, arrow functions, classes, structs, enums, traits, interfaces, type aliases (`pub type`), receiver methods, docstrings, and imports/re-exports (`use`, `pub use`, `import`, `export`, `from`, `require`). |
| **Tier 2: C-Family & OOP Scanners** | **C / C++** (`.c`, `.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.hxx`)<br>**Java & Kotlin** (`.java`, `.kt`)<br>**C#** (`.cs`), **Swift** (`.swift`), **PHP** (`.php`)<br>**Scala, Dart, Zig** (`.scala`, `.dart`, `.zig`) | Classes, structs, inheritance declarations, constructors/destructors, virtual/override/constexpr methods, and braced function definitions. |
| **Tier 3: Universal / Language-Agnostic** | **All Text & Config Files** (`.rb`, `.lua`, `.sh`, `.sql`, `.toml`, `.yaml`, `.json`, `.md`, etc.) | Supported by `get_lines`, `get_tree`, `find_references`, dense n-gram vector `search`, and real-time diff history. |

---

## 🛠️ Complete Tool Inventory (9 MCP Tools + 17 CLI Commands)

### 1. The 9 MCP Tools (Agent-Facing via `layer mcp`)

| # | MCP Tool Name | Parameters | What It Returns & Why Agents Use It | Typical Cost |
| :--- | :--- | :--- | :--- | :--- |
| **1** | `get_file_skeleton` | `path` | Compact file outline: imports, `pub use` re-exports, symbol names, kinds, line spans, and signatures without function bodies. | ~350 tokens |
| **2** | `find_symbol` | `name` | Exact symbol declarations across the workspace with file paths, line ranges, signatures, and doc comments. | ~150 tokens |
| **3** | `get_symbol_body` | `name`, `file?` | Exact implementation body of a function, method, or struct without loading the surrounding file. Auto-truncates if >150 lines. | ~180 tokens |
| **4** | `get_callers` | `symbol`, `limit?` | Distinct caller functions/methods across the codebase, grouped by enclosing symbol with line spans and `call_count`. Excludes non-code files. | ~280 tokens |
| **5** | `find_references` | `symbol`, `code_only?`, `limit?` | All usages and call-sites across the workspace. Setting `code_only: true` strips markdown docs and JSON schemas. | ~400 tokens |
| **6** | `get_impact_radius` | `symbol`, `file?` | 1-hop blast radius before refactoring: returns callers (who calls it), callees (what it calls internally), and confidence metrics. | ~450 tokens |
| **7** | `get_lines` | `path`, `start`, `end` | Exact slice of lines (1-indexed, inclusive) in compact plain text. | ~100–300 tokens |
| **8** | `get_tree` | *(none)* | Repository directory structure with file counts and symbol counts per file. | ~250 tokens |
| **9** | `search` | `query`, `limit?` | Subword n-gram embedding semantic search with cosine similarity and keyword boosting across code chunks. | ~350 tokens |

### 2. The 17 CLI Subcommands (Developer & Script-Facing)

All CLI commands support global flags: `-p / --path <DIR>`, `--json` (machine-readable JSON output), and `-q / --quiet`.

| Command | Usage | Description |
| :--- | :--- | :--- |
| `mcp` | `layer mcp` | Start the Model Context Protocol (MCP) JSON-RPC server over stdio. |
| `skeleton` | `layer skeleton <file>` | Print the structural outline (imports, signatures, line ranges) of a file. |
| `find` | `layer find <symbol>` | Locate where a symbol is declared across the codebase. |
| `body` | `layer body <symbol> [-f <file>]` | Retrieve the exact implementation body of a function, method, or struct. |
| `callers` | `layer callers <symbol>` | List all caller functions grouped by enclosing symbol with call counts. |
| `references` | `layer references <symbol>` | Find all references and call-sites to a symbol across the repository. |
| `impact` | `layer impact <symbol> [-f <file>]` | Compute the 1-hop impact radius (callers, callees, and candidate definitions). |
| `lines` | `layer lines <file> <start> <end>` | Print an exact range of lines (1-based, inclusive) from a file. |
| `symbols` | `layer symbols <file>` | List all extracted AST symbols and line spans in a specific file. |
| `file` | `layer file <file>` | Print the entire contents of a file with line numbers. |
| `tree` | `layer tree` | Display the project directory hierarchy and symbol counts. |
| `search` | `layer search <query> [-l <limit>]` | Perform semantic vector search across codebase chunks. |
| `query` | `layer query <prompt...>` | Run a one-shot natural language query against the context engine. |
| `history` | `layer history [file]` | View in-memory change history (`CREATED`, `MODIFIED`, `DELETED`, `MOVED`). |
| `diff` | `layer diff <event_id>` | Inspect the unified diff and prior file snapshot for a history event ID. |
| `watch` | `layer watch` | Run as a live debounced file-watcher daemon printing real-time AST updates. |
| `repl` | `layer repl` | Launch the interactive colorized CLI REPL with live file watching and history. |

---

## 📦 Installation

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (Rust 1.85+ / 2024 edition).

### Install via Cargo
```bash
git clone https://github.com/junaidmirr/layer.git
cd layer
cargo install --path .
```
This compiles and installs the optimized release binary to `~/.cargo/bin/layer`.

---

## 🔌 MCP Client Setup

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

### Recommended `AGENTS.md` / `CLAUDE.md` Snippet
While agents adopt `layer` autonomously from its tool schemas, adding this one-liner to your project's `AGENTS.md` or `CLAUDE.md` ensures zero wasted shell calls:
```markdown
Prefer `layer_*` MCP tools (`find_symbol`, `get_symbol_body`, `get_callers`, `get_impact_radius`, `find_references`) for code navigation and impact analysis before falling back to raw grep or full-file reads.
```

---

## 📊 Empirical Benchmarks Across Codebases

Layer has been evaluated in head-to-head comparisons against standard agent tooling (`grep`, `cat`, `ripgrep 15.2.0`) across C++, Rust, and TypeScript codebases:

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

## 🔒 Security & Production Reliability

Layer is built with strict security and concurrency guarantees for autonomous agent execution:

- **Workspace Path Confinement**: Every file path requested via MCP is canonicalized and verified against the workspace root (`resolve_safe_path`), blocking `../` traversal and outward symlink escapes.
- **Credential & Secret Filtering**: Hardcoded security policy blocks access to sensitive files at any depth (`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa`, `credentials.json`, `.npmrc`, `.netrc`), while allowing safe templates like `.env.example`.
- **Monotonic Version Gating**: Combines modification timestamps with deterministic 64-bit FNV-1a content hashing so stale background reads can never overwrite newer indexed state.
- **Stale Edge Reconciliation**: Updating or deleting a file automatically prunes outdated dependency edges in the graph before re-linking.

---

## 🧪 Development & Testing

Before submitting a pull request, run the verification suite locally:

```bash
# 1. Check formatting
cargo fmt --all -- --check

# 2. Run strict Clippy linter (zero warnings allowed)
cargo clippy --all-targets --all-features -- -D warnings

# 3. Run unit and security tests
cargo test --all-targets

# 4. Build optimized release binary
cargo build --release
```

---

## 🤝 Contributing

Contributions are welcome!
1. Fork the repository.
2. Create your feature branch (`git checkout -b feat/my-feature`).
3. Ensure `cargo fmt`, `cargo clippy`, and `cargo test` pass cleanly.
4. Open a Pull Request (GitHub Actions CI will automatically test across Ubuntu and macOS).

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
