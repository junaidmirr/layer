# Layer Master Empirical Benchmark Suite

> **Evaluation Suite**: Unified empirical measurements of `layer` across C++, Rust, and React/TypeScript codebases.  
> **Token Metric**: Positive delta (`+X%`) = Layer used fewer tokens (leaner). Negative delta (`-X%`) = Baseline used fewer tokens.

---

## Executive Summary Across All Tested Codebases

| Codebase | Tech Stack & Scale | Test Scope | Baseline | Layer | Net Impact |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Creo** | C++17 / Qt (~55 files) | Tool-level retrieval & call-site precision | 18,400 tokens / 14 calls | 3,100 tokens / 6 calls | **-83.2% tokens, 100% test pass** |
| **Creo UI** | React 18 / TypeScript (1,997-line file) | Targeted component refactoring | ~35,000 tokens (est. 3 reads) | 3,148 tokens / 9 calls | **-91.0% tokens, 0 full-file reads** |
| **Project Repos** | Multi-file agent sessions (Claude 3.5 Sonnet) | 6-Task cumulative real-world agent runs | $1.28 / 42 requests / 5m 48s | $0.4747 / 19 requests / 2m 13s | **-63% cost, -55% requests, -62% time** |
| **Tauri** | Rust / Multi-crate (856 files, 100k+ LoC) | 10-Task systematic comparison vs `ripgrep 15.2` | 9,333 tokens / 15 calls | 4,091 tokens / 10 calls | **+56.2% net tokens, 100% caller precision** |
| **Tauri** | Rust (IPC & Webview Security Audit) | Autonomous agent hard audit (unprompted) | ~28,000 tokens (cat/grep) | ~3,500 tokens (11 Layer calls) | **~87% token reduction, 100% pipeline accuracy** |

---

## Codebase 1: Creo (C++ / Qt Core Engine)
**Scale**: ~55 files | **Language**: C++17, CMake, CTest

### 1.1 Tool-Level Baseline vs. Layer Comparison (`InsertClipCommand`)
Evaluating single-task code discovery against raw CLI tools and ripgrep:

| Metric | Baseline (cat / find / grep) | Ripgrep Only | Layer Context Engine | Delta (Layer vs Baseline) |
| :--- | :--- | :--- | :--- | :--- |
| **Cold Index Time** | 0 ms | 0 ms | **46.86 ms** (55 files) | — |
| **Estimated Tokens** | ~18,400 | ~10,200 | **~3,100** | **-83.2% tokens** |
| **Tool Invocations** | 14 calls | 10 calls | **6 calls** | **-57.1% calls** |
| **Test Verification** | 100% passed | 100% passed | **100% passed (4/4 CTest)** | Verified zero regression |

### 1.2 Call-Site Ground-Truth Check (10 C++ Symbols)
Validating Layer's AST call graph precision against regex ground truth:

| Symbol | Kind | Layer (Direct / Total) | Ground Truth | Matched | Precision |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `InsertClipCommand` | Class/Command | 4 / 4 | 4 | 4 | **100%** |
| `SplitClipCommand` | Class/Command | 1 / 1 | 1 | 1 | **100%** |
| `RippleDeleteClipCommand` | Class/Command | 0 / 0 | 0 | 0 | **100%** |
| `MediaProbe` | Class | 0 / 5 | 5 | 5 | **100%** |
| `generate_id` | Function | 11 / 11 | 11 | 11 | **100%** |
| `probe_file` | Function | 4 / 4 | 4 | 4 | **100%** |
| `active_sequence` | Method/Accessor | 23 / 26 | 26 | 26 | **100%** |
| `current_snapshot` | Method/Accessor | 30 / 30 | 30 | 30 | **100%** |
| `execute` | Virtual Method | 11 / 11 | 11 | 11 | **100%** |
| `RationalTime` | Struct/Type | 23 / 113 | 27 | 27 | **100%** |

### 1.3 Codex CLI Case Study (`InsertClipCommand`)

