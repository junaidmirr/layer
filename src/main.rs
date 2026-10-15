use clap::{Parser, Subcommand};
use colored::*;
use layer::context_engine::ContextEngine;
use layer::graph::VectorGraph;
use layer::mcp;
use layer::walker::CodebaseScanner;
use layer::watcher::CodeWatcher;
use rustyline::DefaultEditor;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "layer")]
#[command(about = "Intelligent Codebase Context Engine & Vector Graph Layer", long_about = None)]
struct Cli {
    /// Root path of the codebase to analyze (defaults to current directory)
    #[arg(short, long, global = true, default_value = ".")]
    path: PathBuf,

    /// Output clean structured JSON without decorations or ANSI color codes (agent-facing)
    #[arg(long, global = true)]
    json: bool,

    /// Suppress verbose initialization banner and output only clean text/JSON
    #[arg(short, long, global = true)]
    quiet: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start Model Context Protocol (MCP) stdio server for AI agents
    Mcp,
    /// Return compact skeleton of a file (imports, signatures, line numbers)
    Skeleton {
        /// Relative path to file
        file_path: String,
    },
    /// Find all references / call sites to a symbol across the codebase
    References {
        /// Symbol or function name
        symbol: String,
    },
    /// Find callers of a function or method across the codebase
    Callers {
        /// Symbol or function name
        symbol: String,
    },
    /// Find declaration(s) of a symbol
    Find {
        /// Symbol name
        name: String,
    },
    /// Get exact body/implementation of a symbol
    Body {
        /// Symbol name
        name: String,
        /// Optional file hint
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Get 1-hop impact radius of a symbol (callers and callees)
    Impact {
        /// Symbol name
        symbol: String,
        /// Optional file hint
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Interactive intelligent context REPL
    Repl,
    /// Quick one-shot query against the codebase
    Query {
        /// Query string
        prompt: Vec<String>,
    },
    /// Print directory structure and symbol summaries
    Tree,
    /// Semantic vector search across the codebase
    Search {
        /// Semantic query
        query: String,
        /// Maximum number of hits to return
        #[arg(short, long, default_value_t = 5)]
        limit: usize,
    },
    /// Show exact file content
    File {
        /// Relative path to file
        file_path: String,
    },
    /// Show functions and symbols in a file
    Symbols {
        /// Relative path to file
        file_path: String,
    },
    /// Show slice of lines in a file
    Lines {
        /// Relative path to file
        file_path: String,
        /// 1-based start line
        start: usize,
        /// 1-based end line
        end: usize,
    },
    /// Show codebase change history, modifications, deletions, and moves
    History {
        /// Optional relative path to file to filter by
        file_path: Option<String>,
    },
    /// Inspect diff and prior content for a specific history event ID
    Diff {
        /// History record event ID (e.g. 1)
        id: usize,
    },
    /// Scaffold a new project starter in an empty destination directory
    Scaffold {
        /// Project template (e.g. rust-cli)
        #[arg(short, long, default_value = "rust-cli")]
        template: String,
        /// Project name (e.g. my_tool)
        #[arg(short, long)]
        name: String,
        /// Optional destination directory (defaults to current path)
        #[arg(short, long)]
        dest: Option<PathBuf>,
    },
    /// Compile a task-specific, token-bounded context packet
    Prepare {
        /// Task description (e.g. 'implement postgres user repository')
        task: String,
        /// Optional target file path being written or edited
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Token budget ceiling
        #[arg(short, long, default_value_t = 1200)]
        max_tokens: usize,
    },
    /// Keep running as a live watching daemon
    Watch,
    /// Display Layer ROI and session telemetry statistics
    Stats {
        /// Optional path to workspace root
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        /// Reset session statistics to zero
        #[arg(long)]
        reset: bool,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let root = cli.path.canonicalize().unwrap_or(cli.path);

    if let Some(Commands::Stats { workspace, reset }) = cli.command {
        let target_root = workspace.unwrap_or_else(|| root.clone());
        let mut tracker = layer::telemetry::TelemetryTracker::load(&target_root);
        if reset {
            let _ = tracker.reset(&target_root);
            if !cli.quiet && !cli.json {
                println!("{}", "Telemetry session metrics reset to zero.".green());
            }
        }

        if cli.json {
            println!("{}", serde_json::to_string_pretty(&tracker)?);
        } else {
            print!("{}", tracker.format_dashboard());
        }
        return Ok(());
    }

    let is_mcp = matches!(cli.command, Some(Commands::Mcp));
    let quiet = cli.quiet || cli.json || is_mcp;

    if !quiet {
        println!(
            "{} {}",
            "⚡ Initializing Layer Context Engine for:".cyan().bold(),
            root.display().to_string().yellow()
        );
    }

    let start_time = std::time::Instant::now();
    let scanner = CodebaseScanner::new(&root);
    let mut vector_graph = VectorGraph::new();

    let file_nodes = scanner.scan_all();
    let file_count = file_nodes.len();

    for node in file_nodes {
        vector_graph.upsert_file(node);
    }
    let elapsed = start_time.elapsed();

    if !quiet {
        println!(
            "{} Indexed {} files with {} vector chunks & AST nodes in {:.2?}.\n",
            "✓".green().bold(),
            file_count.to_string().green(),
            vector_graph.chunks.len().to_string().green(),
            elapsed
        );
    }

    let graph_arc = Arc::new(Mutex::new(vector_graph));

    // Start background watcher for persistent modes (MCP, REPL, Watch)
    let is_persistent =
        is_mcp || matches!(cli.command, Some(Commands::Repl) | Some(Commands::Watch));
    if is_persistent {
        let watcher = CodeWatcher::new(root.clone(), Arc::clone(&graph_arc));
        let _watcher_thread = watcher.start_in_background();
    }

    match cli.command {
        Some(Commands::Mcp) => {
            mcp::run_mcp_server(graph_arc, root)?;
        }
        Some(Commands::Skeleton { file_path }) => {
            let g = graph_arc.lock().unwrap();
            let path_match = g
                .find_file_match(&file_path)
                .unwrap_or_else(|| PathBuf::from(&file_path));
            match g.get_file_skeleton(&path_match) {
                Some(sk) => {
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&sk)?);
                    } else {
                        println!("File: {} ({} lines)", sk.path, sk.total_lines);
                        if !sk.imports.is_empty() {
                            println!("Imports ({}):", sk.imports.len());
                            for imp in &sk.imports {
                                println!("  {}", imp);
                            }
                        }
                        println!("Symbols ({}):", sk.symbols.len());
                        for s in &sk.symbols {
                            println!("  [L{}-L{}] {} {}", s.lines[0], s.lines[1], s.kind, s.name);
                            println!("    {}", s.signature);
                        }
                        let json_str = serde_json::to_string(&sk).unwrap_or_default();
                        println!(
                            "Token footprint: {} bytes (~{} tokens)",
                            json_str.len(),
                            json_str.len().div_ceil(4)
                        );
                    }
                }
                None => {
                    eprintln!("File '{}' not found.", file_path);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::References { symbol }) => {
            let g = graph_arc.lock().unwrap();
            let refs = g.find_references(&symbol);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&refs)?);
            } else {
                println!("References to '{}' ({} found):", symbol, refs.len());
                for r in refs {
                    let caller_info = r
                        .caller_symbol
                        .map(|c| format!(" (in {})", c))
                        .unwrap_or_default();
                    println!("  {}:{}{} -> {}", r.file, r.line, caller_info, r.text);
                }
            }
        }
        Some(Commands::Callers { symbol }) => {
            let g = graph_arc.lock().unwrap();
            let callers = g.get_callers(&symbol);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&callers)?);
            } else {
                println!("Callers of '{}' ({} found):", symbol, callers.len());
                for c in callers {
                    println!(
                        "  {} [L{}-L{}] in {}:",
                        c.caller_symbol, c.caller_lines[0], c.caller_lines[1], c.caller_file
                    );
                    println!("    Line {}: {}", c.call_line, c.call_snippet);
                }
            }
        }
        Some(Commands::Find { name }) => {
            let g = graph_arc.lock().unwrap();
            let hits = g.find_symbol(&name);
            if cli.json {
                let list: Vec<_> = hits
                    .iter()
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
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                println!("Declarations of '{}' ({} found):", name, hits.len());
                for (path, s) in hits {
                    println!(
                        "  {} [L{}-L{}] in {}",
                        s.kind,
                        s.start_line,
                        s.end_line,
                        path.display()
                    );
                    println!("    {}", s.signature);
                }
            }
        }
        Some(Commands::Body { name, file }) => {
            let g = graph_arc.lock().unwrap();
            let file_buf = file.as_deref().map(PathBuf::from);
            match g.get_symbol_body(&name, file_buf.as_deref()) {
                Some(body) => {
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&body)?);
                    } else {
                        println!(
                            "Symbol: {} (in {}) L{}-L{}:",
                            body.symbol_name, body.file, body.lines[0], body.lines[1]
                        );
                        println!("Signature: {}", body.signature);
                        if !body.candidates.is_empty() {
                            println!("Alternative candidates ({}):", body.candidates.len());
                            for cand in &body.candidates {
                                println!("  ↳ {}", cand);
                            }
                        }
                        if body.is_truncated {
                            println!("[Note: Output body was truncated to save tokens]");
                        }
                        let json_str = serde_json::to_string(&body).unwrap_or_default();
                        println!(
                            "Token footprint: {} bytes (~{} tokens)",
                            json_str.len(),
                            json_str.len().div_ceil(4)
                        );
                        println!("----------------------------------------");
                        println!("{}", body.body);
                    }
                }
                None => {
                    eprintln!("Symbol '{}' not found.", name);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Impact { symbol, file }) => {
            let g = graph_arc.lock().unwrap();
            let file_buf = file.as_deref().map(PathBuf::from);
            let impact = g.get_impact_radius(&symbol, file_buf.as_deref());
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&impact)?);
            } else {
                println!("Impact radius for '{}':", impact.target_symbol);
                if let Some(ref f) = impact.file {
                    println!("  Defined in: {}", f);
                }
                if let Some(ref sig) = impact.signature {
                    println!("  Signature:  {}", sig);
                }
                println!(
                    "  Confidence: callers={}, callees={}",
                    impact.callers_confidence, impact.callees_confidence
                );
                println!(
                    "  Callers ({}/{} shown{}):",
                    impact.callers.len(),
                    impact.total_callers,
                    if impact.callers_truncated {
                        ", capped at 10"
                    } else {
                        ""
                    }
                );
                for c in &impact.callers {
                    let approx = if c.is_approximate { " (~approx)" } else { "" };
                    println!(
                        "    ↳ {} in {}:{}{}",
                        c.caller_symbol, c.caller_file, c.call_line, approx
                    );
                }
                println!("  Callees ({}):", impact.callees.len());
                for c in &impact.callees {
                    println!("    ↳ calls {}", c);
                }
                if !impact.candidates.is_empty() {
                    println!("  Alternative candidates ({}):", impact.candidates.len());
                    for cand in &impact.candidates {
                        println!("    ↳ {}", cand);
                    }
                }
                let json_str = serde_json::to_string(&impact).unwrap_or_default();
                println!(
                    "  Token footprint: {} bytes (~{} tokens)",
                    json_str.len(),
                    json_str.len().div_ceil(4)
                );
            }
        }
        Some(Commands::Tree) => {
            let g = graph_arc.lock().unwrap();
            if cli.json {
                let list: Vec<_> = g
                    .list_files()
                    .iter()
                    .filter_map(|p| {
                        g.files.get(p).map(|node| {
                            json!({
                                "path": p.display().to_string(),
                                "lines": node.line_count,
                                "symbols": node.symbols.len()
                            })
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                println!("{}", g.format_tree());
            }
        }
        Some(Commands::File { file_path }) => {
            let g = graph_arc.lock().unwrap();
            let path_match = g
                .find_file_match(&file_path)
                .unwrap_or_else(|| PathBuf::from(&file_path));
            match g.get_file_content(&path_match) {
                Some(content) => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(
                                &json!({ "path": path_match.display().to_string(), "content": content })
                            )?
                        );
                    } else {
                        for (idx, line) in content.lines().enumerate() {
                            println!("{:4} | {}", idx + 1, line);
                        }
                    }
                }
                None => {
                    eprintln!("File '{}' not found.", file_path);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Symbols { file_path }) => {
            let g = graph_arc.lock().unwrap();
            let path_match = g
                .find_file_match(&file_path)
                .unwrap_or_else(|| PathBuf::from(&file_path));
            match g.get_file_symbols(&path_match) {
                Some(syms) => {
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&syms)?);
                    } else {
                        for s in syms {
                            println!(
                                "• [{}] {} (L{}-L{}) - {}",
                                s.kind, s.name, s.start_line, s.end_line, s.signature
                            );
                        }
                    }
                }
                None => {
                    eprintln!("File '{}' not found.", file_path);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Lines {
            file_path,
            start,
            end,
        }) => {
            let g = graph_arc.lock().unwrap();
            let path_match = g
                .find_file_match(&file_path)
                .unwrap_or_else(|| PathBuf::from(&file_path));
            match g.get_file_lines(&path_match, start, end) {
                Some(slice) => {
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(
                                &json!({ "path": path_match.display().to_string(), "start": start, "end": end, "lines": slice })
                            )?
                        );
                    } else {
                        print!("{}", slice);
                    }
                }
                None => {
                    eprintln!("File '{}' not found.", file_path);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::History { file_path }) => {
            let g = graph_arc.lock().unwrap();
            let target_buf = file_path.map(PathBuf::from);
            if cli.json {
                let records: Vec<_> = match target_buf.as_deref() {
                    Some(p) => g.history.get_file_records(p),
                    None => g.history.get_all_records().iter().collect(),
                };
                println!("{}", serde_json::to_string_pretty(&records)?);
            } else {
                println!("{}", g.history.format_history(target_buf.as_deref()));
            }
        }
        Some(Commands::Diff { id }) => {
            let g = graph_arc.lock().unwrap();
            if cli.json {
                match g.history.get_record_by_id(id) {
                    Some(rec) => println!("{}", serde_json::to_string_pretty(rec)?),
                    None => {
                        eprintln!("History event #{} not found.", id);
                        std::process::exit(1);
                    }
                }
            } else {
                match g.history.format_record_detail(id) {
                    Some(detail) => println!("{}", detail),
                    None => println!("History event #{} not found.", id),
                }
            }
        }
        Some(Commands::Search { query, limit }) => {
            let g = graph_arc.lock().unwrap();
            let hits = g.search(&query, limit);
            if cli.json {
                let list: Vec<_> = hits
                    .iter()
                    .map(|h| {
                        json!({
                            "file": h.file_path.display().to_string(),
                            "score": (h.score * 1000.0).round() / 1000.0,
                            "symbol": h.symbol_name,
                            "lines": [h.start_line, h.end_line],
                            "snippet": h.snippet
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for (i, hit) in hits.iter().enumerate() {
                    println!(
                        "{}. {} (L{}..L{}) - Score: {:.3}",
                        i + 1,
                        hit.file_path.display(),
                        hit.start_line,
                        hit.end_line,
                        hit.score
                    );
                    for line in hit.snippet.lines().take(5) {
                        println!("    {}", line);
                    }
                    println!();
                }
            }
        }
        Some(Commands::Query { prompt }) => {
            let query_text = prompt.join(" ");
            let g = graph_arc.lock().unwrap();
            println!("{}", ContextEngine::execute_query(&g, &query_text));
        }
        Some(Commands::Scaffold {
            template,
            name,
            dest,
        }) => {
            let target = dest.unwrap_or_else(|| root.clone());
            let supported = match layer::scaffold::SupportedTemplate::parse(&template) {
                Some(t) => t,
                None => {
                    eprintln!(
                        "Error: Unsupported template '{}'. Supported templates: 'rust-cli'",
                        template
                    );
                    std::process::exit(1);
                }
            };
            let mut g = graph_arc.lock().unwrap();
            match layer::scaffold::scaffold_project(supported, &name, &target, &mut g) {
                Ok(res) => {
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&res)?);
                    } else {
                        println!("{} {}", "✓".green().bold(), res.message.bold());
                        println!("  Destination: {}", res.destination.cyan());
                        println!("  Created files:");
                        for f in &res.created_files {
                            println!("    • {}", f);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("{} {}", "Error:".red().bold(), err);
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Prepare {
            task,
            file,
            max_tokens,
        }) => {
            let g = graph_arc.lock().unwrap();
            let packet = layer::context_compiler::ContextCompiler::compile_task_context(
                &g,
                &task,
                file.as_deref(),
                max_tokens,
            );
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&packet)?);
            } else {
                println!(
                    "{}",
                    format!("📋 Compiled Context for Task: {}", packet.task)
                        .cyan()
                        .bold()
                );
                println!(
                    "  Budget: {}/{} tokens (truncated: {})",
                    packet.budget.estimated_tokens,
                    packet.budget.max_tokens,
                    packet.budget.is_truncated
                );
                if !packet.interfaces.is_empty() {
                    println!("\n  {}", "Interfaces / Traits:".yellow().bold());
                    for iface in &packet.interfaces {
                        println!(
                            "    • {} ({}) in {}:{}..{}",
                            iface.symbol, iface.kind, iface.file, iface.lines[0], iface.lines[1]
                        );
                        println!("      Signature: {}", iface.signature.dimmed());
                    }
                }
                if !packet.types.is_empty() {
                    println!("\n  {}", "Types & Models:".yellow().bold());
                    for t in &packet.types {
                        println!(
                            "    • {} ({}) in {}:{}..{}",
                            t.symbol, t.kind, t.file, t.lines[0], t.lines[1]
                        );
                    }
                }
                if !packet.errors.is_empty() {
                    println!("\n  {}", "Error Types:".yellow().bold());
                    for err in &packet.errors {
                        println!(
                            "    • {} in {}:{}..{}",
                            err.symbol, err.file, err.lines[0], err.lines[1]
                        );
                    }
                }
                if !packet.imports.is_empty() {
                    println!("\n  {}", "Relevant Imports:".yellow().bold());
                    for imp in &packet.imports {
                        println!("    {}", imp);
                    }
                }
                if !packet.unresolved_dependencies.is_empty() {
                    println!("\n  {}", "Unresolved Dependencies:".red().bold());
                    for un in &packet.unresolved_dependencies {
                        println!("    ⚠ {}", un);
                    }
                }
            }
        }
        Some(Commands::Watch) => {
            println!("{}", "👀 Watch mode active. Monitoring file changes in real-time... (Press Ctrl+C to stop)".magenta());
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        Some(Commands::Stats { .. }) => unreachable!(),
        None | Some(Commands::Repl) => {
            run_repl(graph_arc)?;
        }
    }

    Ok(())
}

fn run_repl(graph: Arc<Mutex<VectorGraph>>) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        "=======================================================".cyan()
    );
    println!(
        "{}",
        "  Layer Context Engine REPL (Interactive Prompt)       ".bold()
    );
    println!(
        "{}",
        "=======================================================".cyan()
    );
    println!("Type your prompts naturally. Examples:");
    println!("  • 'give me file structure'");
    println!("  • 'history' or 'what changed'");
    println!("  • 'diff #1' (view exact diff and older code)");
    println!("  • 'this files code src/main.rs'");
    println!("  • 'all functions in src/scanner.rs'");
    println!("  • 'lines 20 to 60 in src/graph.rs'");
    println!("  • 'search vector embedding computation'");
    println!("  • 'exit' or 'quit' to leave\n");

    let mut rl = DefaultEditor::new()?;

    loop {
        let readline = rl.readline("layer> ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "exit" || trimmed == "quit" {
                    println!("Goodbye!");
                    break;
                }
                let _ = rl.add_history_entry(trimmed);

                let g = match graph.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };

                let response = ContextEngine::execute_query(&g, trimmed);
                println!("\n{}\n", response);
            }
            Err(_) => {
                println!("\nGoodbye!");
                break;
            }
        }
    }

    Ok(())
}
