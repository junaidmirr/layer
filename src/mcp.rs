use crate::graph::VectorGraph;
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Runs an MCP-compliant JSON-RPC stdio server
pub fn run_mcp_server(graph: Arc<Mutex<VectorGraph>>, mut root: PathBuf) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    let mut line = String::new();
    let mut recent_queries: Vec<String> = Vec::new();
    let mut pkg_indexer = crate::package_indexer::PackageIndexer::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF reached: client disconnected cleanly
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("[layer-mcp] Stdin read error: {}", e);
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(val) => {
                if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
                    let id = val.get("id").cloned();
                    if let Err(e) = handle_rpc_method(
                        &graph,
                        &mut root,
                        method,
                        id,
                        val.get("params"),
                        &mut writer,
                        &mut recent_queries,
                        &mut pkg_indexer,
                    ) {
                        eprintln!("[layer-mcp] RPC method error: {}", e);
                        if e.kind() == io::ErrorKind::BrokenPipe {
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "[layer-mcp] Warning: Received invalid JSON line: {} ({})",
                    trimmed, err
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_rpc_method(
    graph: &Arc<Mutex<VectorGraph>>,
    root: &mut PathBuf,
    method: &str,
    id: Option<serde_json::Value>,
    params: Option<&serde_json::Value>,
    writer: &mut io::StdoutLock,
    recent_queries: &mut Vec<String>,
    pkg_indexer: &mut crate::package_indexer::PackageIndexer,
) -> io::Result<()> {
    match method {
        "initialize" => {
            // If the client supplies rootUri, rootPath, or workspaceFolders, update workspace root
            if let Some(p) = params {
                let client_uri = p.get("rootUri").and_then(|u| u.as_str()).or_else(|| {
                    p.get("workspaceFolders")
                        .and_then(|w| w.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|obj| obj.get("uri"))
                        .and_then(|u| u.as_str())
                });

                if let Some(uri_str) = client_uri {
                    let cleaned_path = if let Some(stripped) = uri_str.strip_prefix("file://") {
                        stripped
                    } else {
                        uri_str
                    };
                    let new_root = PathBuf::from(cleaned_path);
                    if new_root.exists() && new_root.is_dir() && new_root != *root {
                        eprintln!(
                            "[layer-mcp] Switching workspace root to: {}",
                            new_root.display()
                        );
                        *root = new_root.clone();
                        if let Ok(mut g) = graph.lock() {
                            g.clear();
                            g.sync_workspace(&new_root);
                        }
                    }
                } else if let Some(path_str) = p.get("rootPath").and_then(|u| u.as_str()) {
                    let new_root = PathBuf::from(path_str);
                    if new_root.exists() && new_root.is_dir() && new_root != *root {
                        eprintln!(
                            "[layer-mcp] Switching workspace root to: {}",
                            new_root.display()
                        );
                        *root = new_root.clone();
                        if let Ok(mut g) = graph.lock() {
                            g.clear();
                            g.sync_workspace(&new_root);
                        }
                    }
                }
            }

            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "layer-context-engine",
                        "version": "0.1.0"
                    },
                    "capabilities": {
                        "tools": {}
                    },
                    "instructions": "CRITICAL WORKFLOW: Always prefer Layer MCP tools over generic native file/shell tools when working on this project.\n1. Scaffolding & Setup: Use `bootstrap_project` or `scaffold_module` to scaffold verified structures with 0 hallucinated files and instant graph indexing.\n2. External APIs: Always call `inspect_package_api` before importing or using any third-party library to get exact type definitions and avoid hallucinated functions.\n3. Writing & Editing: ALWAYS use `write_file` and `edit_file`. They include automatic pre-mutation snapshotting, sub-5ms AST pre-checking, and error circuit breaking.\n4. Verification: Run `verify_project` to check project health with compact diagnostics, lifecycle tracking (new vs persistent vs resolved), and ground-truth error context injection.\n5. Refactoring: If an interface edit causes downstream errors, do NOT rollback; inspect the `pending_downstream_updates` checklist and update downstream files sequentially."
                }
            });
            send_json_line(writer, &resp)?;
        }
        "notifications/initialized" => {
            // No response required for notifications
        }
        "tools/list" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "get_file_skeleton",
                            "description": "PREFERRED over reading entire files. Retrieves the outline of a file (imports, classes, functions, and method signatures with line numbers) without bodies. Use this first when exploring a file or understanding module structure.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path to the file in the workspace"
                                    }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "find_symbol",
                            "description": "PREFERRED over grep for symbol declarations. Finds exact definitions of a function, class, or struct. For common names ('run', 'new'), pass 'file' or 'limit' to avoid broad lists.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Exact or partial symbol name (e.g., 'TimelineView', 'InsertClipCommand')"
                                    },
                                    "file": {
                                        "type": "string",
                                        "description": "Optional file path hint to narrow down search"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Optional maximum results to return (default: 15)"
                                    }
                                },
                                "required": ["name"]
                            }
                        },
                        {
                            "name": "find_references",
                            "description": "Finds all occurrences, usages, and references to a symbol across the codebase. Set code_only=true to filter out documentation, markdown, and JSON schemas.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "The function, class, or symbol name to look up"
                                    },
                                    "code_only": {
                                        "type": "boolean",
                                        "description": "Optional: If true, only returns references in code files (.rs, .ts, .py, etc.), ignoring docs and schemas."
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Optional: Maximum references to return (default 60)"
                                    }
                                },
                                "required": ["symbol"]
                            }
                        },
                        {
                            "name": "get_callers",
                            "description": "PREFERRED over ripgrep for finding callers. Returns distinct caller functions and methods across the codebase, grouped by enclosing function with call counts and line ranges.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "The function, class, or method name to find callers for"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Optional: Maximum distinct callers to return (default 25)"
                                    }
                                },
                                "required": ["symbol"]
                            }
                        },
                        {
                            "name": "get_lines",
                            "description": "PREFERRED over reading entire files with cat/head. Retrieves an exact slice of lines from a file (1-based, inclusive) in compact plain text.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path to the file"
                                    },
                                    "start": {
                                        "type": "integer",
                                        "description": "1-based starting line number"
                                    },
                                    "end": {
                                        "type": "integer",
                                        "description": "1-based ending line number"
                                    }
                                },
                                "required": ["path", "start", "end"]
                            }
                        },
                        {
                            "name": "search",
                            "description": "Semantic code search across the project using token and vector similarity. Best for conceptual or keyword queries when symbol names are unknown.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": {
                                        "type": "string",
                                        "description": "Search query or concept description"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum number of results to return (default: 3)"
                                    }
                                },
                                "required": ["query"]
                            }
                        },
                        {
                            "name": "get_symbol_body",
                            "description": "PREFERRED over reading full files. Retrieves ONLY the exact implementation body of a specific function, method, or class with exact line numbers, saving context window tokens.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Symbol name (e.g., 'on_open_file_clicked', 'InsertClipCommand')"
                                    },
                                    "file": {
                                        "type": "string",
                                        "description": "Optional file path hint to disambiguate identical symbol names"
                                    },
                                    "max_lines": {
                                        "type": "integer",
                                        "description": "Optional maximum lines to return before truncation (default: 400)"
                                    }
                                },
                                "required": ["name"]
                            }
                        },
                        {
                            "name": "get_impact_radius",
                            "description": "Comprehensive blast-radius analysis. Returns callers (capped at 10, with total_callers count) and callees (what functions it calls) in one compact response (~300-500 tokens). Use before refactoring, editing, or renaming any symbol.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "Symbol, function, or class name to analyze impact for"
                                    },
                                    "file": {
                                        "type": "string",
                                        "description": "Optional file path hint"
                                    }
                                },
                                "required": ["symbol"]
                            }
                        },
                        {
                            "name": "get_tree",
                            "description": "Returns a compact directory overview with symbol counts. In large repositories or monorepos, specify 'path' or 'max_depth' to inspect specific subdirectories and avoid context bloat.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Optional relative subdirectory path to inspect (e.g., 'crates/tauri' or 'src')"
                                    },
                                    "max_depth": {
                                        "type": "integer",
                                        "description": "Optional maximum depth of folder hierarchy (default: 2)"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Optional maximum files to return (default: 50)"
                                    }
                                }
                            }
                        },
                        {
                            "name": "scaffold_template",
                            "description": "Scaffold a verified, minimal project starter in an empty destination directory. Guarantees zero overwrites and immediately synchronizes the workspace graph. Supported templates: 'rust-cli'.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "template": {
                                        "type": "string",
                                        "description": "Template name to scaffold. Supported: 'rust-cli'"
                                    },
                                    "project_name": {
                                        "type": "string",
                                        "description": "Name of the new project (must be valid identifier)"
                                    },
                                    "destination": {
                                        "type": "string",
                                        "description": "Optional relative destination directory path (defaults to current workspace root)"
                                    }
                                },
                                "required": ["template", "project_name"]
                            }
                        },
                        {
                            "name": "prepare_task_context",
                            "description": "Compile a task-specific, token-bounded context packet containing relevant interfaces, types, errors, imports, and provenance. Replaces exploratory multi-tool loops with a single ranked, budgeted context delivery.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "task": {
                                        "type": "string",
                                        "description": "Description of the module or feature to implement, e.g. 'implement postgres user repository with error handling'"
                                    },
                                    "target_path": {
                                        "type": "string",
                                        "description": "Optional relative file path being written or edited"
                                    },
                                    "max_tokens": {
                                        "type": "integer",
                                        "description": "Optional token ceiling for the context packet (default: 1200)"
                                    }
                                },
                                "required": ["task"]
                            }
                        },
                        {
                            "name": "set_workspace",
                            "description": "Dynamically switch Layer's active workspace root to a different project directory. Clears the previous index and immediately indexes the new project. Use this whenever switching repositories or when working in multi-repo workflows.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative or absolute path to the target project directory (e.g. '.' or '/path/to/project')"
                                    }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "verify_project",
                            "description": "PREFERRED over terminal cargo check or tsc. Runs native compiler verification with structured compact diagnostics and error lifecycle tracking (new, persistent, resolved). Pass force_compiler: true to bypass AST pre-check short-circuiting.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "force_compiler": {
                                        "type": "boolean",
                                        "description": "Optional: Force running full compiler subprocess (tsc/cargo) even if fast AST pre-check passes or fails (default: false)"
                                    }
                                }
                            }
                        },
                        {
                            "name": "bootstrap_project",
                            "description": "Bootstrap a new project using standard non-interactive ecosystem CLI generators (Vite React-TS, Cargo binary/lib, Python, Go, C++, Swift, Kotlin, C#, Java). Guarantees zero overwrites, non-interactive execution with timeout, auto-switches workspace root, and immediately indexes the project. Supported presets: 'react-ts', 'rust-bin', 'rust-lib', 'python', 'go', 'cpp', 'swift', 'kotlin', 'csharp', 'java'.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "preset": {
                                        "type": "string",
                                        "description": "Ecosystem preset: 'react-ts', 'rust-bin', 'rust-lib', 'python', 'go', 'cpp', 'swift', 'kotlin', 'csharp', 'java'"
                                    },
                                    "project_name": {
                                        "type": "string",
                                        "description": "Project name (valid identifier)"
                                    },
                                    "destination": {
                                        "type": "string",
                                        "description": "Optional destination parent directory (defaults to current workspace root)"
                                    },
                                    "packages": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Optional initial packages to install immediately using fast non-blocking flags (e.g. ['three', '@react-three/fiber', '@react-three/drei'])"
                                    }
                                }
                            },
                            "required": ["preset", "project_name"]
                        },
                        {
                            "name": "inspect_package_api",
                            "description": "CRITICAL: Call this BEFORE writing code that imports external packages (e.g. Three.js, React Three Fiber, Lucide, Tailwind, Axum). Extracts exact TypeScript declarations (.d.ts) or Rust signatures directly from node_modules or Cargo dependencies to eliminate API hallucinations.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "package_name": {
                                        "type": "string",
                                        "description": "Name of the dependency package (e.g., '@react-three/fiber', 'three', 'zod')"
                                    },
                                    "symbol_name": {
                                        "type": "string",
                                        "description": "Optional exact symbol name to extract declaration and doc comments for"
                                    },
                                    "query": {
                                        "type": "string",
                                        "description": "Optional search term to filter exported package symbols"
                                    }
                                },
                                "required": ["package_name"]
                            }
                        },
                        {
                            "name": "write_file",
                            "description": "PREFERRED over native Write tool. Writes full file content with automatic pre-edit snapshotting, instant AST pre-check (<5ms), graph synchronization, and cascading circuit breaker. Supports verify_mode ('fast', 'full', 'none'). In multi-file refactoring, preserves valid interface edits and returns pending_downstream_updates checklist instead of rolling back.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path to the file to create or overwrite"
                                    },
                                    "content": {
                                        "type": "string",
                                        "description": "Full file content to write"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Optional verification tier: 'fast' (<5ms in-memory AST syntax and local import check, zero subprocesses), 'full' (AST pre-check + compiler subprocess), 'none' (skip verification). Default is 'fast'."
                                    },
                                    "auto_verify": {
                                        "type": "boolean",
                                        "description": "Optional: (Deprecated alias) false maps to verify_mode='none', true maps to 'fast'"
                                    },
                                    "auto_rollback_on_cascade": {
                                        "type": "boolean",
                                        "description": "Optional: Whether to automatically revert if edit causes cascading compilation breakage (default: true)"
                                    }
                                },
                                "required": ["path", "content"]
                            }
                        },
                        {
                            "name": "edit_file",
                            "description": "PREFERRED over native Edit tool. Surgically replaces a target snippet of code. Features automatic snapshotting, sub-5ms AST pre-check, and rollback protection for broken syntax.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path to the file to edit"
                                    },
                                    "target": {
                                        "type": "string",
                                        "description": "Exact text snippet to replace (must match uniquely)"
                                    },
                                    "replacement": {
                                        "type": "string",
                                        "description": "New replacement code"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Optional verification tier: 'fast' (<5ms in-memory AST syntax and local import check, zero subprocesses), 'full' (AST pre-check + compiler subprocess), 'none' (skip verification). Default is 'fast'."
                                    },
                                    "auto_verify": {
                                        "type": "boolean",
                                        "description": "Optional: (Deprecated alias) false maps to verify_mode='none', true maps to 'fast'"
                                    },
                                    "auto_rollback_on_cascade": {
                                        "type": "boolean",
                                        "description": "Optional: Whether to automatically revert if edit causes cascading compilation breakage (default: true)"
                                    }
                                },
                                "required": ["path", "target", "replacement"]
                            }
                        },
                        {
                            "name": "rollback_last_edit",
                            "description": "Instantly reverts the most recent write or edit to its pre-mutation snapshot and synchronizes the workspace graph in 1 tool turn.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "scaffold_module",
                            "description": "PREFERRED for creating new modules or feature architectures. Declaratively scaffolds multiple TypeScript/Rust files with imports, interface declarations, and stubbed functions on Turn 1 with zero subprocess overhead and instant graph indexing.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "module_name": {
                                        "type": "string",
                                        "description": "Logical module or feature name (e.g. 'physics', 'auth')"
                                    },
                                    "files": {
                                        "type": "array",
                                        "description": "List of files to scaffold with their imports, types, and functions",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "path": { "type": "string", "description": "Relative file path" },
                                                "imports": { "type": "array", "items": { "type": "string" } },
                                                "types": {
                                                    "type": "array",
                                                    "items": {
                                                        "type": "object",
                                                        "properties": {
                                                            "name": { "type": "string" },
                                                            "kind": { "type": "string" },
                                                            "definition": { "type": "string" }
                                                        },
                                                        "required": ["name", "kind", "definition"]
                                                    }
                                                },
                                                "functions": {
                                                    "type": "array",
                                                    "items": {
                                                        "type": "object",
                                                        "properties": {
                                                            "name": { "type": "string" },
                                                            "signature": { "type": "string" },
                                                            "stub_body": { "type": "string" },
                                                            "doc_comment": { "type": "string" }
                                                        },
                                                        "required": ["name", "signature"]
                                                    }
                                                },
                                                "raw_content": { "type": "string" }
                                            },
                                            "required": ["path"]
                                        }
                                    }
                                },
                                "required": ["module_name", "files"]
                            }
                        },
                        {
                            "name": "rename_symbol",
                            "description": "Atomically rename a function, struct, interface, class, or variable across its declaration file and all dependent files in a single turn. Replaces exact AST code identifier occurrences while preserving strings and comments, with automatic rollback protection on syntax errors.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path to file where the symbol is declared or used"
                                    },
                                    "old_name": {
                                        "type": "string",
                                        "description": "Current identifier name"
                                    },
                                    "new_name": {
                                        "type": "string",
                                        "description": "New identifier name (valid identifier)"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Verification mode after rename: 'fast' (sub-15ms AST check), 'full' (compiler check), or 'none'. Defaults to 'fast'."
                                    }
                                },
                                "required": ["path", "old_name", "new_name"]
                            }
                        },
                        {
                            "name": "verify_impacted_tests",
                            "description": "Selectively discover and run only the test files impacted by modified code via VectorGraph dependency tracing and companion test patterns, skipping unaffected test suites to save tokens and time.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "changed_files": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Optional list of changed relative file paths. If omitted, uses the most recent mutation snapshot or all indexed files."
                                    }
                                }
                            }
                        },
                        {
                            "name": "get_session_stats",
                            "description": "Retrieve Layer v1.1 ROI and session telemetry statistics, including total tool calls, surgical AST reads, estimated tokens saved, LLM turns saved, and a terminal dashboard summary.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "reset": {
                                        "type": "boolean",
                                        "description": "Optionally reset session statistics to zero."
                                    }
                                }
                            }
                        },
                        {
                            "name": "init_harness",
                            "description": "Initialize agent steering directives (AGENTS.md, CLAUDE.md, .cursorrules) for AI coding agents (Codex, Claude Code, Cursor, OpenCode, Windsurf) so they automatically prioritize Layer MCP tools.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "harness": {
                                        "type": "string",
                                        "description": "Target harness name: 'codex', 'claude', 'cursor', 'opencode', 'windsurf', or 'all'. Defaults to 'all'."
                                    },
                                    "dest": {
                                        "type": "string",
                                        "description": "Optional directory path to initialize. Defaults to current workspace root."
                                    },
                                    "global": {
                                        "type": "boolean",
                                        "description": "Optionally verify and update global harness MCP configs (~/.codex/config.toml, etc.)."
                                    }
                                }
                            }
                        }
                    ]
                }
            });
            send_json_line(writer, &resp)?;
        }
        "tools/call" => {
            let tool_name = params
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let arguments = params.and_then(|p| p.get("arguments"));

            let start_time = std::time::Instant::now();
            let mut g = match graph.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };

            let query_key = format!(
                "{}:{}",
                tool_name,
                arguments.unwrap_or(&serde_json::Value::Null)
            );
            let is_duplicate = recent_queries.contains(&query_key);
            if recent_queries.len() >= 50 {
                recent_queries.remove(0);
            }
            recent_queries.push(query_key);

            let tool_name_owned = tool_name.to_string();
            let args_cloned = arguments.cloned();

            let exec_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_tool(
                    &mut g,
                    root,
                    &tool_name_owned,
                    args_cloned.as_ref(),
                    pkg_indexer,
                )
            }));

            let mut content_text = match exec_res {
                Ok(res) => res,
                Err(err) => {
                    let panic_str = if let Some(s) = err.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = err.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic in tool execution".to_string()
                    };
                    eprintln!(
                        "[layer-mcp] FATAL: Caught panic in tool '{}': {}",
                        tool_name_owned, panic_str
                    );
                    format!(
                        "Error: Internal panic in tool '{}': {}. Workspace preserved.",
                        tool_name_owned, panic_str
                    )
                }
            };
            if is_duplicate
                && !content_text.starts_with("Error:")
                && let Ok(mut parsed) = serde_json::from_str::<serde_json::Value>(&content_text)
                && let Some(obj) = parsed.as_object_mut()
            {
                obj.insert(
                    "_retrieval_notice".to_string(),
                    json!("Advisory: Identical query was already executed earlier in this session. Avoid repeated queries with identical arguments to preserve context."),
                );
                content_text = serde_json::to_string(&parsed).unwrap_or(content_text);
            }
            let duration = start_time.elapsed();
            let byte_count = content_text.len();
            let approx_tokens = byte_count.div_ceil(4);

            let log_line = format!(
                "[layer-mcp] tool_call: '{}' completed in {:?} | size: {} bytes (~{} tokens)\n",
                tool_name, duration, byte_count, approx_tokens
            );
            eprint!("{}", log_line);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/layer_mcp.log")
            {
                let _ = f.write_all(log_line.as_bytes());
            }

            // Track ROI telemetry
            if tool_name != "get_session_stats" {
                let mut tracker = crate::telemetry::TelemetryTracker::load(root);
                let elapsed_ms = duration.as_millis();

                let (tokens_saved, turns_saved) = match tool_name {
                    "inspect_package_api" => {
                        if content_text.contains("\"unindexed_offline\"") {
                            (0, 0)
                        } else {
                            (2500, 1)
                        }
                    }
                    "init_harness" => (2000, 1),
                    "rename_symbol" => {
                        let k = if let Ok(parsed) =
                            serde_json::from_str::<serde_json::Value>(&content_text)
                        {
                            parsed
                                .get("files_modified")
                                .and_then(|f| f.as_array())
                                .map(|a| a.len())
                                .unwrap_or(1)
                        } else {
                            1
                        };
                        let turns = k.saturating_sub(1);
                        (turns * 3500, turns)
                    }
                    "verify_impacted_tests" => {
                        let skipped = if let Ok(parsed) =
                            serde_json::from_str::<serde_json::Value>(&content_text)
                        {
                            parsed
                                .get("skipped_test_files_count")
                                .and_then(|s| s.as_u64())
                                .unwrap_or(0) as usize
                        } else {
                            0
                        };
                        (skipped * 1200, 1)
                    }
                    "write_file" | "edit_file" => {
                        if content_text.contains("SYNTAX_ERROR")
                            || content_text.contains("syntax_error")
                        {
                            tracker.record_ast_catch();
                        }
                        if content_text.contains("\"rolled_back\": true") {
                            tracker.record_circuit_breaker_save();
                        }
                        (0, 0)
                    }
                    "get_file_skeleton"
                    | "find_symbol"
                    | "get_symbol_body"
                    | "get_callers"
                    | "find_references"
                    | "get_impact_radius"
                    | "get_lines"
                    | "get_tree"
                    | "search"
                    | "prepare_task_context" => (1200, 0),
                    _ => (0, 0),
                };

                tracker.record_tool_call(tool_name, elapsed_ms, tokens_saved, turns_saved);
                let _ = tracker.save(root);
            }

            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [
                        {
                            "type": "text",
                            "text": content_text
                        }
                    ]
                }
            });
            send_json_line(writer, &resp)?;
        }
        "shutdown" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null
            });
            send_json_line(writer, &resp)?;
        }
        "exit" => {
            std::process::exit(0);
        }
        _ => {
            if let Some(id_val) = id {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "error": {
                        "code": -32601,
                        "message": format!("Method '{}' not found", method)
                    }
                });
                send_json_line(writer, &err)?;
            }
        }
    }

    Ok(())
}