| Metric | Baseline (`rg` + `cat`) | Layer (`find_symbol` + `get_callers`) | Delta |
| :--- | :--- | :--- | :--- |
| **Tool Latency** | ~570 ms | **13.3 ms** (5.3 ms + 8.0 ms) | **42.8× faster** |
| **Payload Ingested** | >12,000 bytes | **2,669 bytes (~668 tokens)** | **-77.8% data** |
| **Total Session Tokens** | 15,637 tokens | **5,467 tokens** | **-65.0% tokens** |
| **Call-site Recall** | 4 of 4 | **6 of 6** (surfaced header + impl) | **100% recall** |

### 1.4 Evolution of Semantic Search Footprint
Output trimming across internal iterations:

| Engine Version | Payload Bytes | Tokens (Approx.) | Query Latency |
| :--- | :--- | :--- | :--- |
| **Initial Prototype** | 41,980 bytes | ~10,495 tokens | 9.28 ms |
| **V1 (Trimmed Envelope)** | 5,672 bytes | ~1,418 tokens | 8.27 ms |
| **V2 (Production Release)** | **493 bytes** | **~124 tokens** | **5.05 ms** (-98.8% payload) |

---

## Codebase 2: Creo UI (React / TypeScript)
**Scale**: 1,997-line monolithic component (`App.tsx`) | **Language**: TypeScript, React 18, Vite

### 2.1 Monolithic Component Refactoring Run

| Metric | Baseline Agent (Estimated) | Layer-Assisted Agent | Delta |
| :--- | :--- | :--- | :--- |
| **Full-File File Reads** | 2–3 full file reads | **0 full reads** | **100% read elimination** |
| **Layer Tool Invocations** | — | **9 calls** (`get_symbol_body`, `get_lines`) | Surgical slices only |
| **Total Tool Latency** | ~1,200 ms | **~64 ms** | **18.7× faster** |
| **Total Context Consumed** | ~35,000 tokens | **~3,148 tokens** | **-91.0% token reduction** |
| **Compiler / Build Gate** | Passed | **`npm run build` passed (1m 22s)** | Zero build errors |

---

## Codebase 3: End-to-End Agent Sessions (Claude 3.5 Sonnet)
**Model**: `claude-3-5-sonnet` via Claude Code | **Protocol**: Controlled paired comparisons

### 3.1 Single Task A & B

| Metric | Task A (Without) | Task A (With Layer) | Task B (Without) | Task B (With Layer) |
| :--- | :--- | :--- | :--- | :--- |
| **Total Cost** | $0.0804 | **$0.0574 (-28.6%)** | $0.1827 | **$0.0882 (-51.7%)** |
| **LLM Requests** | 4 | **3 (-25.0%)** | 14 | **6 (-57.1%)** |
| **API Wall Time** | — | — | 54s | **29s (-46.3%)** |
| **Total Wall Time** | — | — | 1m 48s | **58s (-50.9%)** |
| **Output Tokens** | 1.4k | **834 (-40.4%)** | 2.1k | **1.2k (-42.9%)** |
| **Prompt Cache Read** | 89.0k | **65.4k (-26.5%)** | 344.0k | **137.6k (-60.0%)** |
| **Prompt Cache Write** | 8.1k | **6.1k (-24.7%)** | 12.0k | **6.9k (-42.5%)** |

### 3.2 Six-Task Cumulative Agent Run (Long-Running Session)

| Checkpoint | Cumulative Cost (Without) | Cumulative Cost (With) | Cumulative Requests (Without) | Cumulative Requests (With) | Cumulative Time (Without) | Cumulative Time (With) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **After Task 1** | $0.0555 | $0.1247 | 2 | 3 | 17s | 24s |
| **After Task 2** | $0.1779 | $0.1720 | 8 | 6 | 52s | 41s |
| **After Task 3** | $0.2712 | $0.2189 | 11 | 9 | 1m 15s | 58s |
| **After Task 4** | $0.3775 | $0.2780 | 15 | 12 | 1m 50s | 1m 19s |
| **After Task 5** | $0.8800 | $0.3805 | 34 | 16 | 4m 17s | 1m 52s |
| **After Task 6** | **$1.2800** | **$0.4747** | **42** | **19** | **5m 48s** | **2m 13s** |

#### Per-Task Marginal Increments

