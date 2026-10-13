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
    while reader.read_line(&mut line)? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(method) = val.get("method").and_then(|m| m.as_str())
        {
            let id = val.get("id").cloned();
            handle_rpc_method(
                &graph,
                &mut root,
                method,
                id,
                val.get("params"),
                &mut writer,
                &mut recent_queries,
            )?;
        }

        line.clear();
    }

    Ok(())
}

fn handle_rpc_method(
    graph: &Arc<Mutex<VectorGraph>>,
    root: &mut PathBuf,
    method: &str,
    id: Option<serde_json::Value>,
    params: Option<&serde_json::Value>,
    writer: &mut io::StdoutLock,
    recent_queries: &mut Vec<String>,
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
                    }
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

            let mut content_text = execute_tool(&mut g, root, tool_name, arguments);
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

/// Canonicalizes a path from user/agent, resolves it against root, and ensures it is safely within root
fn resolve_safe_path(root: &Path, raw_path: &str) -> Result<PathBuf, String> {
    if raw_path.trim().is_empty() {
        return Err("Error: Path cannot be empty.".to_string());
    }

    let input_path = Path::new(raw_path);
    let candidate = if input_path.is_absolute() {
        input_path.to_path_buf()
    } else {
        root.join(input_path)
    };

    let canonical_root = match root.canonicalize() {
        Ok(cr) => cr,
        Err(_) => root.to_path_buf(),
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

fn execute_tool(
    g: &mut VectorGraph,
    root: &Path,
    tool_name: &str,
    args: Option<&serde_json::Value>,
) -> String {
    let empty_args = json!({});
    let args = args.unwrap_or(&empty_args);

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
}