/// Given an absolute path to a file or directory, walk up parent directories to find a project root indicator
/// (.git, Cargo.toml, package.json, pyproject.toml, go.mod, etc.)
pub fn find_project_root_for_path(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };

    while current.parent().is_some() {
        if current.join("Cargo.toml").exists()
            || current.join("package.json").exists()
            || current.join("tsconfig.json").exists()
            || current.join("pyproject.toml").exists()
            || current.join("requirements.txt").exists()
            || current.join("go.mod").exists()
            || current.join("CMakeLists.txt").exists()
            || current.join("Package.swift").exists()
            || current.join("pom.xml").exists()
            || current.join("build.gradle.kts").exists()
            || current.join("build.gradle").exists()
            || current.join(".git").exists()
        {
            return Some(current);
        }
        current = match current.parent() {
            Some(p) => p.to_path_buf(),
            None => break,
        };
    }
    None
}

/// Canonicalizes a path from user/agent, resolves it against root, and ensures it is safely within root
pub fn resolve_safe_path(root: &Path, raw_path: &str) -> Result<PathBuf, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err("Error: Path cannot be empty.".to_string());
    }

    let canonical_root = match root.canonicalize() {
        Ok(cr) => cr,
        Err(_) => root.to_path_buf(),
    };

    let effective_root_name = canonical_root
        .file_name()
        .or_else(|| root.file_name())
        .and_then(|n| n.to_str());

    let mut normalized_raw = trimmed.replace('\\', "/");
    while let Some(stripped) = normalized_raw.strip_prefix("./") {
        normalized_raw = stripped.to_string();
    }

    if let Some(root_name) = effective_root_name {
        let nested_dir = canonical_root.join(root_name);
        if !nested_dir.is_dir() {
            if normalized_raw == root_name {
                normalized_raw = ".".to_string();
            } else if let Some(stripped) = normalized_raw.strip_prefix(&format!("{}/", root_name)) {
                normalized_raw = stripped.to_string();
            }
        }
    }

    let input_path = Path::new(&normalized_raw);
    let candidate = if input_path.is_absolute() {
        if let Ok(rel) = input_path
            .strip_prefix(&canonical_root)
            .or_else(|_| input_path.strip_prefix(root))
        {
            if let Some(root_name) = effective_root_name {
                if !canonical_root.join(root_name).is_dir() {
                    if let Ok(stripped_rel) = rel.strip_prefix(root_name) {
                        canonical_root.join(stripped_rel)
                    } else {
                        canonical_root.join(rel)
                    }
                } else {
                    canonical_root.join(rel)
                }
            } else {
                canonical_root.join(rel)
            }
        } else {
            input_path.to_path_buf()
        }
    } else {
        canonical_root.join(input_path)
    };

    let canonical_target = match candidate.canonicalize() {
        Ok(ct) => ct,
        Err(_) => {
            // Path doesn't exist on disk yet, normalize components
            use std::path::Component;
            let mut normalized = PathBuf::new();
            for comp in candidate.components() {
                match comp {
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    Component::Normal(c) => normalized.push(c),
                    Component::RootDir => normalized.push("/"),
                    _ => {}
                }
            }
            normalized
        }
    };

    if !canonical_target.starts_with(&canonical_root) {
        return Err(format!(
            "Error: Access denied. Path '{}' is outside workspace root.",
            raw_path
        ));
    }

    // Convert to relative path from workspace root
    match canonical_target.strip_prefix(&canonical_root) {
        Ok(rel) => Ok(rel.to_path_buf()),
        Err(_) => Err("Error: Failed to relativize path to workspace root.".to_string()),
    }
}