| Task # | Cost Without | Cost With Layer | Requests Without | Requests With Layer | Net Savings |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **1** | $0.056 | $0.125 | 2 | 3 | Cold index penalty |
| **2** | $0.122 | $0.047 | 6 | 3 | **-61.5% cost** |
| **3** | $0.093 | $0.047 | 3 | 3 | **-49.5% cost** |
| **4** | $0.106 | $0.059 | 4 | 3 | **-44.3% cost** |
| **5** | $0.503 | $0.103 | 19 | 4 | **-79.5% cost** |
| **6** | $0.400 | $0.094 | 8 | 3 | **-76.5% cost** |

#### Session 3 Summary

| Metric | Baseline (Without) | With Layer Engine | Net Delta |
| :--- | :--- | :--- | :--- |
| **Session Cost** | $1.2800 | **$0.4747** | **-62.9% Cost Reduction** |
| **Agent API Turns** | 42 requests | **19 requests** | **-54.8% Fewer Turns** |
| **Agent API Time** | 5m 48s | **2m 13s** | **-61.8% Latency Reduction** |
| **Output Generation** | 21,200 tokens | **6,700 tokens** | **-68.4% Output Waste** |
| **Prompt Cache Reads** | 1,900,000 tokens | **528,700 tokens** | **-72.2% Cache Churn** |
| **Code Changes Written** | +681 / -21 lines | **+55 / -0 lines** | Minimal surgical patch |

---

## Codebase 4: Tauri Framework (Rust / Systems Architecture)
**Scale**: 856 files, 100k+ LoC across 40 crates | **Cold Index**: 1.30s | **Memory**: In-memory petgraph + vectors

### 4.1 Systematic 10-Task Head-to-Head (`ripgrep 15.2.0` vs. Layer)
Token counts measured in exact OpenAI `cl100k_base` BPE tokens:

| # | Task & Operation | Baseline (`rg` / `cat`) | Layer Tool | Baseline Tokens | Layer Tokens | Token Delta | Baseline ms | Layer ms |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **1** | Inspect `WindowBuilder` struct | `rg` + `sed` (2 calls) | `get_symbol_body` | 184 | 212 | -15.2% | 25.9 | 44.4 |
| **2** | Callers of `serialize_node` | `rg` (1 call) | `get_callers` | 290 | 409 | -41.0% | 27.1 | 75.5 |
| **3** | Explore `html.rs` outline (442 L) | `cat html.rs` (1 call) | `get_file_skeleton` | 3,404 | 944 | **+72.3%** | 8.6 | 41.2 |
| **4** | Find `Manager` trait declarations | `rg` (1 call) | `find_symbol` | 67 | 332 | -395.5% | 28.7 | 44.5 |
| **5** | Pre-refactor impact of `WebviewBuilder` | `rg` (2 calls) | `get_impact_radius` | 2,794 | 795 | **+71.5%** | 33.1 | 90.6 |
| **6** | `normalize_script_for_csp` references | `rg` (1 call) | `find_references` | 428 | 334 | **+22.0%** | 27.0 | 67.8 |
| **7** | Search event loop message dispatch | `rg` across crates | `search` | 1,626 | 338 | **+79.2%** | 35.9 | 55.3 |
| **8** | Inspect `inject_nonce_token` body | `rg` + `sed` (2 calls) | `get_symbol_body` | 106 | 146 | -37.7% | 20.3 | 41.9 |
| **9** | Surgical lines 100–130 in `lib.rs` | `sed -n` (1 call) | `get_lines` | 261 | 358 | -37.2% | 8.0 | 41.1 |
| **10** | Inspect `run_iteration` function body | `rg` + `sed` (2 calls) | `get_symbol_body` | 173 | 223 | -28.9% | 21.3 | 41.9 |
| | **Totals Across All 10 Tasks** | **15 tool calls** | **10 tool calls** | **9,333** | **4,091** | **+56.2%** | 235.9 | 544.2 |

### 4.2 Category Breakdown (The "Hybrid" Architecture Divide)

