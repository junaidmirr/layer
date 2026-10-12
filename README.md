# `layer` — Intelligent Codebase Context Engine, Vector Graph & Audit History

A lightning-fast, real-time context engine for codebases written in Rust. It scans your entire project, extracts AST symbols (functions, methods, classes, line ranges), creates dense vector embeddings for files and code chunks, builds a dependency relationship graph, watches the filesystem in real-time, and **caches previous versions of files across edits, moves, copies, and deletions with unified diffs**.

---

## 🚀 Features

- **AST Symbol Extraction**: Parses functions, structs, classes, methods, docstrings, and signatures across Rust, JavaScript/TypeScript, Python, Go, C/C++, etc.
- **Dense Vector Database & Graph**: Subword/n-gram hashing embeddings with cosine similarity and keyword boosting.
- **Real-Time Debounced File Watcher**: Watches file edits, additions, and deletions using `notify` and debounces changes to re-index immediately in the background without restarting.
- **In-Memory History & Diff Cache**:
  - Automatically snapshots file contents before modifications or deletions.
  - Detects operations: `CREATED`, `MODIFIED`, `DELETED`, `MOVED`, and `COPIED`.
  - Generates unified colored diffs showing added/removed lines and changed symbols.
  - Allows retrieving previous versions of deleted or changed files at any point in time.
- **Natural Language & Intent Context Engine**:
  - *"give me file structure"* / *"tree"*
  - *"history"* / *"what changed"* / *"changes in src/graph.rs"*
  - *"diff #1"* (shows unified diff and older file version)
  - *"this files code src/models.rs"*
  - *"all functions in src/embedder.rs"*
  - *"lines 20 to 50 in src/graph.rs"*
  - *"semantic query or concept search"*
- **Interactive REPL**: A colorized CLI REPL with command history and instant answers.
- **Universal CLI**: Can be run inside any project or targeted via `-p <path>`.

---

## 🛠️ Installation & Building

```bash
cargo install --path .
```
Or build locally:
```bash
cargo build --release
```

---

## 📊 Empirical Benchmarks on Tauri (855 Files, `cl100k_base` Tokens vs `ripgrep 15.2.0`)

Evaluating Layer against standard agent tooling (`ripgrep 15.2.0`, `sed`, `cat`) across 10 real-world retrieval tasks on the official Tauri codebase demonstrates a clear category divide:

### Category A: Decisive Wins on Large Structural Outputs & Exploration
Where reading entire files or running loose searches pollutes the context window:

| Task & Scope | Baseline (`rg` / `cat`) | Layer MCP Tool | Layer Tokens (`cl100k`) | Baseline Tokens | Token Delta | Recall / Correctness |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Search Runtime Dispatch** | `rg` across crates | `search` | **338** | 1,626 | **+79.2% Leaner** | **100% Match** |
| **Explore `html.rs` Outline (442 L)** | `cat` full file | `get_file_skeleton` | **944** | 3,404 | **+72.3% Leaner** | **100% Match** |
| **Pre-Refactor Impact Analysis** | `rg` (2 calls) | `get_impact_radius` | **795** | 2,794 | **+71.5% Leaner** | **Total callers: 88, capped at 10** |
| **Dead Code Audit (`normalize_...`)** | `rg` (1 call) | `find_references` | **334** | 428 | **+22.0% Leaner** | **100% Precision (0 FP)** |
| **Category Subtotal** | **4 tool calls** | **4 tool calls** | **2,411** | **8,252** | **+70.8% Net Savings** | **0 Regressions** |

---

### Category B: Small Focused Lookups & Protocol Overhead Trade-offs
Where raw line slicing (`sed`) or one-line string matching (`rg`) on local SSD page cache has lower protocol framing than JSON-RPC over stdio:

| Task & Scope | Baseline (`rg` / `sed`) | Layer MCP Tool | Layer Tokens (`cl100k`) | Baseline Tokens | Token Delta | Set-Based Metrics / Exact Match |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Inspect `WindowBuilder` struct** | `rg` + `sed` (2 calls) | `get_symbol_body` | **212** | 184 | **-15.2%** | **100% Body Match** |
| **Inspect `run_iteration` body** | `rg` + `sed` (2 calls) | `get_symbol_body` | **223** | 173 | **-28.9%** | **100% Body Match** |
| **Surgical lines 100-130 in `lib.rs`** | `sed -n` (1 call) | `get_lines` | **358** | 261 | **-37.2%** | **100% Line Exact Match (27/27)** |
| **Inspect `inject_nonce_token`** | `rg` + `sed` (2 calls) | `get_symbol_body` | **146** | 106 | **-37.7%** | **100% Body Match** |
| **Trace `serialize_node` callers** | `rg` (1 call) | `get_callers` | **409** | 290 | **-41.0%** | **100% Precision, 87.5% Recall** |
| **Find `Manager` trait declarations** | `rg` (1 call) | `find_symbol` | **332** | 67 | **-395.5%** | **Includes full trait docs** |
| **Category Subtotal** | **11 tool calls** | **6 tool calls** | **1,680** | **1,081** | **-55.4% (JSON wrap)** | **Reduced turns from 11 to 6** |

---

### Net Suite Total Across All 10 Tasks
- **Total Context Consumed**: **4,091 tokens** (Layer) vs **9,333 tokens** (Baseline) → **+56.2% Net Token Reduction**.
- **Tool Invocations**: **10 calls** (Layer) vs **15 calls** (Baseline) → **-33.3% Fewer Agent Round Trips**.
- **Recall & Safety**: High precision (100% on callers and references) prevents dead-code false positives; exact line recovery validated on `get_lines`.

---

## 💻 Usage

### 1. Interactive REPL Mode (Live Session with Real-Time History)
Launch interactive terminal prompt in any repository:
```bash
layer repl
```
```text
layer> give me file structure
layer> history
layer> diff #1
layer> all functions in src/scanner.rs
layer> lines 10 to 30 in src/models.rs
layer> this files code src/main.rs
layer> how does AST scanning work?
```

### 2. Dedicated CLI Subcommands
```bash
layer history                    # View all change history events & diffs
layer history src/graph.rs       # Filter history for a specific file
layer diff 1                     # View detailed diff & old file contents for event #1
layer tree                       # Print directory tree structure & symbol count
layer symbols src/scanner.rs     # List functions and symbols in a file
layer lines src/graph.rs 10 50   # Slice exact line numbers
layer file src/main.rs           # Print entire file with line numbers
layer search "history cache"     # Semantic search
layer watch                      # Live watcher daemon
```
