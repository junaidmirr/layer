<div align="center">

<img src="https://i.ibb.co/yFFyLDcy/Layer-Smarter-Context-Faster-Agents-1.png" width="200" alt="Layer logo" />

# Layer

**Smarter context for faster coding agents.**

A small, fast MCP server written in Rust that helps AI coding agents understand your codebase instead of guessing at it.

[![CI](https://github.com/junaidmirr/layer/actions/workflows/ci.yml/badge.svg)](https://github.com/junaidmirr/layer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)
[![Tree-sitter](https://img.shields.io/badge/Tree--sitter-Compiled%20Grammars-blueviolet.svg)](https://tree-sitter.github.io/tree-sitter/)
[![MCP](https://img.shields.io/badge/MCP-23%20Tools%20%7C%2019%20CLI%20Commands-green.svg)](https://modelcontextprotocol.io)

[Why Layer](#why-layer) · [Benchmarks](#benchmarks) · [How it works](#how-it-works) · [Languages](#supported-languages) · [Tools](#tools) · [Install](#install) · [CLI](#cli-usage)

</div>

---

## Why Layer

If you use coding agents like Claude Code, Cursor, Windsurf, Codex or OpenCode on real projects, you have probably seen these problems:

**1. The agent can't see the big picture.**
On a repo with thousands of files, agents fall back to `grep` and dump whole files into the prompt. They read 1,500 line files, burn 100k+ tokens just scanning, lose the thread and go in circles.

**2. Fixes turn into guessing games.**
The agent edits something, breaks a caller it never read, then spends five turns guessing how to repair it.

**3. New projects get hallucinated APIs.**
When starting from scratch, agents invent package functions or get stuck in broken config.

**Layer sits between your agent and your code** and talks over the standard Model Context Protocol. Instead of raw file dumps, the agent gets symbol outlines, exact function slices, impact graphs and compile checks in milliseconds.

---

## Benchmarks

Both tests used the same model, the same prompt and the same starting repo state. No cherry picked prompts. Independent testing is very welcome.

### Real bug: NSA Ghidra

Fixing a PowerPC 64 bit width mismatch in the decompiler ([Issue #9886](https://github.com/NationalSecurityAgency/ghidra/issues/9886)) using `gpt-6-luna`.

| Metric | Without Layer | With Layer (v1.3) | Change |
| :--- | ---: | ---: | :--- |
| Time taken | 7m 57s | **2m 50s** | 2.8x faster |
| Input tokens | 101,391 | **37,477** | -63.0% |
| Output tokens | 14,474 | **5,731** | -60.4% |
| Reasoning tokens | 7,313 | **2,666** | -63.5% |
| Cache read tokens | 5,880,320 | **754,432** | -87.2% |
| Total tokens | 5,996,185 | **797,640** | **-86.7%** |
| Cost (USD) | $0.08 | **$0.01** | -87.5% |

Without Layer, the agent spent nearly eight minutes on noisy text searches across thousands of C++ and Sleigh files. With Layer, it found the exact instruction semantics in under three minutes.

<details>
<summary><b>See the raw agent responses</b></summary>

<br>

**With Layer** (2m 50s, 797,640 tokens, $0.01)

> *“I made a Sleigh semantics change in ppc_instructions.sinc (Ghidra/Processors/PowerPC/data/languages/ppc_instructions.sinc:3888): tw, its trap alias, and twi now pass the low 32-bit register values to trapWord. For twi, the immediate is sign-extended to 32 bits. td and tdi are unchanged. This addresses the architectural width mismatch: these word-trap instructions compare 32-bit words even when GPRs are 64 bits. I left RuleSubCommute::applyOp unchanged; its INT_ADD case already avoids commuting a spacebase input. The issue’s attached reproducer and expected output were not present in the checkout, so I could not establish the reported function-level failure or prove the fix resolves it.*
>
> *Validation is incomplete. The Layer navigation calls did not return. The Gradle wrapper could not download Gradle because network access was unavailable, and the Sleigh launcher reports that this checkout has not been compiled. I therefore could not compile the modified Sleigh, run processor or decompiler tests, or perform a before-and-after decompilation. No regression test was added, and I can’t claim the fix passes. The change still needs Sleigh compilation and the requested reproducer or equivalent automated regression before it is validated.”*

**Without Layer** (7m 57s, 5,996,185 tokens, $0.08)

> *“The fix is incomplete; I can’t verify it against the reported failure.*
>
> *I added a RuleSubCommute::applyOp path in ruleaction.cc (Ghidra/Features/Decompiler/src/decompile/cpp/ruleaction.cc:4589). When a low-word SUBPIECE reads an INT_ADD result that has other readers, it now rewrites that SUBPIECE as a narrow add and leaves the original wide add intact. This targets the root cause described in issue #9886 (https://github.com/NationalSecurityAgency/ghidra/issues/9886), but I have not added a regression test or demonstrated before-and-after decompiler output.*
>
> *In this checkout, the PowerPC tw and twi semantics already pass 4-byte operands to trapWord in ppc_instructions.sinc. That matches word-trap behavior, so I left Sleigh unchanged. The issue’s alternative semantics patch appears unnecessary for this source; td and tdi remain separate full-width operations.*
>
> *Validation: make -j2 ghidra_opt succeeded. make -j2 test stopped because the environment lacks bfd.h. I couldn’t fetch the issue’s attached ELF because network DNS is unavailable, and the installed Clang has no PowerPC target, so I couldn’t construct an equivalent binary here. The repository also has no .git metadata, which prevents a Git diff or status check.*
>
> *The source change is therefore not yet supported by the required reproducible regression. I can’t claim the reported bug is fixed based on the validation available here.”*

</details>

### Build from scratch: 3D gravity simulator

A full 3D N Body orbital gravity and collision simulator (React, TypeScript, Three.js, Tailwind CSS) built from an empty folder.

| Metric | Without Layer | With Layer | Change |
| :--- | ---: | ---: | :--- |
| Total tokens | 1,002,538 | **567,206** | -43.4% |
| Input tokens | 59,985 | **43,844** | -26.9% |
| Output tokens | 12,505 | **7,778** | -37.8% |
| Reasoning tokens | 1,613 | **557** | -65.5% |
| Cache read tokens | 930,048 | **515,584** | -44.6% |
| Cost (USD) | $0.02 | **$0.01** | -50% |
| Build failures | 2 (a syntax error and an unused import) | **0** | Caught in under 15ms |
| Frame rate | 43 FPS, unstable third body | **61 FPS**, stable orbit | Better result |

---

## How it works

Layer does four main things.

### 1. Instant syntax checks (under 15ms)
Every edit is checked in memory using compiled Tree sitter syntax trees. Layer validates grammar, balanced brackets, strings and imports without waiting for a slow compiler to start (`tsc` alone takes 400 to 800ms to boot).

### 2. Safe multi file refactors
When an agent changes a shared interface like `src/types.ts`, Layer separates errors in the file you edited from errors in files that simply haven't been updated yet. Good interface edits are kept, and the agent gets a `pending_downstream_updates` checklist instead of a rollback.

### 3. Full compiler checks and an error circuit breaker
Layer plugs into your real toolchain (`tsc`, `cargo check`, `pyright`, `go vet`, `clang++`, `dotnet`, `javac`, `mvn`) and sorts every diagnostic into **new**, **persistent** or **resolved**. If the same error survives two or more turns, the circuit breaker steps in and feeds the agent verified type signatures and definitions so it stops guessing.

### 4. Task aware context
Instead of reading whole files, Layer ranks code using BM25F scoring, Personalized PageRank and test pairing. It returns small slices with exact line numbers, saving three to five wasted grep and read turns per task.

### Built to scale (v1.3)

- **No lock contention.** Indexing runs in the background and swaps in atomically, so agent queries never wait on a big scan.
- **Fast cold starts.** A seed scanner capped at 250ms handles the first query on a fresh repo.
- **Constant time lookups.** Hash indexes replace full repo scans, so 10,000+ symbols are searched in under 40ms with a hard 450ms deadline.
- **Do no harm by default.** `auto_rollback_on_cascade` is off, so modern syntax (Python 3.12, TypeScript 5.x) never triggers a false rollback.

---

## Supported languages

Layer supports 25+ languages through compiled Tree sitter grammars and its own ULBA engine (Universal Lexical and Block Automaton).

| Language | Symbols and graph | Instant syntax check | Package inspection | Scaffolding | Compiler check |
| :--- | :---: | :---: | :---: | :---: | :---: |
| **TypeScript and JavaScript** | ✅ Tree sitter | ✅ | ✅ `node_modules`, `.d.ts` | ✅ | ✅ `tsc` |
| **Rust** | ✅ Tree sitter | ✅ | ✅ Cargo crates | ✅ | ✅ `cargo check` |
| **Python** | ✅ Tree sitter | ✅ | ✅ `.venv`, stubs | ✅ | ✅ `pyright` |
| **Go** | ✅ Tree sitter | ✅ | ✅ `go.mod` | ✅ | ✅ `go vet` |
| **C and C++** | ✅ Tree sitter | ✅ | ✅ `#include` headers | ✅ | ✅ AST fallback |
| **Java** | ✅ ULBA | ✅ | ✅ Spring Boot | ✅ | ✅ `javac` |
| **C#** | ✅ ULBA | ✅ | ✅ ASP.NET | ✅ | ✅ `dotnet` |
| **Swift** | ✅ ULBA | ✅ | ✅ SPM | ✅ | ✅ AST fallback |
| **Kotlin** | ✅ ULBA | ✅ | ✅ Gradle | ✅ | ✅ AST fallback |
| **Ruby, PHP, Scala, Dart, Zig, Elixir, Lua, SQL, Shell** | ✅ ULBA | ✅ | ✅ Manifests | ✅ | ✅ AST fallback |

---

## Tools

Layer exposes 23 tools over standard MCP JSON-RPC.

### Navigate and understand

| Tool | What it does |
| :--- | :--- |
| `find_symbol` | Finds a declaration with its file, line range, signature and docstring |
| `get_symbol_body` | Returns just one function, method or struct without the surrounding file |
| `get_file_skeleton` | A compact outline of any file in about 350 tokens |
| `get_callers` | Lists every caller of a function, grouped by enclosing symbol |
| `find_references` | Finds all usages, ignoring comments and strings |
| `get_impact_radius` | Shows callers, callees and confidence before you refactor |
| `get_lines` | Reads an exact range of lines with zero overhead |
| `get_tree` | Shows the repo layout with file counts and symbol density |
| `search` | Fast fuzzy search across the codebase |
| `prepare_task_context` | Builds a compact context packet for a given prompt |
| `set_workspace` | Switches the active workspace root |

### Start new projects

| Tool | What it does |
| :--- | :--- |
| `bootstrap_project` | Creates a fresh project (`react-ts`, `rust-bin`, `rust-lib`, `python`, `go`, `cpp`, `swift`, `kotlin`, `csharp`, `java`) without overwriting anything |
| `scaffold_module` | Generates a multi file module with typed interfaces and stubs in one turn |
| `scaffold_template` | Creates a starter template such as `rust-cli` |
| `inspect_package_api` | Reads real package declarations before the agent imports them |
| `init_harness` | Writes `AGENTS.md`, `CLAUDE.md` or `.cursorrules` so agents prefer Layer tools |

### Edit safely

| Tool | What it does |
| :--- | :--- |
| `write_file` | Writes a file with a snapshot, syntax check and circuit breaker. Supports `verify_mode` of `fast`, `full` or `none` |
| `edit_file` | Applies exact block edits with a syntax check and automatic rollback |
| `rename_symbol` | Renames a symbol across the whole project in one go, rolling back everything if a syntax error appears |
| `rollback_last_edit` | Undoes the latest change, including multi file edits |

### Verify and measure

| Tool | What it does |
| :--- | :--- |
| `verify_project` | Runs checks and compares against the last run (`new_errors`, `persistent_errors`, `resolved_errors`) |
| `verify_impacted_tests` | Runs only the tests affected by your changes |
| `get_session_stats` | Shows tool calls, latency, estimated tokens saved and interventions |

---

## Install

### Quick install

No Rust toolchain or C compiler needed. The script finds your platform, downloads the latest binary and adds it to your `PATH`.

**macOS and Linux**
```bash
curl -fsSL https://raw.githubusercontent.com/junaidmirr/layer/master/install.sh | sh
```

**Windows (PowerShell)**
```powershell
irm https://raw.githubusercontent.com/junaidmirr/layer/master/install.ps1 | iex
```

### Download a binary

Grab a build from [GitHub Releases](https://github.com/junaidmirr/layer/releases).

| Platform | Architecture | Format |
| :--- | :--- | :--- |
| Windows | x86_64 | `.zip` |
| macOS | Apple Silicon | `.tar.gz` |
| macOS | Intel | `.tar.gz` |
| Linux | x86_64 | `.tar.gz` |

### Docker

```bash
docker run -i --rm -v $(pwd):/workspace ghcr.io/junaidmirr/layer:latest mcp
```

### Build from source

```bash
git clone https://github.com/junaidmirr/layer.git
cd layer
cargo install --path . --force
layer --help
```

---

## Connect to your agent

Add Layer as an MCP server in your agent's config.

**Claude Code** (`~/.claude/claude_desktop_config.json` or your project config) and **Cursor** (`~/.cursor/mcp.json`)
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

**Windsurf, OpenCode or any MCP client**
```json
{
  "name": "layer",
  "command": "layer",
  "args": ["mcp"],
  "transport": "stdio"
}
```

---

## CLI usage

You can also use Layer straight from the terminal.

```bash
layer mcp                        # start the MCP server
layer skeleton src/graph.rs      # outline a file
layer find VectorGraph           # locate a symbol
layer body resolve_module_import_path   # print one function
layer impact inspect_package_api # see what an edit could affect
layer watch                      # run the file watcher
layer stats                      # live session dashboard
layer stats --reset              # reset the counters
layer init                       # create steering files for your agent
layer init codex                 # steering file for Codex
layer init claude                # steering file for Claude Code
layer init --alias               # enable `codex --layer` and `claude --layer`
```

---

## Development

```bash
cargo test                                              # 101 tests across 10 suites
cargo clippy --all-targets --all-features -- -D warnings # lint
cargo fmt --check                                       # formatting
```

---

## License

Layer is released under the [MIT License](LICENSE).
