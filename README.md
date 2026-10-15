<img src="https://i.ibb.co/yFFyLDcy/Layer-Smarter-Context-Faster-Agents-1.png" width="200" />

# Layer — The Intelligent Codebase & Greenfield Engine for AI Coding Agents

[![CI](https://github.com/junaidmirr/layer/actions/workflows/ci.yml/badge.svg)](https://github.com/junaidmirr/layer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Tree-sitter](https://img.shields.io/badge/Tree--sitter-Compiled%20Grammars-blueviolet.svg)](https://tree-sitter.github.io/tree-sitter/)
[![MCP Compliant](https://img.shields.io/badge/MCP-23%20Tools%20%7C%2019%20CLI%20Commands-green.svg)](https://modelcontextprotocol.io)

**A high-performance Rust MCP (Model Context Protocol) server that sits between AI coding agents and codebases—performing surgical AST operations, dependency graph analysis, external package API inspection, and stateful error circuit-breaking in milliseconds.**

---

## The Dual Superpower

AI coding agents (Claude Code, Cursor, Windsurf, OpenCode, Codex) frequently burn through token budgets and stumble into multi-turn failure loops. Layer eliminates these bottlenecks across both stages of the software engineering lifecycle:

1. **Existing Codebases (Medium to Large — e.g., Tauri, C++, Rust, Monorepos)**:
   - Eliminates whole-file context dumps via sub-millisecond AST symbol extraction and precise dependency graph navigation.
   - Finds callers, callees, and 1-hop refactoring impact radii using concrete syntax trees (`tree-sitter`) without blowing past context limits.
   - Provides deterministic context packets (`prepare_task_context`) respecting exact token ceilings.

2. **Greenfield Projects (Building from Scratch)**:
   - **Zero API Hallucinations**: `inspect_package_api` extracts real TypeScript definitions (`.d.ts`), Cargo manifests, and library signatures directly from dependencies or instant offline fallbacks.
   - **Immediate Architecture Scaffolding**: `scaffold_module` and `bootstrap_project` convert natural language requests into structured, multi-file codebases with instant graph indexing and typed stubs on Turn 1.
   - **Sub-15ms Pre-Mutation Verification**: AST and delimiter pre-checks catch syntax errors before compiler execution.
   - **Stateful Error Circuit Breaker**: Tracks diagnostic lifecycles (`new`, `persistent`, `resolved`), stopping repetitive trial-and-error loops by automatically injecting ground-truth symbol signatures.

---

## Real-World Benchmark Results

Head-to-head empirical benchmark building a complete 3D N-Body Orbital Gravity & Collision Simulator (`React + TypeScript + Three.js / @react-three/fiber + Tailwind CSS`) from scratch:

| Metric | Native Agent (Without Layer) | Agent + Layer v1.0 | Improvement |
| :--- | ---: | ---: | :--- |
| **Total Tokens** | 1,002,538 | **567,206** | **-43.4% (435k tokens saved)** |
| **Input Tokens** | 59,985 | **43,844** | **-26.9%** |
| **Output Tokens** | 12,505 | **7,778** | **-37.8% (less file rewriting)** |
| **Reasoning Tokens** | 1,613 | **557** | **-65.5% (eliminated trial-and-error)** |
| **Cache Read Tokens** | 930,048 | **515,584** | **-44.6% (fewer turns)** |
| **Cost (USD)** | $0.02 | **$0.01** | **50% cost reduction** |
| **Build / Syntax Failures** | 2 failed builds (`esbuild` syntax error + unused import) | **0 syntax errors** (caught in <15ms by AST pre-check) | **100% cleaner first-pass code** |
| **Simulation Output** | 43 FPS (unstable 3rd body trajectory) | **61 FPS** (stable orbit + 3D reference grid & telemetry) | **Higher architectural quality** |

---

## How Layer Works (System Architecture)

```mermaid
flowchart TD
    Agent([AI Coding Agent\nClaude Code / Cursor / Windsurf]) <-->|stdio JSON-RPC / MCP| Layer[Layer MCP Server]

    subgraph MutationPipeline [Surgical Mutation & Verification Engine]
        Layer -->|write_file / edit_file| PreSnapshot[Pre-Mutation Snapshot Buffer]
        PreSnapshot --> FastPath{Tier-1 Fast Path\n< 15ms In-Memory}

        FastPath -->|Tree-sitter CST Check| SyntaxCheck[Lexical & Delimiter Syntax Check]
        FastPath -->|Petgraph VectorGraph| ImportCheck[Import & Path Alias Resolution]

        SyntaxCheck -->|Syntax Error Found| InstantReject[Instant Rejection & Diagnostic]
        SyntaxCheck -->|Valid Syntax| PartitionCheck{Error Partitioning}

        PartitionCheck -->|Target File Error| AutoRollback[Auto-Rollback to Snapshot]
        PartitionCheck -->|Downstream File Errors| PendingDownstream[Preserve Interface Edit\nReturn Pending Downstream Checklist]

        FastPath -->|Tier-2 Full Check| CompilerCheck[Compiler / Typechecker\ntsc, cargo check, pyright, go vet]
    end

    subgraph CircuitBreaker [Stateful Error Circuit Breaker]
        CompilerCheck --> LifecycleTracker[Error Lifecycle Tracker]
        LifecycleTracker -->|Track Occurrence| DiagStates{Diagnostic State}
        DiagStates -->|First Seen| NewErr[New Error]
        DiagStates -->|Resolved| ResolvedErr[Resolved Error]
        DiagStates -->|Occurrence >= 2| PersistentErr[Persistent Error Spiral]

        PersistentErr --> ContextInjection[Auto-Inject Ground-Truth Context\nfrom Graph & PackageIndexer]
        ContextInjection --> Agent
    end

    subgraph GreenfieldEngine [Greenfield Intelligence]
        Layer -->|inspect_package_api| PackageIndex[External Package & .d.ts Indexer]
        Layer -->|scaffold_module| ModuleScaffold[Multi-File Architecture Scaffolder]
        Layer -->|bootstrap_project| ProjectBootstrapper[Universal Preset Bootstrapper]
        ModuleScaffold --> VectorGraph[Live In-Memory VectorGraph]
        ProjectBootstrapper --> VectorGraph
    end
```

### Architectural Highlights

1. **Tier-1 Fast Path (<15ms In-Memory)**:
   - Uses compiled Tree-sitter concrete syntax trees to validate grammar, balanced delimiters (`{ }`, `( )`, `[ ]`), strings, and import declarations in memory.
   - Bypasses expensive compiler process cold-starts (e.g. `tsc` 400–800ms startup) during rapid agent edits.

2. **Downstream Refactoring Protection**:
   - Solves the multi-file refactoring auto-rollback trap. When an agent updates a shared interface (e.g., `src/types.ts`), Layer distinguishes between errors in the *target file* versus *downstream files* awaiting updates.
   - Instead of rolling back valid interface edits, Layer preserves the change and returns a structured `pending_downstream_updates` checklist.

3. **Tier-2 Incremental Verification & Error Circuit Breaker**:
   - Integrates with native toolchains (`tsc`, `cargo check`, `pyright`, `go vet`, `clang++`, `dotnet`, `javac`/`mvn`).
   - Categorizes diagnostics into `new`, `persistent`, and `resolved`.
   - When an error persists across 2+ turns, Layer engages the **Circuit Breaker**, auto-injecting verified type signatures and definitions directly into the prompt to halt trial-and-error loops.

---

## Universal Polyglot Support Matrix

Layer provides universal intelligence across 9 major programming languages and environments:

| Language | AST Symbol & Graph Extraction | <15ms AST Syntax Pre-Check | External Package & Header Inspection | Multi-File Skeleton Scaffolding | Compiler Verification & Error Lifecycle |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **TypeScript / JavaScript** | ✅ Full Tree-sitter | ✅ Instant | ✅ `node_modules`, `.d.ts` + built-in fallback | ✅ Typed interfaces & stubs | ✅ `tsc` / AST fallback |
| **Rust** | ✅ Full Tree-sitter | ✅ Instant | ✅ Cargo manifests & crates | ✅ Structs, traits & `todo!()` | ✅ `cargo check` JSON |
| **Python** | ✅ Full Tree-sitter | ✅ Instant | ✅ `.venv`, stubs + built-in fallback | ✅ Classes & `NotImplementedError` | ✅ `pyright` / AST fallback |
| **Go** | ✅ Full Tree-sitter | ✅ Instant | ✅ `go.mod` & package signatures | ✅ Structs, funcs & `panic()` | ✅ `go vet` / AST fallback |
| **C / C++** | ✅ Full Tree-sitter | ✅ Instant | ✅ `#include` headers + built-in fallback | ✅ `#pragma once`, headers & stubs | ✅ AST fallback |
| **Swift** | ✅ OOP Scanner | ✅ Instant | ✅ SPM `Package.swift` | ✅ Protocols & `fatalError()` | ✅ AST fallback |
| **Kotlin** | ✅ OOP Scanner | ✅ Instant | ✅ Gradle dependencies | ✅ Data classes & `TODO()` | ✅ AST fallback |
| **C#** | ✅ OOP Scanner | ✅ Instant | ✅ ASP.NET & JSON fallbacks | ✅ Classes & `NotImplementedException` | ✅ AST fallback |
| **Java** | ✅ OOP Scanner | ✅ Instant | ✅ Spring Boot & Web fallbacks | ✅ Classes & `UnsupportedOperationException` | ✅ AST fallback |

---

## Complete MCP Tool Inventory (23 Tools)

All tools are exposed over standard Model Context Protocol (MCP) JSON-RPC:

### 1. Codebase Intelligence & Navigation
- `find_symbol`: Locate exact symbol declarations across the repository with file paths, line ranges, signatures, and docstrings.
- `get_symbol_body`: Retrieve the exact implementation of a function, method, or struct without loading the surrounding file.
- `get_file_skeleton`: Produce a compact outline (imports, exports, types, signatures, line numbers) of any file (~350 tokens).
- `get_callers`: List all callers of a function or method across the codebase, grouped by enclosing AST symbol.
- `find_references`: Find all identifier usages and call-sites across the project, ignoring comments and string literals.
- `get_impact_radius`: Calculate the 1-hop blast radius of a symbol before refactoring (callers, callees, and confidence metrics).
- `get_lines`: Read an exact slice of lines (1-indexed, inclusive) with zero overhead.
- `get_tree`: Display repository directory hierarchy with file counts and extracted symbol densities.
- `search`: Fast subword n-gram vector search with lexical boosting across codebase chunks.
- `prepare_task_context`: Compile a deterministic, token-budgeted context packet tailored to a user prompt.
- `set_workspace`: Dynamically switch the active workspace root directory.

### 2. Greenfield & External API Intelligence
- `bootstrap_project`: Bootstrap a fresh project using ecosystem CLI generators (`react-ts`, `rust-bin`, `rust-lib`, `python`, `go`, `cpp`, `swift`, `kotlin`, `csharp`, `java`). Guarantees zero overwrites and immediate graph indexing.
- `scaffold_module`: Scaffold a multi-file architectural module in a single turn with typed interfaces, imports, and stubs.
- `inspect_package_api`: Inspect exact third-party package declarations (`.d.ts`, Rust signatures, Python stubs) before importing to prevent hallucinations.
- `scaffold_template`: Scaffold pre-defined starter templates (e.g. `rust-cli`).
- `init_harness`: Prime AI coding agents (Codex, Claude Code, Cursor, OpenCode, Windsurf) by generating or appending steering instructions (`AGENTS.md`, `CLAUDE.md`, `.cursorrules`) so models automatically prioritize Layer MCP tools over native bash/edit commands.

### 3. Surgical File Mutation & Cross-File Refactoring
- `write_file`: Write or create a file with automatic pre-mutation snapshotting, AST syntax validation, and circuit-breaker verification.
  - Supports `verify_mode`: `"fast"` (sub-15ms AST check), `"full"` (compiler typecheck), or `"none"`.
  - Returns `pending_downstream_updates` if an edit breaks downstream consumers during multi-file refactoring.
- `edit_file`: Surgically apply contiguous edits with exact block matching, AST pre-checking, and automatic rollback on local syntax errors.
- `rename_symbol`: Atomically rename any identifier or symbol across all files in the project in a single turn. Uses Tree-sitter CST analysis to strictly prune comments and strings, applies reverse byte order replacements, and automatically rolls back all touched files if any syntax error is introduced.
- `rollback_last_edit`: Revert the most recent mutation from the snapshot buffer (supports atomic multi-file rollbacks).

### 4. Verification & Selective Testing
- `verify_project`: Run structured project verification with diagnostic comparisons against previous runs:
  - Tracks diagnostic states: `new_errors`, `persistent_errors`, and `resolved_errors`.
  - Automatically attaches `ground_truth_context` when errors persist across multiple attempts.
  - Guarantees `success: true` when 0 errors are present (never returns `success: false` with 0 diagnostics).
- `verify_impacted_tests`: Graph-driven selective test runner. Traverses incoming dependency edges in the `VectorGraph` to run only tests impacted by the modified files, skipping unaffected test suites to save LLM turns and execution latency.

### 5. Built-in ROI & Session Telemetry
- `get_session_stats`: Inspect live session ROI metrics, including tool call counts, latency breakdown, estimated token savings, LLM turns saved, syntax errors caught, and circuit breaker interventions.

---

## Quickstart & Installation

### 1. One-Line Installers (Zero Dependencies Required)

No Rust toolchain, C compiler, or Visual Studio Build Tools required. These scripts automatically detect your platform, download the latest pre-compiled binary, and install it to your `PATH`:

**macOS & Linux**:
```bash
curl -fsSL https://raw.githubusercontent.com/junaidmirr/layer/master/install.sh | sh
```

**Windows (PowerShell)**:
```powershell
irm https://raw.githubusercontent.com/junaidmirr/layer/master/install.ps1 | iex
```

---

### 2. Standalone Pre-Built Binaries

Download standalone executables directly from [GitHub Releases](https://github.com/junaidmirr/layer/releases):

| Platform | Architecture | Package Format | Binary Inside |
| :--- | :--- | :--- | :--- |
| **Windows** | x86_64 | `.zip` | `layer.exe` (run directly) |
| **macOS** | Apple Silicon (M1/M2/M3/M4) | `.tar.gz` | `layer` |
| **macOS** | Intel (x86_64) | `.tar.gz` | `layer` |
| **Linux** | x86_64 | `.tar.gz` | `layer` |

---

### 3. Docker Container

For containerized agent sandboxes (Cursor Docker, devcontainers, or remote agent runners):
```bash
docker run -i --rm -v $(pwd):/workspace ghcr.io/junaidmirr/layer:latest mcp
```

---

### 4. Build From Source (Rust Developers)

```bash
git clone https://github.com/junaidmirr/layer.git
cd layer
cargo install --path . --force
```

Verify installation:
```bash
layer --help
```

---

### 5. Configure Layer with Your AI Agent

Add Layer as an MCP server in your agent configuration:

#### Claude Code (`~/.claude/claude_desktop_config.json` or project config)
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

#### Cursor (`~/.cursor/mcp.json` or Workspace Settings)
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

#### Windsurf / OpenCode / Any MCP Client
```json
{
  "name": "layer",
  "command": "layer",
  "args": ["mcp"],
  "transport": "stdio"
}
```

---

## Developer & CLI Usage

Layer can also be used directly from the command line:

```bash
# Start MCP server
layer mcp

# Inspect a file skeleton
layer skeleton src/graph.rs

# Locate a symbol across the codebase
layer find VectorGraph

# Extract an exact function body
layer body resolve_module_import_path

# View 1-hop impact radius before editing
layer impact inspect_package_api

# Run live file-watching daemon
layer watch

# Display live ROI & session telemetry dashboard
layer stats

# Reset telemetry session counters
layer stats --reset

# Prime workspace with steering files (AGENTS.md, CLAUDE.md, .cursorrules)
layer init
layer init codex
layer init claude

# Output shell function wrappers (to enable 'codex --layer' or 'claude --layer')
layer init --alias
```

---

## Running the Test Suite

Layer maintains a rigorous, 100% passing test suite with zero warnings:

```bash
# Run all 76 unit and integration tests across 8 test suites
cargo test

# Run Clippy checks
cargo clippy -- -D warnings

# Verify formatting
cargo fmt --check
```

---

## License

Licensed under the [MIT License](LICENSE).