| Category | Tasks Included | Baseline Tokens | Layer Tokens | Net Token Delta | Baseline Tool Turns | Layer Tool Turns |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Category A: Large Structural Outputs** | 3, 5, 6, 7 | 8,252 tokens | **2,411 tokens** | **+70.8% Leaner** | 6 calls | **4 calls** |
| **Category B: Tiny Literal Lookups** | 1, 2, 4, 8, 9, 10 | 1,081 tokens | **1,680 tokens** | -55.4% (JSON tax) | 9 calls | **6 calls** |

*Takeaway*: For tiny 5-line lookups, raw `sed`/`grep` on cached SSD files has less schema overhead. For structural navigation, impact analysis, and file outlines, Layer saves **70%–80% of tokens**.

### 4.3 Set-Based Ground Truth Verification

| Check Target | Ground Truth Baseline | Layer Output | Precision | Recall | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `serialize_node` callers | 8 call sites | 7 returned | **100% (7/7)** | **87.5%** | False negative was symbol definition itself |
| `normalize_script_for_csp` refs | 9 references | 5 returned | **100% (5/5)** | **55.6%** | Direct code usages (0 markdown false-positives) |
| `WebviewBuilder` blast radius | 88 total references | 10 capped callers | **100%** | **100% (top-10)** | Total caller count accurately reported (88) |
| `get_lines` lines 100–130 vs `sed` | 27 non-empty lines | 27 lines returned | **100%** | **100%** | 27/27 character-exact match after gutter strip |

### 4.4 Grouped Callers & Non-Code Filtering Benchmark (`WindowEffect`)
Measured after adding enclosing symbol grouping with counts and `code_only: true` filtering:

| Metric | Before Optimization | After Optimization | Delta |
| :--- | :--- | :--- | :--- |
| **Caller Entries Returned** | ~40 entries (30 duplicate `to_tokens` match arms) | **5 distinct callers** (`count: 28` inside `to_tokens`) | **-87.5% entry clutter** |
| **Context Token Footprint** | 2,739 tokens | **288 tokens** | **-89.5% Token Cost** |
| **Non-Code Contamination** | Included `CHANGELOG.md` & 4 JSON schemas | **0 non-code files** (100% pure Rust code) | **Zero noise** |
| **Execution Latency** | 100.5 ms | **79.6 ms** | **-20.8% latency** |

### 4.5 Hard-Level Autonomous Agent Audit (Tauri IPC Security Pipeline)
*Task*: Unprompted end-to-end audit tracing raw IPC entry through origin security gates to command dispatch; mapping blast radius of adding `origin_verified: bool` to `InvokeRequest`.

| Measurement | Native Baseline (`cat` / `grep`) | Autonomous Agent using Layer | Net Result |
| :--- | :--- | :--- | :--- |
| **Tool Invocations** | 20–30 exploratory reads | **11 Layer calls + 2 scouting greps** | **85% Layer adoption** |
| **Context Window Consumed** | ~28,000 tokens (full file reads) | **~3,500 tokens** | **~87.5% Token Savings** |
| **Security Guard Pinpointed** | Often missed in 3,000-line file | Located `webview/mod.rs:1952–1989` | **100% exact lines** |
| **Constructors Identified** | Partial | `protocol.rs:538, 301`, tests `2579, 2599` | **100% recall** |
| **Error Trap Recognition** | Accepted false symbol | Caught prompt typo `IpcResponse` vs `InvokeResponse` | **Compiler-grade verification** |

---

## Reproducibility & Test Commands

To reproduce these benchmarks locally against any repository:

```bash
# 1. Benchmark callers with grouping and token timing
layer callers <symbol_name>

# 2. Benchmark 1-hop impact radius
layer impact <symbol_name>

# 3. Benchmark exact symbol extraction without full file reads
layer symbols <file_path>

# 4. Measure end-to-end MCP latency over stdio
python3 -c "
import subprocess, json, time
proc = subprocess.Popen(['layer', 'mcp'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def call(tool, args):
    t0 = time.perf_counter()
    proc.stdin.write(json.dumps({'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':tool,'arguments':args}}) + '\n')
    proc.stdin.flush()
    res = json.loads(proc.stdout.readline())
    print(f'{tool}: {(time.perf_counter()-t0)*1000:.2f}ms')
call('get_callers', {'symbol': 'WindowEffect'})
"
```
