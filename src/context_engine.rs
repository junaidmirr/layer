use crate::graph::VectorGraph;
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

static LINE_RANGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:lines?|from line|l)\s*(\d+)\s*(?:to|-|through|..)\s*(\d+)").unwrap()
});

static SINGLE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:line|l)\s*(\d+)").unwrap());

pub enum QueryAction {
    Tree,
    FileContent {
        path: PathBuf,
    },
    FileLines {
        path: PathBuf,
        start: usize,
        end: usize,
    },
    FileFunctions {
        path: PathBuf,
    },
    History {
        path: Option<PathBuf>,
    },
    HistoryDetail {
        id: usize,
    },
    SemanticSearch {
        query: String,
    },
    Help,
}

pub struct ContextEngine;

impl ContextEngine {
    /// Parse natural user queries like:
    /// - "give me file structure" -> Tree
    /// - "this files code src/main.rs" -> FileContent
    /// - "all functions in src/scanner.rs" -> FileFunctions
    /// - "lines 10 to 45 in src/graph.rs" -> FileLines
    /// - "how is vector search calculated" -> SemanticSearch
    pub fn parse_intent(graph: &VectorGraph, prompt: &str) -> QueryAction {
        let trimmed = prompt.trim();
        let lower = trimmed.to_lowercase();

        // 1. Structure / Tree query
        if lower.contains("file structure")
            || lower.contains("folder structure")
            || lower.contains("directory structure")
            || lower == "tree"
            || lower == "structure"
            || lower == "files"
            || lower.contains("show me all files")
            || lower.contains("give me project tree")
        {
            return QueryAction::Tree;
        }

        // 2. Specific history detail or diff by ID (e.g. "history #1", "diff 2", "diff #3")
        if lower.starts_with("diff #")
            || lower.starts_with("diff ")
            || lower.starts_with("history #")
        {
            for token in trimmed.split_whitespace() {
                let cleaned = token.trim_start_matches('#');
                if let Ok(id) = cleaned.parse::<usize>() {
                    return QueryAction::HistoryDetail { id };
                }
            }
        }

        // 3. History / Changes / Modified / Deleted / Diff query
        if lower.contains("history")
            || lower.contains("changes")
            || lower.contains("changelog")
            || lower.contains("modified files")
            || lower.contains("deleted files")
            || lower.contains("what was changed")
            || lower.contains("what changed")
            || lower.contains("what happened")
            || lower == "diff"
        {
            let target = extract_file_target(graph, trimmed);
            return QueryAction::History { path: target };
        }

        // 2. Line range queries (e.g., "lines 10 to 40 in main.rs" or "lines from 5 to 20")
        if let Some(caps) = LINE_RANGE_RE.captures(&lower) {
            let start: usize = caps
                .get(1)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(1);
            let end: usize = caps
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(start);

            // Find target file mentioned in prompt
            if let Some(target_file) = extract_file_target(graph, trimmed) {
                return QueryAction::FileLines {
                    path: target_file,
                    start,
                    end,
                };
            }
        }

        // 3. Function / Symbol queries (e.g., "all functions in main.rs", "give me functions of embedder.rs")
        if (lower.contains("function") || lower.contains("methods") || lower.contains("symbols"))
            && let Some(target_file) = extract_file_target(graph, trimmed)
        {
            return QueryAction::FileFunctions { path: target_file };
        }

        // 4. File content query (e.g. "this files code src/main.rs", "cat src/models.rs", "show code of graph.rs")
        if (lower.contains("code")
            || lower.contains("content")
            || lower.starts_with("cat ")
            || lower.starts_with("file ")
            || lower.starts_with("show "))
            && let Some(target_file) = extract_file_target(graph, trimmed)
        {
            return QueryAction::FileContent { path: target_file };
        }

        // 5. If prompt contains a recognized filename directly
        if let Some(target_file) = extract_file_target(graph, trimmed) {
            // Check if prompt specifically mentions line numbers
            if let Some(caps) = SINGLE_LINE_RE.captures(&lower) {
                let line_no: usize = caps
                    .get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(1);
                return QueryAction::FileLines {
                    path: target_file,
                    start: line_no.saturating_sub(5).max(1),
                    end: line_no + 10,
                };
            }
            return QueryAction::FileContent { path: target_file };
        }

        // 6. Default to semantic vector search
        QueryAction::SemanticSearch {
            query: trimmed.to_string(),
        }
    }

