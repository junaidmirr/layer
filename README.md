<img src="https://i.ibb.co/yFFyLDcy/Layer-Smarter-Context-Faster-Agents-1.png" width="200" />

[![CI](https://github.com/junaidmirr/layer/actions/workflows/ci.yml/badge.svg)](https://github.com/junaidmirr/layer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Tree-sitter](https://img.shields.io/badge/Tree--sitter-6%20Compiled%20Grammars-blueviolet.svg)](https://tree-sitter.github.io/tree-sitter/)
[![MCP Compliant](https://img.shields.io/badge/MCP-9%20Tools%20%7C%2017%20CLI%20Commands-green.svg)](https://modelcontextprotocol.io)

**Tree-sitter AST code navigation, static call-graph analysis, and Model Context Protocol (MCP) server for AI coding agents.**

`layer` parses your repository using compiled **Tree-sitter** grammars, constructs an in-memory symbol and dependency graph (`petgraph`), and exposes **9 surgical MCP tools** and **17 CLI subcommands** over stdio JSON-RPC. It gives AI coding agents exact function bodies, structural file skeletons, AST-filtered caller graphs, and 1-hop impact radii without dumping entire files into the context window.

---

## Why Layer?

When AI coding agents (Claude Code, Cursor, OpenCode, Codex, Windsurf) explore repositories using raw shell primitives (`grep`, `find`, `cat`), they hit two predictable failure modes:

1. **Context Window Bloat**: Running `cat` on a 2,000-line file just to inspect one 35-line method burns ~6,000–9,000 tokens in a single turn.
2. **Lexical Noise**: Line-based `grep` cannot distinguish between an actual function invocation, a mention inside a doc comment, a string literal, or a markdown file.

**How Layer solves this:**
- **Compiled Tree-sitter CST Parsing**: Uses official Tree-sitter grammars (`tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-javascript`, `tree-sitter-python`, `tree-sitter-go`, `tree-sitter-cpp`) to extract exact AST node spans, signatures, docstrings, and imports.
- **AST-Verified Reference & Caller Tracking**: Walks the syntax tree to identify real identifier and call-expression nodes while automatically ignoring comments (`line_comment`, `block_comment`) and string literals (`string_literal`, `raw_string_literal`).
- **Surgical Token Efficiency**: Retrieves exact function/method implementations (`get_symbol_body`), structural outlines (`get_file_skeleton`), and line slices (`get_lines`) in a fraction of the tokens required by full-file reads.
- **Zero External Runtime Dependencies**: Uses deterministic 128-dimensional character n-gram feature hashing (`src/embedder.rs`) + lexical boosting for local chunk search—requiring no ONNX runtime, no external weight downloads, and no API keys.
- **Live Watcher & Diff History**: Includes an in-memory debounced file watcher that tracks file creations, modifications, renames, and deletions with unified diffs.

---

## Architecture & Honest Engineering Trade-offs

Layer is engineered specifically for **fast, zero-config agent navigation**, sitting deliberately between raw `ripgrep` and a full compiler frontend (`rust-analyzer`, `clangd`, `tsserver`):

| Capability | Raw `grep` / `cat` | **Layer (`tree-sitter` + `petgraph`)** | Full Compiler LSP (`rust-analyzer` / `clangd`) |
| :--- | :--- | :--- | :--- |
| **Cold startup on 850-file repo** | ~25 ms | **~1.3 sec** | 30–180+ sec (requires `cargo check` / `compile_commands.json`) |
| **Works on broken / uncompilable code** | Yes | **Yes (error-tolerant Tree-sitter CST)** | Often fails or degrades until build succeeds |
| **Extracts exact function/struct bounds** | No | **Yes (exact AST start/end rows)** | Yes |
| **Ignores comments & string literals** | No | **Yes (AST node filtering)** | Yes |
| **Cross-trait dynamic dispatch resolution** | No | **No (lexical + syntactic call matching; flagged via `is_approximate`)** | Yes (full type inference) |
| **External models / daemons required** | None | **None (single compiled binary)** | Language-specific toolchains & build artifacts |

### What Layer Is (and Isn't)
- **Real Tree-sitter Parsing (Tier 1)**: Rust, TypeScript/TSX, JavaScript/JSX, Python, Go, and C/C++ are parsed into concrete syntax trees via `tree-sitter 0.25`.
- **Lexical Block Fallback (Tier 2)**: Languages without an embedded C grammar in the binary (Java, Kotlin, C#, Swift, PHP, Scala, Dart, Zig) fall back to regex signature recognition + brace-depth span tracking.
- **Static Heuristic Call Graph**: Callers (`get_callers`) and blast radius (`get_impact_radius`) resolve call sites using AST call-expression nodes and identifier matching across indexed files. Because Layer does not run a type checker, calls on overloaded method names are transparently marked with `is_approximate` and `confidence` metadata (`"high"` vs `"approximate"`) so agents know when to disambiguate.
- **N-Gram Feature Hashing Search**: `search` uses a fast, deterministic 128-dim character 3-gram hash projection with cosine similarity and keyword boosting—not a neural transformer embedding model.

---

## Supported Languages

| Tier | Parsing Engine | Languages & Extensions | Extracted Constructs |
| :--- | :--- | :--- | :--- |
| **Tier 1: Native Tree-sitter AST** | Compiled `tree-sitter` C/Rust grammars | **Rust** (`.rs`)<br>**TypeScript / TSX** (`.ts`, `.tsx`)<br>**JavaScript / JSX** (`.js`, `.jsx`, `.mjs`, `.cjs`)<br>**Python** (`.py`, `.pyw`)<br>**Go** (`.go`)<br>**C / C++** (`.c`, `.cpp`, `.cc`, `.cxx`, `.h`, `.hpp`, `.hxx`) | Functions, methods (distinguished by enclosing `impl`/`class` AST node), arrow functions, classes, structs, enums, traits, interfaces, type aliases, namespaces, docstrings/doc-comments, imports/re-exports, and AST-filtered identifier usages. |
| **Tier 2: Lexical & Brace-Depth Fallback** | Regex signature + delimiter balancer | **Java & Kotlin** (`.java`, `.kt`)<br>**C#** (`.cs`), **Swift** (`.swift`), **PHP** (`.php`)<br>**Scala, Dart, Zig** (`.scala`, `.dart`, `.zig`) | Classes, structs, interfaces, methods, and brace-delimited function bodies. |
| **Tier 3: Universal Text & Config** | Line & n-gram chunk indexer | **All Text & Config Files** (`.rb`, `.lua`, `.sh`, `.sql`, `.toml`, `.yaml`, `.json`, `.md`, etc.) | Supported by `get_lines`, `get_tree`, `find_references`, n-gram `search`, and live diff history. |

---

## Complete Tool Inventory (9 MCP Tools + 17 CLI Commands)

### 1. The 9 MCP Tools (Agent-Facing via `layer mcp`)

| # | MCP Tool Name | Parameters | What It Returns & Why Agents Use It | Typical Payload |
| :--- | :--- | :--- | :--- | :--- |
| **1** | `get_file_skeleton` | `path` | Compact file outline: imports, `pub use` re-exports, symbol names, kinds, line spans, and signatures without function bodies. | ~350 tokens |
| **2** | `find_symbol` | `name` | Exact symbol declarations across the workspace with file paths, line ranges, signatures, and doc comments. | ~150 tokens |
| **3** | `get_symbol_body` | `name`, `file?` | Exact implementation body of a function, method, or struct without loading the surrounding file. Auto-truncates if >150 lines. | ~180 tokens |
| **4** | `get_callers` | `symbol`, `limit?` | Distinct caller functions/methods across the codebase, grouped by enclosing AST symbol with line spans and `call_count`. | ~280 tokens |
| **5** | `find_references` | `symbol`, `code_only?`, `limit?` | All usages and call-sites across the workspace. Uses AST filtering on Tier 1 languages to ignore comments and strings. | ~400 tokens |
| **6** | `get_impact_radius` | `symbol`, `file?` | 1-hop blast radius before refactoring: returns callers (who calls it), callees (what it calls internally), and confidence metrics. | ~450 tokens |
| **7** | `get_lines` | `path`, `start`, `end` | Exact slice of lines (1-indexed, inclusive) in compact plain text. | ~100–300 tokens |
| **8** | `get_tree` | *(none)* | Repository directory structure with file counts and extracted symbol counts per file. | ~250 tokens |
| **9** | `search` | `query`, `limit?` | Subword n-gram feature-hashed similarity search with lexical keyword boosting across code chunks. | ~350 tokens |

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
| `search` | `layer search <query> [-l <limit>]` | Perform n-gram vector + keyword search across codebase chunks. |
| `query` | `layer query <prompt...>` | Run a one-shot natural language query against the context engine. |
| `history` | `layer history [file]` | View in-memory change history (`CREATED`, `MODIFIED`, `DELETED`, `MOVED`). |
| `diff` | `layer diff <event_id>` | Inspect the unified diff and prior file snapshot for a history event ID. |
| `watch` | `layer watch` | Run as a live debounced file-watcher daemon printing real-time AST updates. |
| `repl` | `layer repl` | Launch the interactive colorized CLI REPL with live file watching and history. |

---

## Installation

### One-Line Installers (Zero Dependencies Required)

No Rust toolchain, C compilers, or Visual Studio Build Tools required. These scripts automatically detect your platform, download the latest pre-compiled binary, and add it to your `PATH`:

**Windows (PowerShell)**:
```powershell
irm https://raw.githubusercontent.com/junaidmirr/layer/master/install.ps1 | iex
```

**macOS & Linux**:
```bash
curl -fsSL https://raw.githubusercontent.com/junaidmirr/layer/master/install.sh | sh
```

---

### Standalone Pre-Built Binaries

Download standalone executables directly from [GitHub Releases](https://github.com/junaidmirr/layer/releases):

| Platform | Architecture | Package Format | Binary Inside |
| :--- | :--- | :--- | :--- |
| **Windows** | x86_64 | `.zip` | `layer.exe` (run directly) |
| **macOS** | Apple Silicon (M1/M2/M3/M4) | `.tar.gz` | `layer` |
| **macOS** | Intel (x86_64) | `.tar.gz` | `layer` |
| **Linux** | x86_64 | `.tar.gz` | `layer` |

---

### Docker Container

For containerized agent sandboxes (Cursor Docker, devcontainers, or remote agent runners):
```bash
docker run -i --rm -v $(pwd):/workspace ghcr.io/junaidmirr/layer:latest mcp
```

---

### Build From Source (Rust Developers)

```bash
git clone https://github.com/junaidmirr/layer.git
cd layer
cargo install --path .
```
This compiles and installs the optimized release binary to `~/.cargo/bin/layer`.

---

## MCP Client Setup

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
```markdown
Prefer `layer_*` MCP tools (`find_symbol`, `get_symbol_body`, `get_callers`, `get_impact_radius`, `find_references`) for code navigation and impact analysis before falling back to raw grep or full-file reads.
```

---

## Empirical Benchmarks Across Codebases

Layer has been evaluated in head-to-head comparisons against standard agent tooling (`grep`, `cat`, `ripgrep 15.2.0`) across C++, Rust, and TypeScript codebases:

| Codebase & Stack | Scope & Task | Baseline | Layer | Net Impact |
| :--- | :--- | :--- | :--- | :--- |
| **Creo** (C++17 / Qt) | Symbol retrieval & call-site discovery | 18,400 tokens / 14 calls | **3,100 tokens / 6 calls** | **-83.2% tokens**, 100% CTest pass |
| **Creo UI** (React 18 / TS) | Refactor monolithic 2,000-line `App.tsx` | ~35,000 tokens (3 reads) | **3,148 tokens / 9 calls** | **-91.0% tokens**, 0 full-file reads |
| **Claude Code** (Sonnet 4.5) | 6-task cumulative agent development | $1.28 / 42 requests / 5m 48s | **$0.47 / 19 requests / 2m 13s** | **-63% cost**, **-55% turns**, **-62% time** |
| **Tauri** (Rust, 856 files) | 10-task suite vs `ripgrep 15.2` | 9,333 tokens / 15 calls | **3,887 tokens / 10 calls** | **+58.4% net tokens**, 100% precision |
| **Tauri** (IPC Security Audit) | Autonomous unprompted agent audit | ~28,000 tokens (cat/grep) | **~3,500 tokens (11 calls)** | **~87% token reduction**, 100% accuracy |

### When to Use `ripgrep` vs. `layer`
* **Literal Regex & Path Globbing**: Native `ripgrep` / `glob` is faster and uses fewer tokens for simple 1-line literal regex searches or filename lookups.
* **AST & Relational Queries**: `layer_*` saves **60%–90% of tokens** when extracting complete function bodies from large files, generating file skeletons, grouping callers by enclosing function, or inspecting 1-hop impact radii.

👉 *See [`BENCHMARKS.md`](BENCHMARKS.md) for the complete task logs, recall tables, and methodology.*

---

## Security & Concurrency Guarantees

- **Workspace Path Confinement**: Every file path requested via MCP is canonicalized and verified against the workspace root (`resolve_safe_path`), blocking `../` traversal and outward symlink escapes.
- **Credential & Secret Filtering**: Hardcoded security policy blocks access to sensitive files at any depth (`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa`, `credentials.json`, `.npmrc`, `.netrc`), while allowing safe templates like `.env.example`.
- **Monotonic Version Gating**: Combines modification timestamps with deterministic 64-bit FNV-1a content hashing so stale background reads never overwrite newer indexed state.
- **Stale Edge Reconciliation**: Updating or deleting a file automatically prunes outdated dependency edges in the graph before re-linking.

---

## Development & Testing

Before submitting a pull request, run the verification suite locally:

```bash
# 1. Check formatting
cargo fmt --all -- --check

# 2. Run strict Clippy linter (zero warnings allowed)
cargo clippy --all-targets --all-features -- -D warnings

# 3. Run unit, Tree-sitter AST, and security tests
cargo test --all-targets

# 4. Build optimized release binary
cargo build --release
```

---

## Contributing

1. Fork the repository.
2. Create your feature branch (`git checkout -b feat/my-feature`).
3. Ensure `cargo fmt`, `cargo clippy`, and `cargo test` pass cleanly.
4. Open a Pull Request (GitHub Actions CI automatically verifies Ubuntu and macOS builds).

---

## License

This project is licensed under the [MIT License](LICENSE).
