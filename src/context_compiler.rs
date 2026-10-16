use crate::graph::VectorGraph;
use crate::models::{Symbol, SymbolKind};
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Interface,
    Error,
    Type,
    Function,
}

struct HACSCandidate {
    symbol: Symbol,
    file_path: PathBuf,
    category: Category,
    bm25f_score: f64,
    final_score: f64,
}

/// Splits any identifier, symbol, or text into sub-tokens along camelCase,
/// PascalCase, snake_case, and acronym boundaries, preserving compounds.
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

    // Preserve compound identifiers containing separators (e.g. calculate_crc32_checksum, get_lines)
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
        // Final check on curr for run-together acronyms like XMLHTTP
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
    /// Compiles a task-specific, token-bounded context packet using HACS (BM25F + PPR + Holographic Slicing)
    pub fn compile_task_context(
        graph: &VectorGraph,
        task: &str,
        target_path: Option<&Path>,
        max_tokens: usize,
    ) -> TaskContextPacket {
        let query_tokens = subtokenize_identifier(task);
        let task_lower = task.to_lowercase();

        // 1. Compute document frequency n(q) across workspace for BM25F IDF calculation
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let mut total_symbols = 0usize;

        for (path, node) in &graph.files {
            let path_tokens: HashSet<String> = subtokenize_identifier(&path.to_string_lossy())
                .into_iter()
                .collect();
            for sym in &node.symbols {
                total_symbols += 1;
                let sym_tokens: HashSet<String> =
                    subtokenize_identifier(&sym.name).into_iter().collect();
                for q in &query_tokens {
                    if sym_tokens.contains(q) || path_tokens.contains(q) {
                        *doc_freq.entry(q.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        let n_docs = total_symbols.max(graph.files.len()).max(1);
        let idfs: HashMap<String, f64> = query_tokens
            .iter()
            .map(|q| {
                let n_q = doc_freq.get(q).copied().unwrap_or(0);
                // BM25 positive IDF
                let idf = ((n_docs as f64 - n_q as f64 + 0.5) / (n_q as f64 + 0.5) + 1.0)
                    .ln()
                    .max(0.1);
                (q.clone(), idf)
            })
            .collect();

        // 2. Score candidates with BM25F across fields
        // Field weights: name = 6.0, path = 3.5, signature = 2.5, body/doc = 1.0
        let w_name = 6.0f64;
        let w_path = 3.5f64;
        let w_sig = 2.5f64;
        let w_body = 1.0f64;
        let k1 = 1.2f64;

        let mut candidates = Vec::new();

        for (path, node) in &graph.files {
            let path_str = path.to_string_lossy().to_lowercase();
            let path_tokens = subtokenize_identifier(&path_str);

            // Graph proximity scoring
            let mut proximity_score = 0.0f64;
            if let Some(target) = target_path {
                let target_rel = target.to_string_lossy().to_lowercase();
                if path_str == target_rel {
                    proximity_score += 6.0;
                } else if let Some(target_dir) = target.parent()
                    && path.starts_with(target_dir)
                {
                    proximity_score += 3.0;
                }
            }

            for sym in &node.symbols {
                let name_lower = sym.name.to_lowercase();
                let name_tokens = subtokenize_identifier(&sym.name);
                let sig_tokens = subtokenize_identifier(&sym.signature);
                let doc_tokens = sym
                    .doc_comment
                    .as_deref()
                    .map(subtokenize_identifier)
                    .unwrap_or_default();

                let mut bm25f = 0.0f64;
                for q in &query_tokens {
                    let idf = idfs.get(q).copied().unwrap_or(0.1);

                    let c_name = name_tokens.iter().filter(|t| *t == q).count() as f64;
                    let c_path = path_tokens.iter().filter(|t| *t == q).count() as f64;
                    let c_sig = sig_tokens.iter().filter(|t| *t == q).count() as f64;
                    let c_body = doc_tokens.iter().filter(|t| *t == q).count() as f64;

                    let s_tilde =
                        w_name * c_name + w_path * c_path + w_sig * c_sig + w_body * c_body;
                    if s_tilde > 0.0 {
                        bm25f += idf * (s_tilde * (k1 + 1.0)) / (s_tilde + k1);
                    }
                }

                // Substring / direct phrase bonus
                if task_lower.contains(&name_lower) {
                    bm25f += 8.0;
                }

                // Categorize symbol
                let is_error = name_lower.ends_with("error")
                    || name_lower.contains("exception")
                    || (sym.kind == SymbolKind::Enum && name_lower.contains("err"));

                let category = if is_error {
                    Category::Error
                } else {
                    match sym.kind {
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

                let total_bm25f = bm25f + proximity_score + category_bonus;

                if bm25f > 0.0 || (proximity_score > 0.0 && category != Category::Function) {
                    candidates.push(HACSCandidate {
                        symbol: sym.clone(),
                        file_path: path.clone(),
                        category,
                        bm25f_score: total_bm25f,
                        final_score: total_bm25f,
                    });
                }
            }
        }

        // 3. 3-step Personalized PageRank (PPR) with alpha = 0.30 over graph edges
        let alpha = 0.30f64;
        let mut file_scores: HashMap<PathBuf, f64> = HashMap::new();
        for c in &candidates {
            *file_scores.entry(c.file_path.clone()).or_insert(0.0) += c.bm25f_score;
        }

        let total_init_score: f64 = file_scores.values().sum();
        if total_init_score > 0.0 {
            // Seed probability p0
            let p0: HashMap<PathBuf, f64> = file_scores
                .iter()
                .map(|(p, s)| (p.clone(), s / total_init_score))
                .collect();

            let mut pt = p0.clone();

            for _step in 0..3 {
                let mut next_p = HashMap::new();
                for (file, &curr_prob) in &pt {
                    if curr_prob <= 0.0 {
                        continue;
                    }
                    let dependents = graph.get_downstream_dependents(file);
                    let mut neighbors = dependents;
                    if let Some(node) = graph.files.get(file) {
                        for imp in &node.imports {
                            // Find matching file in graph
                            for target in graph.files.keys() {
                                if imp.contains(
                                    &target
                                        .file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string(),
                                ) {
                                    neighbors.push(target.clone());
                                }
                            }
                        }
                    }

                    let deg = neighbors.len().max(1) as f64;
                    let share = ((1.0 - alpha) * curr_prob) / deg;

                    for neighbor in neighbors {
                        *next_p.entry(neighbor).or_insert(0.0) += share;
                    }
                }

                // Add teleportation alpha * p0
                for (file, &seed_prob) in &p0 {
                    *next_p.entry(file.clone()).or_insert(0.0) += alpha * seed_prob;
                }
                pt = next_p;
            }

            // Suffix-based test file companion pairing boost (+30%)
            let mut test_boosts = HashMap::new();
            for (file, &score) in &pt {
                let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                for other in graph.files.keys() {
                    if other == file {
                        continue;
                    }
                    let other_str = other.to_string_lossy();
                    let is_companion = (VectorGraph::is_test_file(other)
                        && other_str.contains(stem.as_ref()))
                        || (VectorGraph::is_test_file(file)
                            && stem.contains(
                                &other
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            ));
                    if is_companion {
                        *test_boosts.entry(other.clone()).or_insert(0.0) += score * 0.30;
                    }
                }
            }

            for (file, boost) in test_boosts {
                *pt.entry(file).or_insert(0.0) += boost;
            }

            // Combine BM25F and PPR into final_score
            for c in &mut candidates {
                let ppr = pt.get(&c.file_path).copied().unwrap_or(0.0);
                c.final_score = c.bm25f_score * (1.0 + 0.5 * ppr);
            }
        }

        // Sort candidates descending by final_score
        candidates.sort_by(|a, b| {
            b.final_score
                .partial_cmp(&a.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.symbol.start_line.cmp(&b.symbol.start_line))
        });

        // Deduplicate candidates
        let mut seen_symbols = HashSet::new();
        let mut filtered_candidates = Vec::new();
        for c in candidates {
            let key = (c.symbol.name.clone(), c.file_path.clone());
            if seen_symbols.insert(key) {
                filtered_candidates.push(c);
            }
        }

        // 4. Collect top relevant files (capped at 10)
        let mut ordered_relevant_files: Vec<String> = Vec::new();
        for c in &filtered_candidates {
            let f = c.file_path.display().to_string();
            if !ordered_relevant_files.contains(&f) {
                ordered_relevant_files.push(f);
            }
            if ordered_relevant_files.len() >= 10 {
                break;
            }
        }

        // 5. Holographic Context Slicing (HACS):
        // Up to 4,800 characters budget for inline line-numbered source code
        let hologram_char_budget = 4800usize;
        let mut hologram_buf = String::with_capacity(hologram_char_budget);
        let mut code_slices = Vec::new();
        let mut hot_inlined_files = HashSet::new();

        // Inline top hot blocks
        for (slice_idx, candidate) in filtered_candidates.iter().take(5).enumerate() {
            if let Some(file_node) = graph.files.get(&candidate.file_path) {
                let file_lines: Vec<&str> = file_node.content.lines().collect();
                let start_idx = candidate.symbol.start_line.saturating_sub(1);
                let end_idx = candidate.symbol.end_line.min(file_lines.len());

                if start_idx < end_idx {
                    // Cap slice length at 50 lines to prevent blowing out budget
                    let slice_end_idx = end_idx.min(start_idx + 50);
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
                        "// --- [HOLOGRAPHIC SLICE {}] {} (L{}-L{}) ---\n",
                        slice_idx + 1,
                        candidate.file_path.display(),
                        candidate.symbol.start_line,
                        slice_end_idx
                    );

                    if hologram_buf.len() + header.len() + slice_code.len() <= hologram_char_budget
                    {
                        hologram_buf.push_str(&header);
                        hologram_buf.push_str(&slice_code);
                        hologram_buf.push('\n');

                        code_slices.push(HologramSlice {
                            file: candidate.file_path.display().to_string(),
                            start_line: candidate.symbol.start_line,
                            end_line: slice_end_idx,
                            code: slice_code,
                            score: candidate.final_score,
                        });
                        hot_inlined_files.insert(candidate.file_path.clone());
                    }
                }
            }
        }

        // Skeletal stubs for surrounding symbols in hot files
        for hot_file in &hot_inlined_files {
            if let Some(file_node) = graph.files.get(hot_file) {
                for sym in &file_node.symbols {
                    let already_inlined = code_slices.iter().any(|s| {
                        s.file == hot_file.display().to_string()
                            && s.start_line <= sym.start_line
                            && s.end_line >= sym.end_line
                    });

                    if !already_inlined {
                        let stub = format!(
                            "// [Skeletal Stub] L{}-L{}: {} {{ ... }}\n",
                            sym.start_line, sym.end_line, sym.signature
                        );
                        if hologram_buf.len() + stub.len() <= hologram_char_budget {
                            hologram_buf.push_str(&stub);
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

        // 6. Build structured items and enforce token budgeting
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
            let item_chars = candidate.symbol.signature.len()
                + candidate.symbol.name.len()
                + candidate.file_path.to_string_lossy().len()
                + candidate
                    .symbol
                    .doc_comment
                    .as_ref()
                    .map(|d| d.len())
                    .unwrap_or(0)
                + 60;
            let item_tokens = item_chars.div_ceil(4);

            if estimated_tokens + item_tokens > max_tokens {
                is_truncated = true;
                omitted_count += 1;
                omitted_symbols.push(candidate.symbol.name);
                continue;
            }

            estimated_tokens += item_tokens;
            relevant_files_set.insert(candidate.file_path.clone());

            let provenance = format!(
                "get_lines(path='{}', start={}, end={})",
                candidate.file_path.display(),
                candidate.symbol.start_line,
                candidate.symbol.end_line
            );

            let item = TaskContextItem {
                symbol: candidate.symbol.name,
                kind: candidate.symbol.kind.to_string(),
                file: candidate.file_path.display().to_string(),
                lines: [candidate.symbol.start_line, candidate.symbol.end_line],
                signature: candidate.symbol.signature,
                doc: candidate.symbol.doc_comment,
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
                max_tokens,
                is_truncated,
                omitted_count,
                omitted_symbols,
            },
            hologram,
            code_slices,
            directive,
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
}
