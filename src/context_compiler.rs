use crate::graph::VectorGraph;
use crate::models::{ParserProvenance, SymbolBlock, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContextItem {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub lines: [usize; 2],
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContextBudget {
    pub estimated_tokens: usize,
    pub max_tokens: usize,
    pub is_truncated: bool,
    pub omitted_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted_symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HologramSlice {
    pub file: String,
    pub start_line: usize,
    pub end_line: usize,
    pub code: String,
    pub score: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String, // "hot" or "warm"
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provenance: String, // "tree_sitter", "universal_parser", "heuristic"
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String, // "lexical_bm25f", "graph_expansion", "companion_test"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContextDiagnostics {
    pub total_indexed_files: usize,
    pub total_indexed_symbols: usize,
    pub matched_query_tokens: Vec<String>,
    pub graph_expansion_applied: bool,
    pub test_discovery_applied: bool,
    pub hot_slices_count: usize,
    pub warm_slices_count: usize,
    pub omitted_cold_count: usize,
    pub execution_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContextOptions {
    pub max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<PathBuf>,
    pub enable_graph_expansion: bool,
    pub enable_test_discovery: bool,
    pub max_hot_slices: usize,
    pub max_files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ranking_mode: Option<String>,
}

impl Default for TaskContextOptions {
    fn default() -> Self {
        Self {
            max_tokens: 1400,
            target_path: None,
            enable_graph_expansion: true,
            enable_test_discovery: true,
            max_hot_slices: 5,
            max_files: 10,
            ranking_mode: Some("bm25f".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContextPacket {
    pub task: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub architecture_overview: Vec<String>,
    pub interfaces: Vec<TaskContextItem>,
    pub types: Vec<TaskContextItem>,
    pub errors: Vec<TaskContextItem>,
    pub imports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_dependencies: Vec<String>,
    pub budget: TaskContextBudget,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hologram: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub code_slices: Vec<HologramSlice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<TaskContextDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Interface,
    Error,
    Type,
    Function,
}

struct HACSCandidate {
    block: SymbolBlock,
    category: Category,
    bm25f_score: f64,
    final_score: f64,
    selection_reason: &'static str,
}

/// Splits any identifier, symbol, or text into sub-tokens along camelCase,
/// PascalCase, snake_case, kebab-case, acronyms, and digits, preserving compounds.
pub fn subtokenize_identifier(text: &str) -> Vec<String> {
    let stop_words: HashSet<&str> = [
        "the",
        "a",
        "an",
        "in",
        "to",
        "for",
        "with",
        "implement",
        "add",
        "create",
        "and",
        "or",
        "of",
        "on",
        "new",
        "from",
        "by",
        "that",
        "this",
        "it",
        "is",
        "as",
        "at",
        "be",
        "do",
    ]
    .into_iter()
    .collect();

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    // Split on punctuation / separators
    let raw_tokens: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    for raw in raw_tokens {
        let parts = split_camel_and_acronyms(raw);
        let mut clean_parts = Vec::new();

        for p in parts {
            let lower = p.to_lowercase();
            if lower.len() > 1 && !stop_words.contains(lower.as_str()) && seen.insert(lower.clone())
            {
                clean_parts.push(lower.clone());
                result.push(lower);
            }
        }

        // If identifier had multiple sub-tokens, include the full combined lowercase identifier
        let full_lower = raw.to_lowercase();
        if clean_parts.len() > 1 && seen.insert(full_lower.clone()) {
            result.push(full_lower);
        }
    }

    // Preserve compound identifiers containing separators (e.g. calculate_crc32_checksum, get_lines, rate-limit-exceeded)
    let full_trimmed = text.trim().to_lowercase();
    let has_separator =
        full_trimmed.contains('_') || full_trimmed.contains('-') || full_trimmed.contains('.');
    if has_separator
        && full_trimmed.split_whitespace().count() <= 1
        && seen.insert(full_trimmed.clone())
    {
        result.push(full_trimmed);
    }

    result
}

/// Helper splitting camelCase, PascalCase, and acronyms (e.g. XMLHTTPRequest -> XML, HTTP, Request)
fn split_camel_and_acronyms(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut parts = Vec::new();
    let mut curr = String::new();

    let known_acronyms: &[&str] = &[
        "xml", "http", "https", "rpc", "url", "uri", "ast", "api", "json", "sql", "html", "css",
        "tcp", "udp", "crc", "eof", "io", "db", "vm", "os", "id", "ip",
    ];

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        // Check for run-together known acronyms at start of curr
        if curr.len() >= 3 {
            let curr_lower = curr.to_lowercase();
            for &acr in known_acronyms {
                if curr_lower.starts_with(acr) && curr_lower.len() > acr.len() {
                    let first = curr[..acr.len()].to_string();
                    let rest = curr[acr.len()..].to_string();
                    parts.push(first);
                    curr = rest;
                    break;
                }
            }
        }

        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();

            let is_boundary = (prev.is_lowercase() && ch.is_uppercase())
                || (prev.is_uppercase()
                    && ch.is_uppercase()
                    && next.is_some_and(|n| n.is_lowercase()))
                || (prev.is_alphabetic() && ch.is_ascii_digit())
                || (prev.is_ascii_digit() && ch.is_alphabetic());

            if is_boundary && !curr.is_empty() {
                parts.push(curr);
                curr = String::new();
            }
        }

        curr.push(ch);
        i += 1;
    }

    if !curr.is_empty() {
        let curr_lower = curr.to_lowercase();
        let mut split_applied = false;
        for &acr in known_acronyms {
            if curr_lower.starts_with(acr) && curr_lower.len() > acr.len() {
                let first = curr[..acr.len()].to_string();
                let rest = curr[acr.len()..].to_string();
                parts.push(first);
                parts.push(rest);
                split_applied = true;
                break;
            }
        }
        if !split_applied {
            parts.push(curr);
        }
    }

    parts
}

pub struct ContextCompiler;

impl ContextCompiler {
    /// Compiles a task-specific, token-bounded context packet using HACS with default options
    pub fn compile_task_context(
        graph: &VectorGraph,
        task: &str,
        target_path: Option<&Path>,
        max_tokens: usize,
    ) -> TaskContextPacket {
        let options = TaskContextOptions {
            max_tokens,
            target_path: target_path.map(PathBuf::from),
            enable_graph_expansion: true,
            enable_test_discovery: true,
            max_hot_slices: 5,
            max_files: 10,
            ranking_mode: Some("bm25f".to_string()),
        };
        Self::compile_task_context_with_options(graph, task, &options)
    }

    /// Full HACS retrieval pipeline with configurable options
    pub fn compile_task_context_with_options(
        graph: &VectorGraph,
        task: &str,
        options: &TaskContextOptions,
    ) -> TaskContextPacket {
        let start_time = std::time::Instant::now();
        let deadline = start_time + std::time::Duration::from_millis(450);
        let query_tokens = subtokenize_identifier(task);
        let task_lower = task.to_lowercase();
        let target_path_opt = options.target_path.as_deref();

        let mut clean_query_words: Vec<String> = query_tokens
            .iter()
            .map(|s| s.to_lowercase())
            .filter(|s| s.len() >= 2)
            .collect();
        for w in task_lower.split(|c: char| !c.is_alphanumeric()) {
            let wl = w.to_lowercase();
            if wl.len() >= 3 && !clean_query_words.contains(&wl) {
                clean_query_words.push(wl);
            }
        }

        // 1. Build typed SymbolBlocks from candidate files and symbols
        let mut blocks: Vec<SymbolBlock> = Vec::new();
        let mut total_symbols = 0usize;

        for (path, node) in &graph.files {
            total_symbols += node.symbols.len();
            let path_str_lower = path.to_string_lossy().to_lowercase();
            let path_matches_target = target_path_opt.is_some_and(|tp| path.starts_with(tp));
            let path_matches_query = clean_query_words.iter().any(|q| path_str_lower.contains(q));

            let ext = node.extension.as_deref().unwrap_or("");
            let lang_str = ext.to_string();
            let is_tree_sitter = matches!(
                ext,
                "rs" | "js"
                    | "mjs"
                    | "cjs"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "py"
                    | "pyw"
                    | "pyi"
                    | "go"
                    | "c"
                    | "cpp"
                    | "cc"
                    | "cxx"
                    | "h"
                    | "hpp"
                    | "hxx"
            );
            let provenance = if is_tree_sitter {
                ParserProvenance::TreeSitter
            } else {
                ParserProvenance::UniversalParser
            };

            for sym in &node.symbols {
                if !path_matches_target && !path_matches_query {
                    let name_lower = sym.name.to_lowercase();
                    let matches = clean_query_words.iter().any(|q| {
                        name_lower.contains(q)
                            || sym.signature.to_lowercase().contains(q)
                            || sym
                                .doc_comment
                                .as_deref()
                                .is_some_and(|d| d.to_lowercase().contains(q))
                    });
                    if !matches && !task_lower.contains(&name_lower) {
                        continue;
                    }
                }

                let normalized_tokens = subtokenize_identifier(&sym.name);
                let qualified_name = format!("{}:{}", path.display(), sym.name);

                blocks.push(SymbolBlock {
                    file_path: path.clone(),
                    language: lang_str.clone(),
                    name: sym.name.clone(),
                    qualified_name,
                    kind: sym.kind,
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                    signature: sym.signature.clone(),
                    body_preview: None,
                    doc_comment: sym.doc_comment.clone(),
                    parent_symbol: None,
                    normalized_tokens,
                    provenance,
                    confidence: 1.0,
                });
            }
        }

        // Handle empty index gracefully
        if blocks.is_empty() {
            let execution_ms = start_time.elapsed().as_millis();
            return TaskContextPacket {
                task: task.to_string(),
                relevant_files: Vec::new(),
                architecture_overview: Vec::new(),
                interfaces: Vec::new(),
                types: Vec::new(),
                errors: Vec::new(),
                imports: Vec::new(),
                unresolved_dependencies: Vec::new(),
                budget: TaskContextBudget {
                    estimated_tokens: 0,
                    max_tokens: options.max_tokens,
                    is_truncated: false,
                    omitted_count: 0,
                    omitted_symbols: Vec::new(),
                },
                hologram: String::new(),
                code_slices: Vec::new(),
                directive: Some("HOLOGRAM_EMPTY: No indexed code blocks available.".to_string()),
                diagnostics: Some(TaskContextDiagnostics {
                    total_indexed_files: graph.files.len(),
                    total_indexed_symbols: total_symbols,
                    matched_query_tokens: query_tokens,
                    graph_expansion_applied: false,
                    test_discovery_applied: false,
                    hot_slices_count: 0,
                    warm_slices_count: 0,
                    omitted_cold_count: 0,
                    execution_ms,
                }),
            };
        }

        let n_docs = total_symbols.max(blocks.len()).max(1);

        // 2. Pre-calculate field lengths and average field lengths for BM25F
        // Fields: name (w=6.0, b=0.75), path (w=3.5, b=0.50), sig (w=2.5, b=0.60), body (w=1.0, b=0.80)
        let w_name = 6.0f64;
        let w_path = 3.5f64;
        let w_sig = 2.5f64;
        let w_body = 1.0f64;

        let b_name = 0.75f64;
        let b_path = 0.50f64;
        let b_sig = 0.60f64;
        let b_body = 0.80f64;
        let k1 = 1.2f64;

        let mut total_len_name = 0usize;
        let mut total_len_path = 0usize;
        let mut total_len_sig = 0usize;
        let mut total_len_body = 0usize;

        // Precompute tokens and field lengths per block
        struct BlockTokens {
            name_tokens: Vec<String>,
            path_tokens: Vec<String>,
            sig_tokens: Vec<String>,
            body_tokens: Vec<String>,
        }

        let mut block_tokens_list = Vec::with_capacity(blocks.len());

        for b in &blocks {
            let name_tokens = b.normalized_tokens.clone();
            let path_tokens = subtokenize_identifier(&b.file_path.to_string_lossy());
            let sig_tokens = subtokenize_identifier(&b.signature);
            let body_tokens = b
                .doc_comment
                .as_deref()
                .map(subtokenize_identifier)
                .unwrap_or_default();

            total_len_name += name_tokens.len();
            total_len_path += path_tokens.len();
            total_len_sig += sig_tokens.len();
            total_len_body += body_tokens.len();

            block_tokens_list.push(BlockTokens {
                name_tokens,
                path_tokens,
                sig_tokens,
                body_tokens,
            });
        }

        let avg_len_name = (total_len_name as f64 / blocks.len() as f64).max(1.0);
        let avg_len_path = (total_len_path as f64 / blocks.len() as f64).max(1.0);
        let avg_len_sig = (total_len_sig as f64 / blocks.len() as f64).max(1.0);
        let avg_len_body = (total_len_body as f64 / blocks.len() as f64).max(1.0);

        // 3. Compute document frequency df(q) and positive IDF
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        for bt in &block_tokens_list {
            let mut unique_in_block = HashSet::new();
            for t in &bt.name_tokens {
                unique_in_block.insert(t);
            }
            for t in &bt.path_tokens {
                unique_in_block.insert(t);
            }
            for t in &bt.sig_tokens {
                unique_in_block.insert(t);
            }
            for t in &bt.body_tokens {
                unique_in_block.insert(t);
            }

            for q in &query_tokens {
                if unique_in_block.contains(q) {
                    *doc_freq.entry(q.clone()).or_insert(0) += 1;
                }
            }
        }

        let idfs: HashMap<String, f64> = query_tokens
            .iter()
            .map(|q| {
                let n_q = doc_freq.get(q).copied().unwrap_or(0);
                let idf = ((n_docs as f64 - n_q as f64 + 0.5) / (n_q as f64 + 0.5) + 1.0)
                    .ln()
                    .max(0.1);
                (q.clone(), idf)
            })
            .collect();

        // 4. BM25F Scoring
        let mut candidates = Vec::new();
        let target_path_opt = options.target_path.as_deref();

        let is_legacy_mode = options
            .ranking_mode
            .as_deref()
            .is_some_and(|m| m == "legacy");

        for (idx, b) in blocks.into_iter().enumerate() {
            let bt = &block_tokens_list[idx];
            let name_lower = b.name.to_lowercase();
            let path_str = b.file_path.to_string_lossy().to_lowercase();

            // Graph proximity scoring
            let mut proximity_score = 0.0f64;
            if let Some(target) = target_path_opt {
                let target_rel = target.to_string_lossy().to_lowercase();
                if path_str == target_rel {
                    proximity_score += 6.0;
                } else if let Some(target_dir) = target.parent()
                    && b.file_path.starts_with(target_dir)
                {
                    proximity_score += 3.0;
                }
            }

            // Categorize symbol
            let is_error = name_lower.ends_with("error")
                || name_lower.contains("exception")
                || (b.kind == SymbolKind::Enum && name_lower.contains("err"));

            let category = if is_error {
                Category::Error
            } else {
                match b.kind {
                    SymbolKind::Trait | SymbolKind::Interface => Category::Interface,
                    SymbolKind::Struct
                    | SymbolKind::Class
                    | SymbolKind::Enum
                    | SymbolKind::TypeAlias => Category::Type,
                    _ => Category::Function,
                }
            };

            let category_bonus = match category {
                Category::Interface => 4.0,
                Category::Error => 3.5,
                Category::Type => 2.5,
                Category::Function => 1.0,
            };

            let mut bm25f_score = 0.0f64;

            if is_legacy_mode {
                // Legacy simple keyword matching baseline
                let mut legacy_score = 0.0f64;
                for kw in &query_tokens {
                    if name_lower == *kw {
                        legacy_score += 100.0;
                    } else if name_lower.contains(kw) || kw.contains(&name_lower) {
                        legacy_score += 40.0;
                    }
                }
                if task_lower.contains(&name_lower) {
                    legacy_score += 80.0;
                }
                if legacy_score > 0.0 {
                    bm25f_score = legacy_score + proximity_score + category_bonus;
                } else if proximity_score > 0.0 {
                    bm25f_score = proximity_score + category_bonus * 0.5;
                }
            } else {
                // Formal BM25F Multi-Field Calculation
                let norm_name =
                    (1.0 - b_name + b_name * (bt.name_tokens.len() as f64 / avg_len_name)).max(0.1);
                let norm_path =
                    (1.0 - b_path + b_path * (bt.path_tokens.len() as f64 / avg_len_path)).max(0.1);
                let norm_sig =
                    (1.0 - b_sig + b_sig * (bt.sig_tokens.len() as f64 / avg_len_sig)).max(0.1);
                let norm_body =
                    (1.0 - b_body + b_body * (bt.body_tokens.len() as f64 / avg_len_body)).max(0.1);

                for q in &query_tokens {
                    let idf = idfs.get(q).copied().unwrap_or(0.1);

                    let tf_name = bt.name_tokens.iter().filter(|t| *t == q).count() as f64;
                    let tf_path = bt.path_tokens.iter().filter(|t| *t == q).count() as f64;
                    let tf_sig = bt.sig_tokens.iter().filter(|t| *t == q).count() as f64;
                    let tf_body = bt.body_tokens.iter().filter(|t| *t == q).count() as f64;

                    let s_tilde = w_name * (tf_name / norm_name)
                        + w_path * (tf_path / norm_path)
                        + w_sig * (tf_sig / norm_sig)
                        + w_body * (tf_body / norm_body);

                    if s_tilde > 0.0 {
                        let term_score = idf * (s_tilde * (k1 + 1.0)) / (s_tilde + k1);
                        if term_score.is_finite() {
                            bm25f_score += term_score;
                        }
                    }
                }

                // Exact symbol name match in query
                if task_lower.contains(&name_lower) {
                    bm25f_score += 8.0;
                }

                // Exact qualified name match
                if task_lower.contains(&b.name.to_lowercase()) {
                    bm25f_score += 4.0;
                }

                if bm25f_score > 0.0 {
                    bm25f_score += proximity_score + category_bonus;
                } else if proximity_score > 0.0 {
                    bm25f_score = proximity_score + category_bonus * 0.5;
                }
            }

            if bm25f_score > 0.0 {
                candidates.push(HACSCandidate {
                    block: b,
                    category,
                    bm25f_score,
                    final_score: bm25f_score,
                    selection_reason: "lexical_bm25f",
                });
            }
        }

        // 5. Bounded Graph Expansion & PageRank (Configurable)
        let mut graph_expansion_applied = false;
        let mut test_discovery_applied = false;

        if options.enable_graph_expansion && !is_legacy_mode {
            let alpha = 0.30f64;
            let mut file_scores: HashMap<PathBuf, f64> = HashMap::new();
            for c in &candidates {
                *file_scores.entry(c.block.file_path.clone()).or_insert(0.0) += c.bm25f_score;
            }

            let total_init_score: f64 = file_scores.values().sum();
            if total_init_score > 0.0 {
                graph_expansion_applied = true;
                let p0: HashMap<PathBuf, f64> = file_scores
                    .iter()
                    .map(|(p, s)| (p.clone(), s / total_init_score))
                    .collect();

                let mut pt = p0.clone();

                for _step in 0..3 {
                    let mut next_p = HashMap::new();
                    let mut dangling_weight = 0.0f64;

                    for (file, &curr_prob) in &pt {
                        if curr_prob <= 0.0 {
                            continue;
                        }
                        let dependents = graph.get_downstream_dependents(file);
                        let dependencies = graph.get_dependencies(file);
                        let mut neighbors = dependents;
                        neighbors.extend(dependencies);

                        if let Some(node) = graph.files.get(file) {
                            for imp in &node.imports {
                                for token in subtokenize_identifier(imp) {
                                    if let Some(targets) =
                                        graph.stem_index.get(&token.to_lowercase())
                                    {
                                        for target in targets {
                                            if target != file && !neighbors.contains(target) {
                                                neighbors.push(target.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if neighbors.is_empty() {
                            dangling_weight += (1.0 - alpha) * curr_prob;
                        } else {
                            let deg = neighbors.len() as f64;
                            let share = ((1.0 - alpha) * curr_prob) / deg;
                            for neighbor in neighbors {
                                *next_p.entry(neighbor).or_insert(0.0) += share;
                            }
                        }
                    }

                    // Distribute dangling node mass and teleportation alpha * p0
                    for (file, &seed_prob) in &p0 {
                        *next_p.entry(file.clone()).or_insert(0.0) +=
                            alpha * seed_prob + dangling_weight * seed_prob;
                    }
                    pt = next_p;
                }

                // 6. Companion-test discovery (O(1) test_by_stem and stem_index lookups)
                if options.enable_test_discovery {
                    let mut test_boosts = HashMap::new();
                    for (file, &score) in &pt {
                        if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
                            let lower_stem = stem.to_lowercase();
                            if VectorGraph::is_test_file(file) {
                                let core = VectorGraph::extract_core_test_stem(&lower_stem);
                                if let Some(sources) = graph.stem_index.get(&core) {
                                    for sf in sources {
                                        if sf != file {
                                            *test_boosts.entry(sf.clone()).or_insert(0.0) +=
                                                score * 0.30;
                                            test_discovery_applied = true;
                                        }
                                    }
                                }
                            } else {
                                if let Some(tests) = graph.test_by_stem.get(&lower_stem) {
                                    for tf in tests {
                                        if tf != file {
                                            *test_boosts.entry(tf.clone()).or_insert(0.0) +=
                                                score * 0.30;
                                            test_discovery_applied = true;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    for (file, boost) in test_boosts {
                        *pt.entry(file).or_insert(0.0) += boost;
                    }
                }

                // Combine BM25F and PPR into final_score
                for c in &mut candidates {
                    let ppr = pt.get(&c.block.file_path).copied().unwrap_or(0.0);
                    if ppr > 0.0 {
                        c.final_score = c.bm25f_score * (1.0 + 0.5 * ppr);
                        if VectorGraph::is_test_file(&c.block.file_path) {
                            c.selection_reason = "companion_test";
                        } else {
                            c.selection_reason = "graph_expansion";
                        }
                    }
                }
            }
        }

        // Sort candidates descending by final_score, then file, then line
        candidates.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.block.file_path.cmp(&b.block.file_path))
                .then_with(|| a.block.start_line.cmp(&b.block.start_line))
        });

        // Deduplicate candidates
        let mut seen_symbols = HashSet::new();
        let mut filtered_candidates = Vec::new();
        for c in candidates {
            let key = (c.block.name.clone(), c.block.file_path.clone());
            if seen_symbols.insert(key) {
                filtered_candidates.push(c);
            }
        }

        // Collect top relevant files (capped at options.max_files, default 10)
        let mut ordered_relevant_files: Vec<String> = Vec::new();
        for c in &filtered_candidates {
            let f = c.block.file_path.display().to_string();
            if !ordered_relevant_files.contains(&f) {
                ordered_relevant_files.push(f);
            }
            if ordered_relevant_files.len() >= options.max_files {
                break;
            }
        }

        // 7. Holographic Context Slicing (HACS):
        // Up to 4,800 characters budget for inline line-numbered source code
        let hologram_char_budget = 4800usize;
        let mut hologram_buf = String::with_capacity(hologram_char_budget);
        let mut code_slices = Vec::new();
        let mut inlined_line_spans: Vec<(PathBuf, usize, usize)> = Vec::new();
        let mut hot_inlined_files = HashSet::new();

        // Inline top hot blocks (complete relevant functions/classes)
        for (slice_idx, candidate) in filtered_candidates
            .iter()
            .take(options.max_hot_slices)
            .enumerate()
        {
            if std::time::Instant::now() >= deadline {
                break;
            }
            if let Some(file_node) = graph.files.get(&candidate.block.file_path) {
                // Ensure no redundant overlapping spans:
                // If this symbol is completely enclosed within an already inlined slice in the same file, skip emitting duplicate code
                let is_enclosed = inlined_line_spans.iter().any(|(p, s, e)| {
                    p == &candidate.block.file_path
                        && *s <= candidate.block.start_line
                        && *e >= candidate.block.end_line
                });

                if is_enclosed {
                    continue;
                }

                let file_lines: Vec<&str> = file_node.content.lines().collect();
                let start_idx = candidate.block.start_line.saturating_sub(1);
                let end_idx = candidate.block.end_line.min(file_lines.len());

                if start_idx < end_idx {
                    let slice_end_idx = end_idx.min(start_idx + 60);
                    let mut slice_code = String::new();

                    for (l_idx, &line_text) in file_lines
                        .iter()
                        .enumerate()
                        .take(slice_end_idx)
                        .skip(start_idx)
                    {
                        let l_num = l_idx + 1;
                        slice_code.push_str(&format!("L{}: {}\n", l_num, line_text));
                    }

                    let header = format!(
                        "// --- [HOLOGRAPHIC SLICE {}] {} (L{}-L{}) [{}] ---\n",
                        slice_idx + 1,
                        candidate.block.file_path.display(),
                        candidate.block.start_line,
                        slice_end_idx,
                        candidate.block.provenance
                    );

                    if hologram_buf.len() + header.len() + slice_code.len() <= hologram_char_budget
                    {
                        hologram_buf.push_str(&header);
                        hologram_buf.push_str(&slice_code);
                        hologram_buf.push('\n');

                        code_slices.push(HologramSlice {
                            file: candidate.block.file_path.display().to_string(),
                            start_line: candidate.block.start_line,
                            end_line: slice_end_idx,
                            code: slice_code,
                            score: candidate.final_score,
                            category: "hot".to_string(),
                            provenance: candidate.block.provenance.to_string(),
                            reason: candidate.selection_reason.to_string(),
                        });
                        inlined_line_spans.push((
                            candidate.block.file_path.clone(),
                            candidate.block.start_line,
                            slice_end_idx,
                        ));
                        hot_inlined_files.insert(candidate.block.file_path.clone());
                    }
                }
            }
        }

        // 8. Warm spans: skeletal stubs for secondary symbols in hot files
        let mut warm_slices_count = 0usize;
        for hot_file in &hot_inlined_files {
            if std::time::Instant::now() >= deadline {
                break;
            }
            if let Some(file_node) = graph.files.get(hot_file) {
                for sym in &file_node.symbols {
                    let already_inlined = inlined_line_spans.iter().any(|(p, s, e)| {
                        p == hot_file && *s <= sym.start_line && *e >= sym.end_line
                    });

                    if !already_inlined {
                        let stub = format!(
                            "// [Warm Stub] L{}-L{}: {} {{ ... }}\n",
                            sym.start_line, sym.end_line, sym.signature
                        );
                        if hologram_buf.len() + stub.len() <= hologram_char_budget {
                            hologram_buf.push_str(&stub);
                            warm_slices_count += 1;
                        }
                    }
                }
            }
        }

        let hologram = if !hologram_buf.is_empty() {
            format!("### Holographic Source Slices\n```\n{}```", hologram_buf)
        } else {
            String::new()
        };

        // 9. Structured items and budget enforcement
        let mut interfaces = Vec::new();
        let mut types = Vec::new();
        let mut errors = Vec::new();
        let mut imports_set = HashSet::new();
        let mut relevant_files_set = HashSet::new();

        let mut estimated_tokens = 40; // baseline JSON structure tokens
        let mut is_truncated = false;
        let mut omitted_count = 0;
        let mut omitted_symbols = Vec::new();

        for candidate in filtered_candidates {
            let item_chars = candidate.block.signature.len()
                + candidate.block.name.len()
                + candidate.block.file_path.to_string_lossy().len()
                + candidate
                    .block
                    .doc_comment
                    .as_ref()
                    .map(|d| d.len())
                    .unwrap_or(0)
                + 60;
            let item_tokens = item_chars.div_ceil(4);

            if estimated_tokens + item_tokens > options.max_tokens {
                is_truncated = true;
                omitted_count += 1;
                omitted_symbols.push(candidate.block.name);
                continue;
            }

            estimated_tokens += item_tokens;
            relevant_files_set.insert(candidate.block.file_path.clone());

            let provenance = format!(
                "get_lines(path='{}', start={}, end={}) [provenance={}]",
                candidate.block.file_path.display(),
                candidate.block.start_line,
                candidate.block.end_line,
                candidate.block.provenance
            );

            let item = TaskContextItem {
                symbol: candidate.block.name,
                kind: candidate.block.kind.to_string(),
                file: candidate.block.file_path.display().to_string(),
                lines: [candidate.block.start_line, candidate.block.end_line],
                signature: candidate.block.signature,
                doc: candidate.block.doc_comment,
                provenance,
            };

            match candidate.category {
                Category::Interface => interfaces.push(item),
                Category::Error => errors.push(item),
                Category::Type => types.push(item),
                Category::Function => types.push(item),
            }
        }

        // Collect imports from files containing relevant types
        for file_path in &relevant_files_set {
            if let Some(node) = graph.files.get(file_path) {
                for imp in &node.imports {
                    imports_set.insert(imp.clone());
                }
            }
        }

        let mut imports: Vec<String> = imports_set.into_iter().collect();
        imports.sort();

        // Check for unresolved dependencies
        let mut unresolved = Vec::new();
        for imp in &imports {
            if (imp.starts_with("crate::") || imp.starts_with("use crate::"))
                && let Some(sub) = imp.split("::").nth(1)
            {
                let clean_sub = sub.trim_end_matches(';').trim();
                let expected_rs = format!("src/{}.rs", clean_sub);
                let expected_mod = format!("src/{}/mod.rs", clean_sub);
                let found = graph.files.keys().any(|p| {
                    let s = p.to_string_lossy();
                    s == expected_rs || s == expected_mod
                });
                if !found {
                    unresolved.push(format!(
                        "Module '{}' imported via '{}' was not found in workspace",
                        clean_sub, imp
                    ));
                }
            }
        }

        let mut arch_dirs = HashSet::new();
        for p in graph.files.keys() {
            if let Some(parent) = p.parent() {
                let parent_str = parent.display().to_string();
                if !parent_str.is_empty() && parent_str != "." {
                    arch_dirs.insert(parent_str);
                }
            }
        }
        let mut architecture_overview: Vec<String> = arch_dirs.into_iter().collect();
        architecture_overview.sort();
        architecture_overview.truncate(15);

        let directive = Some(
            "HOLOGRAM_READY: Exact line-numbered source code for top matches is inlined above. Edit directly without re-reading these spans via get_lines or rg."
                .to_string(),
        );

        let execution_ms = start_time.elapsed().as_millis();
        let hot_slices_count = code_slices.len();

        let diagnostics = Some(TaskContextDiagnostics {
            total_indexed_files: graph.files.len(),
            total_indexed_symbols: total_symbols,
            matched_query_tokens: query_tokens,
            graph_expansion_applied,
            test_discovery_applied,
            hot_slices_count,
            warm_slices_count,
            omitted_cold_count: omitted_count,
            execution_ms,
        });

        TaskContextPacket {
            task: task.to_string(),
            relevant_files: ordered_relevant_files,
            architecture_overview,
            interfaces,
            types,
            errors,
            imports,
            unresolved_dependencies: unresolved,
            budget: TaskContextBudget {
                estimated_tokens,
                max_tokens: options.max_tokens,
                is_truncated,
                omitted_count,
                omitted_symbols,
            },
            hologram,
            code_slices,
            directive,
            diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FileNode, Symbol, SymbolKind};
    use std::path::PathBuf;

    fn make_test_node(
        path: &str,
        content: &str,
        symbols: Vec<Symbol>,
        imports: Vec<String>,
    ) -> FileNode {
        let p = PathBuf::from(path);
        FileNode {
            relative_path: p.clone(),
            absolute_path: p,
            symbols,
            imports,
            content: content.to_string(),
            last_modified: None,
            content_hash: 42,
            size_bytes: content.len() as u64,
            line_count: content.lines().count(),
            extension: Some("rs".to_string()),
            vector: Vec::new(),
        }
    }

    #[test]
    fn test_compile_task_context_resolves_and_categorizes_cleanly() {
        let mut graph = VectorGraph::new();

        let sym_repo = Symbol {
            name: "UserRepository".to_string(),
            kind: SymbolKind::Trait,
            start_line: 1,
            end_line: 10,
            signature: "pub trait UserRepository".to_string(),
            doc_comment: Some("Repository for users".to_string()),
        };
        let sym_user = Symbol {
            name: "User".to_string(),
            kind: SymbolKind::Struct,
            start_line: 12,
            end_line: 20,
            signature: "pub struct User".to_string(),
            doc_comment: None,
        };
        let sym_err = Symbol {
            name: "RepositoryError".to_string(),
            kind: SymbolKind::Enum,
            start_line: 22,
            end_line: 30,
            signature: "pub enum RepositoryError".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node(
            "src/repository.rs",
            "pub trait UserRepository {}\npub struct User {}\npub enum RepositoryError {}\n",
            vec![sym_repo, sym_user, sym_err],
            vec!["use std::sync::Arc;".to_string()],
        ));

        let packet = ContextCompiler::compile_task_context(
            &graph,
            "implement postgres UserRepository for User with RepositoryError handling",
            Some(Path::new("src/repository.rs")),
            2000,
        );

        assert_eq!(packet.interfaces.len(), 1);
        assert_eq!(packet.interfaces[0].symbol, "UserRepository");
        assert_eq!(packet.interfaces[0].kind, "trait");

        assert_eq!(packet.types.len(), 1);
        assert_eq!(packet.types[0].symbol, "User");

        assert_eq!(packet.errors.len(), 1);
        assert_eq!(packet.errors[0].symbol, "RepositoryError");

        assert!(packet.budget.estimated_tokens <= 2000);
        assert!(!packet.budget.is_truncated);
        assert!(packet.directive.is_some());
        assert!(packet.diagnostics.is_some());
    }

    #[test]
    fn test_context_budgeting_enforces_ceiling_and_reports_omissions() {
        let mut graph = VectorGraph::new();

        let mut syms = Vec::new();
        for i in 1..=10 {
            syms.push(Symbol {
                name: format!("UserDataModel{}", i),
                kind: SymbolKind::Struct,
                start_line: i * 5,
                end_line: i * 5 + 4,
                signature: format!("pub struct UserDataModel{} {{ pub id: u64 }}", i),
                doc_comment: None,
            });
        }

        graph.upsert_file(make_test_node(
            "src/models.rs",
            "struct dummy;",
            syms,
            vec![],
        ));

        let packet = ContextCompiler::compile_task_context(
            &graph,
            "UserDataModel",
            Some(Path::new("src/models.rs")),
            100,
        );

        assert!(packet.budget.is_truncated);
        assert!(packet.budget.omitted_count > 0);
        assert!(!packet.budget.omitted_symbols.is_empty());
        assert!(packet.budget.estimated_tokens <= 120);
    }

    #[test]
    fn test_unresolved_dependencies_detection() {
        let mut graph = VectorGraph::new();

        let sym = Symbol {
            name: "OrderService".to_string(),
            kind: SymbolKind::Struct,
            start_line: 1,
            end_line: 10,
            signature: "pub struct OrderService".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node(
            "src/service.rs",
            "pub struct OrderService;",
            vec![sym],
            vec!["use crate::non_existent_module::Foo;".to_string()],
        ));

        let packet = ContextCompiler::compile_task_context(
            &graph,
            "OrderService",
            Some(Path::new("src/service.rs")),
            1000,
        );

        assert!(!packet.unresolved_dependencies.is_empty());
        assert!(packet.unresolved_dependencies[0].contains("non_existent_module"));
    }

    #[test]
    fn test_bm25f_empty_index_and_empty_query() {
        let graph = VectorGraph::new();
        let packet = ContextCompiler::compile_task_context(&graph, "", None, 1400);
        assert!(packet.code_slices.is_empty());
        assert!(packet.hologram.is_empty());
        assert_eq!(packet.relevant_files.len(), 0);
    }

    #[test]
    fn test_configurable_graph_expansion_and_test_discovery_flags() {
        let mut graph = VectorGraph::new();

        let sym_main = Symbol {
            name: "PaymentProcessor".to_string(),
            kind: SymbolKind::Struct,
            start_line: 1,
            end_line: 20,
            signature: "pub struct PaymentProcessor".to_string(),
            doc_comment: None,
        };
        let sym_test = Symbol {
            name: "test_payment_processor".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 15,
            signature: "pub fn test_payment_processor()".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node(
            "src/payment.rs",
            "pub struct PaymentProcessor;",
            vec![sym_main],
            vec![],
        ));
        graph.upsert_file(make_test_node(
            "tests/payment_test.rs",
            "pub fn test_payment_processor() {}",
            vec![sym_test],
            vec![],
        ));

        // 1. Without graph expansion & without test discovery (Variant C: HACS Lexical)
        let opt_lexical = TaskContextOptions {
            max_tokens: 1400,
            target_path: None,
            enable_graph_expansion: false,
            enable_test_discovery: false,
            max_hot_slices: 5,
            max_files: 10,
            ranking_mode: Some("bm25f".to_string()),
        };
        let p_lexical = ContextCompiler::compile_task_context_with_options(
            &graph,
            "PaymentProcessor",
            &opt_lexical,
        );
        let diag_lexical = p_lexical.diagnostics.unwrap();
        assert!(!diag_lexical.graph_expansion_applied);
        assert!(!diag_lexical.test_discovery_applied);

        // 2. With graph expansion & test discovery (Variant D: HACS Graph)
        let opt_graph = TaskContextOptions {
            max_tokens: 1400,
            target_path: None,
            enable_graph_expansion: true,
            enable_test_discovery: true,
            max_hot_slices: 5,
            max_files: 10,
            ranking_mode: Some("bm25f".to_string()),
        };
        let p_graph = ContextCompiler::compile_task_context_with_options(
            &graph,
            "PaymentProcessor",
            &opt_graph,
        );
        let diag_graph = p_graph.diagnostics.unwrap();
        assert!(diag_graph.graph_expansion_applied);
        assert!(diag_graph.test_discovery_applied);
    }
}