fn maybe_switch_workspace_root(
    root: &mut PathBuf,
    g: &mut VectorGraph,
    candidate_path: Option<&str>,
    mut pkg_indexer: Option<&mut crate::package_indexer::PackageIndexer>,
) {
    let raw_p = match candidate_path {
        Some(p) => p,
        None => return,
    };
    let p = Path::new(raw_p);
    let abs_p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    if !abs_p.exists() {
        return;
    }
    let canon_target = match abs_p.canonicalize() {
        Ok(c) => c,
        Err(_) => return,
    };
    if canon_target == *root {
        return;
    }
    if let Some(new_proj_root) = find_project_root_for_path(&canon_target) {
        let canon_new = new_proj_root.canonicalize().unwrap_or(new_proj_root);
        if canon_new != *root {
            let switch_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                eprintln!(
                    "[layer-mcp] Auto-switching workspace root to: {}",
                    canon_new.display()
                );
                *root = canon_new.clone();
                g.clear();
                if let Some(ref mut idx) = pkg_indexer {
                    idx.clear();
                }
                g.sync_workspace(&canon_new);
            }));
            if let Err(err) = switch_res {
                eprintln!(
                    "[layer-mcp] Warning: Panic while auto-switching workspace: {:?}",
                    err
                );
            }
        }
    }
}

