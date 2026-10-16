use crate::graph::VectorGraph;
use serde_json::json;
use std::io::{self, BufRead, Read, Write};
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

        let json_str = if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len: usize = match trimmed
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse().ok())
            {
                Some(l) => l,
                None => {
                    eprintln!("[layer-mcp] Invalid Content-Length header: {}", trimmed);
                    continue;
                }
            };
            // Read headers until blank line
            let mut blank_found = false;
            while !blank_found {
                line.clear();
                if reader.read_line(&mut line)? == 0 {
                    return Ok(());
                }
                if line.trim().is_empty() {
                    blank_found = true;
                }
            }
            // Read exactly len bytes
            let mut body_buf = vec![0u8; len];
            reader.read_exact(&mut body_buf)?;
            match String::from_utf8(body_buf) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[layer-mcp] Non-utf8 body: {}", e);
                    continue;
                }
            }
        } else {
            trimmed.to_string()
        };

        match serde_json::from_str::<serde_json::Value>(&json_str) {
            Ok(serde_json::Value::Array(items)) => {
                for val in items {
                    if let Err(e) = dispatch_jsonrpc_value(
                        &graph,
                        &mut root,
                        val,
                        &mut writer,
                        &mut recent_queries,
                        &mut pkg_indexer,
                    ) {
                        eprintln!("[layer-mcp] RPC dispatch error: {}", e);
                        if e.kind() == io::ErrorKind::BrokenPipe {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(val) => {
                if let Err(e) = dispatch_jsonrpc_value(
                    &graph,
                    &mut root,
                    val,
                    &mut writer,
                    &mut recent_queries,
                    &mut pkg_indexer,
                ) {
                    eprintln!("[layer-mcp] RPC dispatch error: {}", e);
                    if e.kind() == io::ErrorKind::BrokenPipe {
                        return Ok(());
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "[layer-mcp] Warning: Received invalid JSON: {} ({})",
                    json_str, err
                );
            }
        }
    }

    Ok(())
}

fn dispatch_jsonrpc_value(
    graph: &Arc<Mutex<VectorGraph>>,
    root: &mut PathBuf,
    val: serde_json::Value,
    writer: &mut io::StdoutLock,
    recent_queries: &mut Vec<String>,
    pkg_indexer: &mut crate::package_indexer::PackageIndexer,
) -> io::Result<()> {
    if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
        let id = val.get("id").cloned();
        let params = val.get("params");
        let method_str = method.to_string();

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle_rpc_method(
                graph,
                root,
                &method_str,
                id.clone(),
                params,
                writer,
                recent_queries,
                pkg_indexer,
            )
        }));

        match res {
            Ok(io_res) => io_res,
            Err(panic_payload) => {
                let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "Unknown internal panic".to_string()
                };
                eprintln!(
                    "[layer-mcp] Caught panic during method '{}': {}",
                    method_str, panic_msg
                );
                if let Some(id_val) = id {
                    let err_resp = json!({
                        "jsonrpc": "2.0",
                        "id": id_val,
                        "error": {
                            "code": -32603,
                            "message": format!("Internal server error: {}", panic_msg)
                        }
                    });
                    send_json_line(writer, &err_resp)
                } else {
                    Ok(())
                }
            }
        }
    } else {
        Ok(())
    }
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
                        let graph_clone = Arc::clone(graph);
                        let root_clone = new_root.clone();
                        std::thread::spawn(move || {
                            let scanner = crate::walker::CodebaseScanner::new(&root_clone);
                            let file_nodes = scanner.scan_all();
                            if let Ok(mut g) = graph_clone.lock() {
                                g.clear();
                                for node in file_nodes {
                                    g.upsert_file_batch(node);
                                }
                                g.rebuild_all_edges();
                            }
                        });
                    }
                } else if let Some(path_str) = p.get("rootPath").and_then(|u| u.as_str()) {
                    let new_root = PathBuf::from(path_str);
                    if new_root.exists() && new_root.is_dir() && new_root != *root {
                        eprintln!(
                            "[layer-mcp] Switching workspace root to: {}",
                            new_root.display()
                        );
                        *root = new_root.clone();
                        let graph_clone = Arc::clone(graph);
                        let root_clone = new_root.clone();
                        std::thread::spawn(move || {
                            let scanner = crate::walker::CodebaseScanner::new(&root_clone);
                            let file_nodes = scanner.scan_all();
                            if let Ok(mut g) = graph_clone.lock() {
                                g.clear();
                                for node in file_nodes {
                                    g.upsert_file_batch(node);
                                }
                                g.rebuild_all_edges();
                            }
                        });
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
                        "version": env!("CARGO_PKG_VERSION")
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
                            "description": "Get file outline with symbols, signatures, and line numbers without bodies.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Workspace-relative file path"
                                    }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "find_symbol",
                            "description": "Find exact definition locations of functions, structs, or classes.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Exact or partial symbol name"
                                    },
                                    "file": {
                                        "type": "string",
                                        "description": "Optional file path to narrow search"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum results to return (default: 15)"
                                    }
                                },
                                "required": ["name"]
                            }
                        },
                        {
                            "name": "find_references",
                            "description": "Find all usages and references of a symbol across workspace.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "Symbol name to look up"
                                    },
                                    "code_only": {
                                        "type": "boolean",
                                        "description": "If true, search only code files, ignoring docs"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum references to return (default: 60)"
                                    }
                                },
                                "required": ["symbol"]
                            }
                        },
                        {
                            "name": "get_callers",
                            "description": "Find distinct caller functions grouped by enclosing symbol.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "Function or method name to find callers for"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum distinct callers to return (default: 25)"
                                    }
                                },
                                "required": ["symbol"]
                            }
                        },
                        {
                            "name": "get_lines",
                            "description": "Get exact slice of lines from a file.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Workspace-relative file path"
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
                            "description": "Semantic token and vector code search across project.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": {
                                        "type": "string",
                                        "description": "Search query or concept description"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum results to return (default: 3)"
                                    }
                                },
                                "required": ["query"]
                            }
                        },
                        {
                            "name": "get_symbol_body",
                            "description": "Get full implementation body of a specific function or class.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Symbol name to retrieve body for"
                                    },
                                    "file": {
                                        "type": "string",
                                        "description": "Optional file path hint"
                                    },
                                    "max_lines": {
                                        "type": "integer",
                                        "description": "Maximum lines to return (default: 400)"
                                    }
                                },
                                "required": ["name"]
                            }
                        },
                        {
                            "name": "get_impact_radius",
                            "description": "Blast-radius analysis returning callers and callees for a symbol.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "symbol": {
                                        "type": "string",
                                        "description": "Symbol or function name to analyze"
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
                            "description": "Compact directory overview with symbol counts. Pass path to narrow.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Optional relative subdirectory path to inspect"
                                    },
                                    "max_depth": {
                                        "type": "integer",
                                        "description": "Hierarchy traversal depth limit (default: 2)"
                                    },
                                    "limit": {
                                        "type": "integer",
                                        "description": "Maximum file entries to return (default: 25)"
                                    }
                                }
                            }
                        },
                        {
                            "name": "scaffold_template",
                            "description": "Scaffold minimal verified project starter in empty destination folder.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "template": {
                                        "type": "string",
                                        "description": "Starter template name (e.g. 'rust-cli')"
                                    },
                                    "project_name": {
                                        "type": "string",
                                        "description": "New project identifier"
                                    },
                                    "destination": {
                                        "type": "string",
                                        "description": "Optional destination directory (defaults to workspace root)"
                                    }
                                },
                                "required": ["template", "project_name"]
                            }
                        },
                        {
                            "name": "prepare_task_context",
                            "description": "Compile token-budgeted packet of relevant types, interfaces, and architecture.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "task": {
                                        "type": "string",
                                        "description": "Task description to compile context for"
                                    },
                                    "target_path": {
                                        "type": "string",
                                        "description": "Optional file path being edited"
                                    },
                                    "max_tokens": {
                                        "type": "integer",
                                        "description": "Maximum token budget (default: 1200)"
                                    }
                                },
                                "required": ["task"]
                            }
                        },
                        {
                            "name": "set_workspace",
                            "description": "Switch active workspace root and re-index project graph immediately.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Path to new workspace directory"
                                    }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "verify_project",
                            "description": "Fast AST precheck and native compiler verification with lifecycle tracking.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "force_compiler": {
                                        "type": "boolean",
                                        "description": "Run compiler even if AST check passes (default: false)"
                                    }
                                }
                            }
                        },
                        {
                            "name": "bootstrap_project",
                            "description": "Instant zero-overhead scaffolding for frontend, backend, or systems projects.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "preset": {
                                        "type": "string",
                                        "description": "Preset name (nextjs, react-ts, rust-bin, python, go, etc.)"
                                    },
                                    "project_name": {
                                        "type": "string",
                                        "description": "Project identifier name"
                                    },
                                    "destination": {
                                        "type": "string",
                                        "description": "Optional destination folder (defaults to workspace root)"
                                    },
                                    "packages": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Optional initial package dependencies"
                                    },
                                    "install_dependencies": {
                                        "type": "boolean",
                                        "description": "Whether to run package manager immediately (default: false)"
                                    }
                                },
                                "required": ["preset", "project_name"]
                            }
                        },
                        {
                            "name": "inspect_package_api",
                            "description": "Extract verified type definitions and signatures from installed dependencies.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "package_name": {
                                        "type": "string",
                                        "description": "Installed package or crate dependency name"
                                    },
                                    "symbol_name": {
                                        "type": "string",
                                        "description": "Optional exact symbol name to extract"
                                    },
                                    "query": {
                                        "type": "string",
                                        "description": "Optional filter term for exported symbols"
                                    }
                                },
                                "required": ["package_name"]
                            }
                        },
                        {
                            "name": "write_file",
                            "description": "Write file with snapshotting, sub-5ms AST check, and circuit breaker.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Workspace-relative file path"
                                    },
                                    "content": {
                                        "type": "string",
                                        "description": "Complete file content to write"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Verification tier: 'fast', 'full', or 'none' (default: 'fast')"
                                    },
                                    "auto_verify": {
                                        "type": "boolean",
                                        "description": "Deprecated: false maps to 'none', true to 'fast'"
                                    },
                                    "auto_rollback_on_cascade": {
                                        "type": "boolean",
                                        "description": "Auto-revert if edit causes cascading errors (default: false)"
                                    }
                                },
                                "required": ["path", "content"]
                            }
                        },
                        {
                            "name": "edit_file",
                            "description": "Surgically replace target snippet with AST precheck and snapshotting.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Workspace-relative file path"
                                    },
                                    "target": {
                                        "type": "string",
                                        "description": "Exact unique snippet to replace"
                                    },
                                    "replacement": {
                                        "type": "string",
                                        "description": "New replacement code"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Verification tier: 'fast', 'full', or 'none' (default: 'fast')"
                                    },
                                    "auto_verify": {
                                        "type": "boolean",
                                        "description": "Deprecated: false maps to 'none', true to 'fast'"
                                    },
                                    "auto_rollback_on_cascade": {
                                        "type": "boolean",
                                        "description": "Auto-revert if edit causes cascading errors (default: false)"
                                    }
                                },
                                "required": ["path", "target", "replacement"]
                            }
                        },
                        {
                            "name": "rollback_last_edit",
                            "description": "Revert most recent file write or edit from in-memory snapshot.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "scaffold_module",
                            "description": "Declaratively scaffold multiple files with types, imports, and stubs.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "module_name": {
                                        "type": "string",
                                        "description": "Logical module name"
                                    },
                                    "files": {
                                        "type": "array",
                                        "description": "Array of file specs with paths, types, and functions",
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
                            "description": "Atomically rename code identifier across all declaring and dependent files.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Relative path where symbol is declared or used"
                                    },
                                    "old_name": {
                                        "type": "string",
                                        "description": "Current identifier name"
                                    },
                                    "new_name": {
                                        "type": "string",
                                        "description": "New identifier name"
                                    },
                                    "verify_mode": {
                                        "type": "string",
                                        "enum": ["fast", "full", "none"],
                                        "description": "Verification tier after rename: 'fast', 'full', or 'none'"
                                    }
                                },
                                "required": ["path", "old_name", "new_name"]
                            }
                        },
                        {
                            "name": "verify_impacted_tests",
                            "description": "Run only test files impacted by modified code via graph tracing.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "changed_files": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "Optional list of changed relative file paths"
                                    }
                                }
                            }
                        },
                        {
                            "name": "get_session_stats",
                            "description": "Retrieve session telemetry, tokens saved, and tool call metrics.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "reset": {
                                        "type": "boolean",
                                        "description": "Whether to reset session counters to zero"
                                    }
                                }
                            }
                        },
                        {
                            "name": "init_harness",
                            "description": "Install AI agent steering directives for Layer MCP tool priority.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "harness": {
                                        "type": "string",
                                        "description": "Target harness ('codex', 'claude', 'cursor', 'windsurf', or 'all')"
                                    },
                                    "dest": {
                                        "type": "string",
                                        "description": "Optional destination directory (defaults to workspace root)"
                                    },
                                    "global": {
                                        "type": "boolean",
                                        "description": "Whether to configure global harness MCP settings"
                                    }
                                }
                            }
                        }
                    ]
                }
            });
            send_json_line(writer, &resp)?;
        }
        "ping" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            });
            send_json_line(writer, &resp)?;
        }
        "resources/list" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resources": []
                }
            });
            send_json_line(writer, &resp)?;
        }
        "resources/templates/list" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resourceTemplates": []
                }
            });
            send_json_line(writer, &resp)?;
        }
        "prompts/list" => {
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "prompts": []
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
    // Only auto-switch workspace root if agent explicitly supplied an absolute path to a different project
    if !p.is_absolute() {
        return;
    }
    let abs_p = p.to_path_buf();
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
        return serde_json::to_string(&res).unwrap_or_default();
    }

    // Auto-detect project root if agent supplied an explicit absolute path pointing to a different project
    let candidate_path = args
        .get("path")
        .or_else(|| args.get("file"))
        .or_else(|| args.get("target_path"))
        .and_then(|v| v.as_str());

    maybe_switch_workspace_root(root, g, candidate_path, Some(pkg_indexer));

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
                Ok(res) => serde_json::to_string(&res).unwrap_or_else(|_| "{}".to_string()),
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
            serde_json::to_string(&packet).unwrap_or_default()
        }
        "verify_project" => {
            let force_compiler = args
                .get("force_compiler")
                .and_then(|f| f.as_bool())
                .unwrap_or(false);

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

            let report = crate::verifier::Verifier::verify_workspace_with_options(
                root,
                g,
                Some(pkg_indexer),
                force_compiler,
            );
            g.last_verification = Some(report.diagnostics.clone());
            serde_json::to_string(&report).unwrap_or_default()
        }
        "inspect_package_api" => {
            let pkg_name = args
                .get("package_name")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let sym_name = args.get("symbol_name").and_then(|s| s.as_str());
            let query = args.get("query").and_then(|q| q.as_str());

            match pkg_indexer.inspect_package(root, pkg_name, sym_name, query) {
                Ok(summary) => serde_json::to_string(&summary).unwrap_or_default(),
                Err(e) => {
                    let unindexed = serde_json::json!({
                        "status": "unindexed_offline",
                        "package": pkg_name,
                        "advisory": format!("Package types for '{}' are unindexed or offline. Proceed directly using Layer mutation tools. Detail: {}", pkg_name, e)
                    });
                    serde_json::to_string(&unindexed).unwrap_or_default()
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
                .unwrap_or(false);

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
                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
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
                .unwrap_or(false);

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
                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "rollback_last_edit" => {
            match crate::mutation::MutationManager::rollback_last_edit(root, g) {
                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
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
                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
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
            let install_dependencies = args
                .get("install_dependencies")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match crate::scaffold::bootstrap_project_with_options(
                preset,
                project_name,
                &dest_parent,
                packages,
                install_dependencies,
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
                    serde_json::to_string(&res).unwrap_or_default()
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
                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "verify_impacted_tests" => {
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
            serde_json::to_string(&res).unwrap_or_default()
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
            serde_json::to_string(&res).unwrap_or_default()
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
            serde_json::to_string(&res).unwrap_or_default()
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
        assert!(
            switch_res.contains("\"success\":true") || switch_res.contains("\"success\": true")
        );
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