    /// Execute the parsed query against the VectorGraph and generate a formatted context response
    pub fn execute_query(graph: &VectorGraph, prompt: &str) -> String {
        let action = Self::parse_intent(graph, prompt);
        match action {
            QueryAction::Tree => graph.format_tree(),
            QueryAction::FileContent { path } => match graph.get_file_content(&path) {
                Some(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let mut out = format!("📄 File: {} ({} lines)\n", path.display(), lines.len());
                    out.push_str("----------------------------------------\n");
                    for (idx, line) in lines.iter().enumerate() {
                        out.push_str(&format!("{:4} | {}\n", idx + 1, line));
                    }
                    out
                }
                None => format!("File not found in index: {}", path.display()),
            },
            QueryAction::FileLines { path, start, end } => {
                match graph.get_file_lines(&path, start, end) {
                    Some(slice) => {
                        format!(
                            "📄 Lines {} to {} of {}:\n----------------------------------------\n{}",
                            start,
                            end,
                            path.display(),
                            slice
                        )
                    }
                    None => format!("File not found in index: {}", path.display()),
                }
            }
            QueryAction::FileFunctions { path } => match graph.get_file_symbols(&path) {
                Some(symbols) => {
                    let mut out = format!("⚡ Symbols & Functions in {}:\n", path.display());
                    out.push_str("----------------------------------------\n");
                    if symbols.is_empty() {
                        out.push_str("No functions or symbols detected.\n");
                    } else {
                        for sym in symbols {
                            out.push_str(&format!(
                                "  • [{}] {} (Lines {}..{})\n    Signature: {}\n",
                                sym.kind, sym.name, sym.start_line, sym.end_line, sym.signature
                            ));
                            if let Some(doc) = &sym.doc_comment {
                                out.push_str(&format!("    Doc: {}\n", doc.replace('\n', " ")));
                            }
                        }
                    }
                    out
                }
                None => format!("File not found in index: {}", path.display()),
            },
            QueryAction::SemanticSearch { query } => {
                let hits = graph.search(&query, 5);
                if hits.is_empty() {
                    format!("No semantic vector matches found for: '{}'", query)
                } else {
                    let mut out = format!("🔍 Semantic Vector Hits for: '{}'\n", query);
                    out.push_str("========================================\n\n");
                    for (i, hit) in hits.iter().enumerate() {
                        let symbol_info = hit
                            .symbol_name
                            .as_deref()
                            .map(|s| format!(" [Symbol: {}]", s))
                            .unwrap_or_default();
                        out.push_str(&format!(
                            "{}. {} (Lines {}..{}) - Similarity: {:.3}{}\n",
                            i + 1,
                            hit.file_path.display(),
                            hit.start_line,
                            hit.end_line,
                            hit.score,
                            symbol_info
                        ));
                        out.push_str("----------------------------------------\n");
                        for line in hit.snippet.lines().take(15) {
                            out.push_str(&format!("   {}\n", line));
                        }
                        if hit.snippet.lines().count() > 15 {
                            out.push_str("   ...\n");
                        }
                        out.push('\n');
                    }
                    out
                }
            }
            QueryAction::History { path } => graph.history.format_history(path.as_deref()),
            QueryAction::HistoryDetail { id } => match graph.history.format_record_detail(id) {
                Some(detail) => detail,
                None => format!("History event #{} not found.", id),
            },
            QueryAction::Help => "Available commands / query formats:
  - 'tree' or 'give me file structure'
  - 'history' or 'what changed' or 'changes in <file>'
  - 'diff #<id>' or 'history #<id>'
  - 'this files code <file_path>' or 'cat <file_path>'
  - 'all functions in <file_path>'
  - 'lines <start> to <end> in <file_path>'
  - '<semantic search prompt>' e.g. 'AST scanning logic'
"
            .to_string(),
        }
    }
}

fn extract_file_target(graph: &VectorGraph, prompt: &str) -> Option<PathBuf> {
    // 1. Direct tokens matching indexed paths
    for token in prompt.split_whitespace() {
        let cleaned = token.trim_matches(|c| {
            c == '\'' || c == '"' || c == '`' || c == ',' || c == ':' || c == ';'
        });
        if let Some(matched) = graph.find_file_match(cleaned) {
            return Some(matched);
        }
    }

    // 2. Scan entire path keys with exact word boundaries
    let mut best_match: Option<(usize, PathBuf)> = None;
    for path in graph.files.keys() {
        let p_str = path.to_string_lossy();
        if let Some(_pos) = prompt.find(&*p_str) {
            let len = p_str.len();
            if best_match.as_ref().is_none_or(|(l, _)| len > *l) {
                best_match = Some((len, path.clone()));
            }
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            // Check if name exists as distinct word/token
            for word in prompt.split_whitespace() {
                let cleaned_word = word.trim_matches(|c| {
                    c == '\'' || c == '"' || c == '`' || c == ',' || c == ':' || c == ';'
                });
                if cleaned_word == name {
                    let len = name.len();
                    if best_match.as_ref().is_none_or(|(l, _)| len > *l) {
                        best_match = Some((len, path.clone()));
                    }
                }
            }
        }
    }

    if let Some((_, path)) = best_match {
        return Some(path);
    }

    None
}