pub fn execute_tool(
    g: &mut VectorGraph,
    root: &mut PathBuf,
    tool_name: &str,
    args: Option<&serde_json::Value>,
    pkg_indexer: &mut crate::package_indexer::PackageIndexer,
) -> String {
    let empty_args = json!({});
    let args = args.unwrap_or(&empty_args);

    if tool_name == "set_workspace" {
        let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        if path_str.trim().is_empty() {
            return "Error: Path cannot be empty.".to_string();
        }
        let target_path = Path::new(path_str);
        let target_abs = if target_path.is_absolute() {
            target_path.to_path_buf()
        } else {
            root.join(target_path)
        };

        let canonical = match target_abs.canonicalize() {
            Ok(c) => c,
            Err(e) => return format!("Error: Cannot resolve path '{}': {}", path_str, e),
        };

        if !canonical.is_dir() {
            return format!("Error: Target path '{}' is not a directory.", path_str);
        }

        let switch_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eprintln!("[layer-mcp] Workspace switched to: {}", canonical.display());
            *root = canonical.clone();
            g.clear();
            pkg_indexer.clear();
            g.sync_workspace(&canonical);
        }));

        if let Err(err) = switch_res {
            let msg = if let Some(s) = err.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = err.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic during workspace sync".to_string()
            };
            return format!(
                "Error: Panic while switching workspace to '{}': {}",
                path_str, msg
            );
        }

        let res = json!({
            "success": true,
            "workspace_root": canonical.display().to_string(),
            "indexed_files": g.files.len(),
            "indexed_chunks": g.chunks.len(),
            "message": format!("Workspace successfully switched to '{}'. Indexed {} files.", canonical.display(), g.files.len())
        });
        return serde_json::to_string_pretty(&res).unwrap_or_default();
    }

    // Auto-detect project root if agent supplied a path pointing to a different project or nested directory
    let candidate_path = args
        .get("path")
        .or_else(|| args.get("file"))
        .or_else(|| args.get("target_path"))
        .and_then(|v| v.as_str());

    maybe_switch_workspace_root(root, g, candidate_path, Some(pkg_indexer));

    // If current root has no recognized build config, but an immediate subdirectory does,
    // automatically switch the active workspace root to that subdirectory!
    if crate::verifier::ProjectType::detect(root) == crate::verifier::ProjectType::Unknown
        && let Some(nested_root) = crate::verifier::detect_nested_project_root(root)
    {
        let canon = nested_root.canonicalize().unwrap_or(nested_root);
        if canon.is_dir() && canon != *root {
            let switch_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                eprintln!(
                    "[layer-mcp] Auto-detected nested project manifest at: {}. Switching active workspace.",
                    canon.display()
                );
                *root = canon.clone();
                g.clear();
                pkg_indexer.clear();
                g.sync_workspace(&canon);
            }));
            if let Err(err) = switch_res {
                eprintln!(
                    "[layer-mcp] Warning: Panic while switching to nested project root: {:?}",
                    err
                );
            }
        }
    }

    // Synchronously sync workspace with disk to catch newly created/modified/deleted files
    g.sync_workspace(root);

    // If an explicit path or file was supplied, check that specific file path immediately
    if let Some(p) = args.get("path").and_then(|v| v.as_str())
        && let Ok(safe_rel) = resolve_safe_path(root, p)
    {
        g.refresh_path_if_modified(root, &safe_rel);
    }
    if let Some(f) = args.get("file").and_then(|v| v.as_str())
        && let Ok(safe_rel) = resolve_safe_path(root, f)
    {
        g.refresh_path_if_modified(root, &safe_rel);
    }

    match tool_name {
        "get_file_skeleton" => {
            let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let safe_rel = match resolve_safe_path(root, path_str) {
                Ok(p) => p,
                Err(err_msg) => return err_msg,
            };

            // Block sensitive files (.env, credentials)
            let file_name = safe_rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if file_name.to_lowercase().starts_with(".env")
                || file_name == ".git"
                || file_name.contains("id_rsa")
                || file_name.ends_with(".pem")
            {
                return format!("Error: Access denied. File '{}' is restricted.", path_str);
            }

            // Strictly serve only files indexed or within workspace
            let path_match = g
                .find_file_match(&safe_rel.to_string_lossy())
                .unwrap_or(safe_rel);
            match g.get_file_skeleton(&path_match) {
                Some(skeleton) => serde_json::to_string(&skeleton).unwrap_or_default(),
                None => format!("Error: File '{}' not found in workspace index.", path_str),
            }
        }
        "find_symbol" => {
            let name = args.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let file_hint = args.get("file").and_then(|f| f.as_str()).map(Path::new);
            let limit = args
                .get("limit")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize)
                .unwrap_or(15);
            let all_hits = g.find_symbol_filtered(name, file_hint, None);
            if all_hits.is_empty() {
                format!("Symbol '{}' not found in codebase.", name)
            } else {
                let total_found = all_hits.len();
                let is_truncated = total_found > limit;
                let list: Vec<_> = all_hits
                    .into_iter()
                    .take(limit)
                    .map(|(p, s)| {
                        json!({
                            "file": p.display().to_string(),
                            "name": s.name,
                            "kind": s.kind.to_string(),
                            "lines": [s.start_line, s.end_line],
                            "signature": s.signature,
                            "doc": s.doc_comment
                        })
                    })
                    .collect();
                if is_truncated {
                    json!({
                        "symbol": name,
                        "total_found": total_found,
                        "returned": list.len(),
                        "is_truncated": true,
                        "omitted_count": total_found.saturating_sub(list.len()),
                        "next_query_hint": format!("Generic symbol with {} declarations. Narrow down with file='<crate_or_path>' (e.g. file='crates/tauri') or pass limit={}", total_found, total_found),
                        "matches": list,
                    })
                    .to_string()
                } else {
                    serde_json::to_string(&list).unwrap_or_default()
                }
            }
        }
        "find_references" => {
            let symbol = args.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let code_only = args
                .get("code_only")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            let limit = args
                .get("limit")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize)
                .unwrap_or(60);
            let mut refs = g.find_references_filtered(symbol, code_only);
            if refs.is_empty() {
                format!("No references found for symbol '{}'.", symbol)
            } else {
                let total_refs = refs.len();
                let is_truncated = total_refs > limit;
                if is_truncated {
                    refs.truncate(limit);
                    json!({
                        "symbol": symbol,
                        "total_found": total_refs,
                        "returned": refs.len(),
                        "is_truncated": true,
                        "omitted_count": total_refs.saturating_sub(refs.len()),
                        "next_query_hint": format!("To view all {} references, increase limit={} or set code_only=true", total_refs, total_refs),
                        "references": refs,
                    })
                    .to_string()
                } else {
                    serde_json::to_string(&refs).unwrap_or_default()
                }
            }
        }
        "get_callers" => {
            let symbol = args.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let limit = args
                .get("limit")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize)
                .unwrap_or(25);
            let callers = g.get_callers(symbol);
            if callers.is_empty() {
                serde_json::to_string(&json!({
                    "symbol": symbol,
                    "callers": [],
                    "status": "zero_internal_callers",
                    "confidence": "heuristic_lexical",
                    "note": "Zero direct callers found in workspace index. Do not assume symbol is completely unused if it is an exported API, virtual method, macro target, or called dynamically."
                })).unwrap_or_default()
            } else {
                let total = callers.len();
                let is_truncated = total > limit;
                let list: Vec<_> = callers.into_iter().take(limit).collect();
                if is_truncated {
                    json!({
                        "symbol": symbol,
                        "total_distinct_callers": total,
                        "returned": list.len(),
                        "is_truncated": true,
                        "omitted_callers_count": total.saturating_sub(list.len()),
                        "next_query_hint": format!("To view all {} callers without omission, pass limit={} or use find_references(symbol='{}', code_only=true)", total, total, symbol),
                        "callers": list,
                    })
                    .to_string()
                } else {
                    serde_json::to_string(&list).unwrap_or_default()
                }
            }
        }
        "get_lines" => {
            let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let start = args.get("start").and_then(|s| s.as_u64()).unwrap_or(1) as usize;
            let end = args
                .get("end")
                .and_then(|e| e.as_u64())
                .unwrap_or(start as u64) as usize;

            let safe_rel = match resolve_safe_path(root, path_str) {
                Ok(p) => p,
                Err(err_msg) => return err_msg,
            };

            // Strictly serve only files that exist in the workspace index (which respects .gitignore and file filters)
            let path_match = match g.find_file_match(&safe_rel.to_string_lossy()) {
                Some(p) => p,
                None => return format!("Error: File '{}' not found in workspace index.", path_str),
            };

            // Block sensitive files (.env*, credentials, keys, certificates)
            let file_name = path_match
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let name_lower = file_name.to_lowercase();
            if name_lower.contains(".env")
                || name_lower == ".git"
                || name_lower.contains("id_rsa")
                || name_lower.ends_with(".pem")
                || name_lower.ends_with(".key")
            {
                return format!("Error: Access denied. File '{}' is restricted.", path_str);
            }

            match g.get_file_lines(&path_match, start, end) {
                Some(content) => content,
                None => format!("Error: File '{}' not found in workspace index.", path_str),
            }
        }
        "search" => {
            let query = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(3) as usize;
            let hits = g.search(query, limit);
            if hits.is_empty() {
                format!("No results for query '{}'.", query)
            } else {
                let list: Vec<_> = hits
                    .iter()
                    .map(|h| {
                        // Limit snippet to at most 8 lines and 400 characters to keep search lean (< 600 tokens total)
                        let snippet_lines: Vec<&str> = h.snippet.lines().take(8).collect();
                        let mut compact_snippet = snippet_lines.join("\n");
                        if compact_snippet.len() > 400 {
                            compact_snippet.truncate(400);
                            compact_snippet.push_str("... [truncated]");
                        }
                        json!({
                            "file": h.file_path.display().to_string(),
                            "score": (h.score * 1000.0).round() / 1000.0,
                            "symbol": h.symbol_name,
                            "lines": [h.start_line, h.end_line],
                            "snippet": compact_snippet
                        })
                    })
                    .collect();
                serde_json::to_string(&list).unwrap_or_default()
            }
        }
        "get_symbol_body" => {
            let name = args.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let file_hint = args
                .get("file")
                .and_then(|f| f.as_str())
                .and_then(|f_str| resolve_safe_path(root, f_str).ok());
            let max_lines = args
                .get("max_lines")
                .and_then(|m| m.as_u64())
                .map(|m| m as usize)
                .unwrap_or(400);
            match g.get_symbol_body_opt(name, file_hint.as_deref(), max_lines) {
                Some(body) => serde_json::to_string(&body).unwrap_or_default(),
                None => format!("Symbol '{}' not found in codebase.", name),
            }
        }
        "get_impact_radius" => {
            let symbol = args.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let file_hint = args
                .get("file")
                .and_then(|f| f.as_str())
                .and_then(|f_str| resolve_safe_path(root, f_str).ok());
            let impact = g.get_impact_radius(symbol, file_hint.as_deref());
            serde_json::to_string(&impact).unwrap_or_default()
        }
        "get_tree" => {
            let path_str = args.get("path").and_then(|p| p.as_str());
            let max_depth = args
                .get("max_depth")
                .and_then(|d| d.as_u64())
                .map(|d| d as usize);
            let limit = args
                .get("limit")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize);
            let path_buf = path_str.map(PathBuf::from);
            g.format_compact_tree(path_buf.as_deref(), max_depth, limit)
        }
        "scaffold_template" => {
            let template_str = args.get("template").and_then(|t| t.as_str()).unwrap_or("");
            let project_name = args
                .get("project_name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let dest_str = args
                .get("destination")
                .and_then(|d| d.as_str())
                .unwrap_or("");

            let supported = match crate::scaffold::SupportedTemplate::parse(template_str) {
                Some(t) => t,
                None => {
                    return format!(
                        "Error: Unsupported template '{}'. Supported templates: 'rust-cli'",
                        template_str
                    );
                }
            };

            let dest_path = if dest_str.is_empty() || dest_str == "." {
                root.to_path_buf()
            } else {
                match resolve_safe_path(root, dest_str) {
                    Ok(p) => root.join(p),
                    Err(e) => return e,
                }
            };

            match crate::scaffold::scaffold_project(supported, project_name, &dest_path, g) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string()),
                Err(err) => format!("Error: {}", err),
            }
        }
        "prepare_task_context" => {
            let task = args.get("task").and_then(|t| t.as_str()).unwrap_or("");
            let target_path_str = args.get("target_path").and_then(|p| p.as_str());
            let max_tokens = args
                .get("max_tokens")
                .and_then(|m| m.as_u64())
                .map(|m| m as usize)
                .unwrap_or(1200);

            let target_buf = target_path_str.and_then(|p| resolve_safe_path(root, p).ok());

            let packet = crate::context_compiler::ContextCompiler::compile_task_context(
                g,
                task,
                target_buf.as_deref(),
                max_tokens,
            );
            serde_json::to_string_pretty(&packet).unwrap_or_default()
        }
        "verify_project" => {
            let force_compiler = args
                .get("force_compiler")
                .and_then(|f| f.as_bool())
                .unwrap_or(false);
            let report = crate::verifier::Verifier::verify_workspace_with_options(
                root,
                g,
                Some(pkg_indexer),
                force_compiler,
            );
            g.last_verification = Some(report.diagnostics.clone());
            serde_json::to_string_pretty(&report).unwrap_or_default()
        }
        "inspect_package_api" => {
            let pkg_name = args
                .get("package_name")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let sym_name = args.get("symbol_name").and_then(|s| s.as_str());
            let query = args.get("query").and_then(|q| q.as_str());

            match pkg_indexer.inspect_package(root, pkg_name, sym_name, query) {
                Ok(summary) => serde_json::to_string_pretty(&summary).unwrap_or_default(),
                Err(e) => {
                    let unindexed = serde_json::json!({
                        "status": "unindexed_offline",
                        "package": pkg_name,
                        "advisory": format!("Package types for '{}' are unindexed or offline. Proceed directly using Layer mutation tools. Detail: {}", pkg_name, e)
                    });
                    serde_json::to_string_pretty(&unindexed).unwrap_or_default()
                }
            }
        }
        "write_file" => {
            let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let verify_mode_str = args.get("verify_mode").and_then(|m| m.as_str());
            let auto_verify = args.get("auto_verify").and_then(|v| v.as_bool());
            let verify_mode = crate::mutation::VerifyMode::parse(verify_mode_str, auto_verify);
            let auto_rollback = args
                .get("auto_rollback_on_cascade")
                .and_then(|r| r.as_bool())
                .unwrap_or(true);

            let safe_rel = match resolve_safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return e,
            };

            match crate::mutation::MutationManager::write_file(
                root,
                &safe_rel,
                content,
                verify_mode,
                auto_rollback,
                g,
                Some(pkg_indexer),
            ) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "edit_file" => {
            let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let target = args.get("target").and_then(|t| t.as_str()).unwrap_or("");
            let replacement = args
                .get("replacement")
                .and_then(|r| r.as_str())
                .unwrap_or("");
            let verify_mode_str = args.get("verify_mode").and_then(|m| m.as_str());
            let auto_verify = args.get("auto_verify").and_then(|v| v.as_bool());
            let verify_mode = crate::mutation::VerifyMode::parse(verify_mode_str, auto_verify);
            let auto_rollback = args
                .get("auto_rollback_on_cascade")
                .and_then(|r| r.as_bool())
                .unwrap_or(true);

            let safe_rel = match resolve_safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return e,
            };

            match crate::mutation::MutationManager::edit_file(
                root,
                &safe_rel,
                target,
                replacement,
                verify_mode,
                auto_rollback,
                g,
                Some(pkg_indexer),
            ) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "rollback_last_edit" => {
            match crate::mutation::MutationManager::rollback_last_edit(root, g) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "scaffold_module" => {
            let req: crate::scaffold::ScaffoldModuleRequest =
                match serde_json::from_value(args.clone()) {
                    Ok(r) => r,
                    Err(e) => return format!("Error parsing scaffold_module arguments: {}", e),
                };

            match crate::scaffold::scaffold_module(root, req, g) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "bootstrap_project" => {
            let preset_str = args.get("preset").and_then(|p| p.as_str()).unwrap_or("");
            let project_name = args
                .get("project_name")
                .and_then(|n| n.as_str())
                .unwrap_or("");
            let dest_parent_str = args
                .get("destination")
                .and_then(|d| d.as_str())
                .unwrap_or(".");

            let preset = crate::scaffold::GeneratorPreset::parse_or_custom(preset_str);

            let dest_parent = match resolve_safe_path(root, dest_parent_str) {
                Ok(p) => root.join(p),
                Err(e) => return format!("Error: Invalid destination: {}", e),
            };

            let packages: Option<Vec<String>> =
                args.get("packages").and_then(|p| p.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                });

            match crate::scaffold::bootstrap_project(
                preset,
                project_name,
                &dest_parent,
                packages,
                g,
            ) {
                Ok(res) => {
                    let target_path = dest_parent.join(project_name.trim());
                    let new_root = target_path.canonicalize().unwrap_or(target_path);
                    if new_root.is_dir() {
                        let switch_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                            || {
                                eprintln!(
                                    "[layer-mcp] Auto-switching workspace root to bootstrapped project: {}",
                                    new_root.display()
                                );
                                *root = new_root.clone();
                                g.clear();
                                pkg_indexer.clear();
                                g.sync_workspace(&new_root);
                            },
                        ));
                        if let Err(err) = switch_res {
                            eprintln!(
                                "[layer-mcp] Warning: Panic while indexing bootstrapped project: {:?}",
                                err
                            );
                        }
                    }
                    serde_json::to_string_pretty(&res).unwrap_or_default()
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "rename_symbol" => {
            let path_str = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            let old_name = args.get("old_name").and_then(|n| n.as_str()).unwrap_or("");
            let new_name = args.get("new_name").and_then(|n| n.as_str()).unwrap_or("");
            let verify_mode_str = args.get("verify_mode").and_then(|m| m.as_str());
            let verify_mode = crate::mutation::VerifyMode::parse(verify_mode_str, None);

            let rel_path = match resolve_safe_path(root, path_str) {
                Ok(p) => p,
                Err(e) => return format!("Error: Invalid path: {}", e),
            };

            match crate::mutation::MutationManager::rename_symbol(
                root,
                &rel_path,
                old_name,
                new_name,
                verify_mode,
                g,
                Some(pkg_indexer),
            ) {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "verify_impacted_tests" => {
            let changed_files: Option<Vec<String>> = args
                .get("changed_files")
                .and_then(|cf| cf.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                });

            let res =
                crate::verifier::Verifier::verify_impacted_tests(root, changed_files.as_deref(), g);
            serde_json::to_string_pretty(&res).unwrap_or_default()
        }
        "get_session_stats" => {
            let reset = args.get("reset").and_then(|r| r.as_bool()).unwrap_or(false);

            let mut tracker = crate::telemetry::TelemetryTracker::load(root);
            if reset {
                let _ = tracker.reset(root);
            }
            let dashboard = tracker.format_dashboard();
            let res = json!({
                "total_tool_calls": tracker.total_tool_calls,
                "surgical_ast_reads": tracker.surgical_ast_reads,
                "estimated_tokens_saved": tracker.estimated_tokens_saved,
                "llm_turns_saved": tracker.llm_turns_saved,
                "ast_precheck_catches": tracker.ast_precheck_catches,
                "circuit_breaker_saves": tracker.circuit_breaker_saves,
                "package_apis_inspected": tracker.package_apis_inspected,
                "cross_file_renames": tracker.cross_file_renames,
                "selective_test_runs": tracker.selective_test_runs,
                "total_execution_ms": tracker.total_execution_ms,
                "dashboard": dashboard,
                "tool_breakdown": tracker.tool_breakdown,
            });
            serde_json::to_string_pretty(&res).unwrap_or_default()
        }
        "init_harness" => {
            let harness_str = args
                .get("harness")
                .and_then(|h| h.as_str())
                .unwrap_or("all");
            let harness_type: crate::harness::HarnessType = harness_str
                .parse()
                .unwrap_or(crate::harness::HarnessType::All);
            let check_global = args
                .get("global")
                .and_then(|g| g.as_bool())
                .unwrap_or(false);

            let target_dir = if let Some(dest_str) = args.get("dest").and_then(|d| d.as_str()) {
                let p = PathBuf::from(dest_str);
                if p.is_absolute() { p } else { root.join(p) }
            } else {
                root.clone()
            };

            let res = crate::harness::HarnessManager::init_workspace(
                &target_dir,
                harness_type,
                check_global,
            );
            serde_json::to_string_pretty(&res).unwrap_or_default()
        }
        _ => format!("Unknown tool: {}", tool_name),
    }
}

fn send_json_line(writer: &mut io::StdoutLock, val: &serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, val)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_resolve_safe_path_containment() {
        let temp_dir = std::env::temp_dir().join(format!("layer_sec_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).unwrap();
        let safe_subfile = temp_dir.join("safe.rs");
        fs::write(&safe_subfile, "pub fn foo() {}").unwrap();

        // 1. Valid relative path inside root
        let res = resolve_safe_path(&temp_dir, "safe.rs");
        assert!(res.is_ok(), "safe.rs should be allowed");
        assert_eq!(res.unwrap(), PathBuf::from("safe.rs"));

        // 2. Directory traversal attempt
        let res_traversal = resolve_safe_path(&temp_dir, "../../etc/passwd");
        assert!(res_traversal.is_err(), "Traversal outside root must fail");
        assert!(
            res_traversal
                .unwrap_err()
                .contains("outside workspace root")
        );

        // 3. Absolute path outside root
        let res_outside = resolve_safe_path(&temp_dir, "/etc/passwd");
        assert!(res_outside.is_err(), "External absolute path must fail");

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_find_project_root_for_path() {
        let temp_dir = std::env::temp_dir().join(format!("layer_root_test_{}", std::process::id()));
        let sub_src = temp_dir.join("src").join("nested");
        fs::create_dir_all(&sub_src).unwrap();
        fs::write(temp_dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        let file_path = sub_src.join("lib.rs");
        fs::write(&file_path, "pub fn test() {}").unwrap();

        let detected = find_project_root_for_path(&file_path);
        assert!(detected.is_some());
        assert_eq!(
            detected.unwrap().canonicalize().unwrap(),
            temp_dir.canonicalize().unwrap()
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_set_workspace_and_auto_switch() {
        let root_a = std::env::temp_dir().join(format!("layer_ws_a_{}", std::process::id()));
        let root_b = std::env::temp_dir().join(format!("layer_ws_b_{}", std::process::id()));
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();

        fs::write(root_a.join("Cargo.toml"), "[package]\nname = \"a\"").unwrap();
        fs::write(root_a.join("main.rs"), "fn main() { println!(\"A\"); }").unwrap();

        fs::write(root_b.join("Cargo.toml"), "[package]\nname = \"b\"").unwrap();
        fs::write(root_b.join("b.rs"), "pub fn b_func() {}").unwrap();

        let canon_a = root_a.canonicalize().unwrap();
        let canon_b = root_b.canonicalize().unwrap();

        let mut current_root = canon_a.clone();
        let mut graph = VectorGraph::new();
        graph.sync_workspace(&current_root);
        assert!(graph.files.contains_key(&PathBuf::from("main.rs")));

        let mut pkg_indexer = crate::package_indexer::PackageIndexer::new();

        // 1. Explicitly switch workspace to root_b
        let switch_args = json!({ "path": canon_b.to_str().unwrap() });
        let switch_res = execute_tool(
            &mut graph,
            &mut current_root,
            "set_workspace",
            Some(&switch_args),
            &mut pkg_indexer,
        );
        assert!(switch_res.contains("\"success\": true"));
        assert_eq!(current_root, canon_b);
        assert!(graph.files.contains_key(&PathBuf::from("b.rs")));
        assert!(!graph.files.contains_key(&PathBuf::from("main.rs")));

        // 2. Auto-switch when querying an absolute file in root_a
        let query_args = json!({
            "path": canon_a.join("main.rs").to_str().unwrap()
        });
        let _ = execute_tool(
            &mut graph,
            &mut current_root,
            "get_skeleton",
            Some(&query_args),
            &mut pkg_indexer,
        );
        assert_eq!(
            current_root, canon_a,
            "Should have auto-switched to project A"
        );
        assert!(graph.files.contains_key(&PathBuf::from("main.rs")));

        let _ = fs::remove_dir_all(&root_a);
        let _ = fs::remove_dir_all(&root_b);
    }
}
