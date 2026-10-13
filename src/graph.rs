use crate::embedder::VectorEmbedder;
use crate::history::HistoryCache;
use crate::models::{
    CallerInfo, FileChunk, FileNode, FileSkeleton, ImpactRadius, SearchHit, Symbol,
    SymbolBodyResult, SymbolKind, SymbolReference, SymbolSkeleton,
};
use crate::scanner::AstScanner;
use crate::walker::CodebaseScanner;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct VectorGraph {
    pub embedder: VectorEmbedder,
    /// History and audit cache keeping old snapshots, diffs, and event logs
    pub history: HistoryCache,
    /// Map of relative path to node metadata
    pub files: HashMap<PathBuf, FileNode>,
    /// Flattened search chunks (functions, classes, blocks, file level)
    pub chunks: Vec<FileChunk>,
    /// Directed graph: nodes are file paths, edges are import/dependency relationships
    pub graph: DiGraph<PathBuf, ()>,
    pub node_indices: HashMap<PathBuf, NodeIndex>,
    pub last_sync: Option<std::time::Instant>,
}

impl Default for VectorGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Truncate a UTF-8 string to at most `max_bytes` without slicing inside a multi-byte codepoint.
pub fn safe_truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

impl VectorGraph {
    pub fn new() -> Self {
        Self {
            embedder: VectorEmbedder::default(),
            history: HistoryCache::new(),
            files: HashMap::new(),
            chunks: Vec::new(),
            graph: DiGraph::new(),
            node_indices: HashMap::new(),
            last_sync: None,
        }
    }

    /// Clear all files, chunks, and graph state (used when switching workspace root)
    pub fn clear(&mut self) {
        self.files.clear();
        self.chunks.clear();
        self.graph.clear();
        self.node_indices.clear();
        self.last_sync = None;
    }

    /// Insert or update a file node and re-index its chunks and graph connections
    pub fn upsert_file(&mut self, mut file: FileNode) {
        let path = file.relative_path.clone();

        // Concurrency Guard: Reject older indexing reads from overwriting newer state
        if let Some(existing) = self.files.get(&path) {
            // If content hash is identical, skip re-indexing (no-op)
            if existing.content_hash == file.content_hash && existing.size_bytes == file.size_bytes
            {
                return;
            }

            if let (Some(incoming_mod), Some(existing_mod)) =
                (file.last_modified, existing.last_modified)
                && incoming_mod < existing_mod
            {
                return; // Stale timestamp write rejected
            }
        }

        // 1. Embed full file representation
        let file_header = format!(
            "File: {}\nSymbols: {}\n\n{}",
            path.display(),
            file.symbols
                .iter()
                .map(|s| format!("{} {}", s.kind, s.name))
                .collect::<Vec<_>>()
                .join(", "),
            file.content
        );
        file.vector = self.embedder.embed(&file_header);

        // 2. Remove any previous chunks for this file
        self.chunks.retain(|c| c.file_path != path);

        // 3. Create symbol-level chunks
        let lines: Vec<&str> = file.content.lines().collect();

        for symbol in &file.symbols {
            let start = symbol.start_line.saturating_sub(1);
            let end = symbol.end_line.min(lines.len());
            let symbol_snippet = if start < lines.len() && start < end {
                lines[start..end].join("\n")
            } else {
                symbol.signature.clone()
            };

            let symbol_embed_text = format!(
                "File: {}\nSymbol: {} {}\nDoc: {}\nCode:\n{}",
                path.display(),
                symbol.kind,
                symbol.name,
                symbol.doc_comment.as_deref().unwrap_or(""),
                symbol_snippet
            );
            let vector = self.embedder.embed(&symbol_embed_text);

            self.chunks.push(FileChunk {
                id: format!("{}:{}", path.display(), symbol.name),
                file_path: path.clone(),
                start_line: symbol.start_line,
                end_line: symbol.end_line,
                content: symbol_snippet,
                symbol_name: Some(symbol.name.clone()),
                vector,
            });
        }

        // Also add whole-file chunk
        let max_preview = lines
            .iter()
            .take(50)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        self.chunks.push(FileChunk {
            id: format!("{}:file", path.display()),
            file_path: path.clone(),
            start_line: 1,
            end_line: lines.len().max(1),
            content: max_preview,
            symbol_name: None,
            vector: file.vector.clone(),
        });

        // 4. Update graph node
        let node_idx = match self.node_indices.get(&path) {
            Some(&idx) => idx,
            None => {
                let idx = self.graph.add_node(path.clone());
                self.node_indices.insert(path.clone(), idx);
                idx
            }
        };

        let old_node = self.files.get(&path).cloned();
        let old_content = old_node.as_ref().map(|n| n.content.clone());
        let old_symbols: Vec<String> = old_node
            .as_ref()
            .map(|n| n.symbols.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        let new_symbols: Vec<String> = file.symbols.iter().map(|s| s.name.clone()).collect();

        // Record history event if content actually changed
        if let Some(ref old_c) = old_content {
            if old_c != &file.content {
                self.history.record_change(
                    &path,
                    old_content,
                    Some(file.content.clone()),
                    &old_symbols,
                    &new_symbols,
                );
            }
        } else {
            // First time seeing this file
            self.history.record_initial(&path, &file.content);
        }

        self.files.insert(path, file);
        self.rebuild_edges_for_node(node_idx);
    }

    /// Remove a file from vector index and graph, recording a deletion event in history cache
    pub fn remove_file(&mut self, path: &Path) {
        let old_node = self.files.remove(path);
        let old_content = old_node.as_ref().map(|n| n.content.clone());
        let old_symbols: Vec<String> = old_node
            .as_ref()
            .map(|n| n.symbols.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();

        if let Some(content) = old_content {
            self.history
                .record_change(path, Some(content), None, &old_symbols, &[]);
        }

        self.chunks.retain(|c| c.file_path != path);
        if let Some(idx) = self.node_indices.remove(path) {
            self.graph.remove_node(idx);
            // Re-index remaining nodes mapping
            self.node_indices.clear();
            for n_idx in self.graph.node_indices() {
                let p = &self.graph[n_idx];
                self.node_indices.insert(p.clone(), n_idx);
            }
        }
    }

    /// Synchronously check if a file on disk has been modified/created/deleted and re-index immediately
    /// This eliminates race conditions where an agent queries layer before debouncer finishes.
    pub fn refresh_path_if_modified(&mut self, root: &Path, rel_or_abs_path: &Path) -> bool {
        let abs_path = if rel_or_abs_path.is_absolute() {
            rel_or_abs_path.to_path_buf()
        } else {
            root.join(rel_or_abs_path)
        };

        let rel_path = match abs_path.strip_prefix(root) {
            Ok(p) => p.to_path_buf(),
            Err(_) => rel_or_abs_path.to_path_buf(),
        };

        if abs_path.exists() && abs_path.is_file() {
            if let Ok(meta) = fs::metadata(&abs_path) {
                let modified = meta.modified().ok();
                let size = meta.len();
                let needs_update = match self.files.get(&rel_path) {
                    Some(existing) => {
                        existing.size_bytes != size
                            || (modified.is_some() && existing.last_modified != modified)
                    }
                    None => true,
                };
                if needs_update {
                    let scanner = CodebaseScanner::new(root);
                    if let Some(file_node) = scanner.scan_file(&abs_path) {
                        self.upsert_file(file_node);
                        return true;
                    }
                }
            }
        } else if self.files.contains_key(&rel_path) {
            self.remove_file(&rel_path);
            return true;
        }

        false
    }

    /// Refresh a file matching a query or filename if it has changed on disk
    pub fn refresh_if_stale(&mut self, root: &Path, partial_or_path: &str) -> bool {
        let matched = self
            .find_file_match(partial_or_path)
            .unwrap_or_else(|| PathBuf::from(partial_or_path));
        self.refresh_path_if_modified(root, &matched)
    }

    /// Synchronously synchronize the workspace index with disk.
    /// Eliminates debouncer race conditions when an agent creates files and queries immediately.
    pub fn sync_workspace(&mut self, root: &Path) {
        let scanner = CodebaseScanner::new(root);
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        let mut seen_paths = std::collections::HashSet::new();

        for entry in walker.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let rel_path = match path.strip_prefix(root) {
                Ok(p) => p.to_path_buf(),
                Err(_) => path.to_path_buf(),
            };

            // Ignore common build and target directories
            let is_ignored = rel_path.components().any(|c| {
                let name = c.as_os_str().to_string_lossy();
                name == "build" || name == "target" || name == "node_modules"
            });
            if is_ignored {
                continue;
            }

            seen_paths.insert(rel_path.clone());

            if let Ok(meta) = fs::metadata(path) {
                let modified = meta.modified().ok();
                let size = meta.len();
                let needs_update = match self.files.get(&rel_path) {
                    Some(existing) => {
                        existing.size_bytes != size
                            || (modified.is_some() && existing.last_modified != modified)
                    }
                    None => true,
                };

                if needs_update && let Some(file_node) = scanner.scan_file(path) {
                    eprintln!(
                        "[layer-sync] Discovered/updated file: {}",
                        rel_path.display()
                    );
                    self.upsert_file(file_node);
                }
            }
        }

        // Remove files that were deleted on disk
        let to_remove: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !seen_paths.contains(*p))
            .cloned()
            .collect();

        for p in to_remove {
            eprintln!("[layer-sync] Removed deleted file: {}", p.display());
            self.remove_file(&p);
        }
    }

    /// Rebuild import/dependency edges for a specific file node
    fn rebuild_edges_for_node(&mut self, source_idx: NodeIndex) {
        // Clear old outgoing edges from this node so stale imports are never retained
        let edges_to_remove: Vec<_> = self
            .graph
            .edges_directed(source_idx, petgraph::Direction::Outgoing)
            .map(|e| e.id())
            .collect();
        for edge_id in edges_to_remove {
            self.graph.remove_edge(edge_id);
        }

        let source_path = self.graph[source_idx].clone();
        if let Some(file_node) = self.files.get(&source_path) {
            let imports = file_node.imports.clone();
            for import in imports {
                // Heuristic matching: find if import matches another file in our files list
                for (other_path, &target_idx) in &self.node_indices {
                    if *other_path == source_path {
                        continue;
                    }
                    let other_str = other_path.to_string_lossy();
                    let stem = other_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if (!stem.is_empty() && import.contains(stem)) || import.contains(&*other_str) {
                        self.graph.update_edge(source_idx, target_idx, ());
                    }
                }
            }
        }
    }

    /// Semantic and hybrid vector search across all codebase chunks
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        if query.trim().is_empty() || self.chunks.is_empty() {
            return Vec::new();
        }

        let query_vec = self.embedder.embed(query);
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<SearchHit> = self
            .chunks
            .iter()
            .map(|chunk| {
                let cos_sim = VectorEmbedder::cosine_similarity(&query_vec, &chunk.vector);

                // Exact token boost for high precision
                let mut boost = 0.0f32;
                let chunk_lower = chunk.content.to_lowercase();
                if let Some(sym) = &chunk.symbol_name {
                    let sym_lower = sym.to_lowercase();
                    if sym_lower == query_lower {
                        boost += 0.5;
                    } else if sym_lower.contains(&query_lower) {
                        boost += 0.3;
                    }
                }

                for term in &query_terms {
                    if chunk_lower.contains(term) {
                        boost += 0.05;
                    }
                }

                let final_score = cos_sim + boost;
                SearchHit {
                    file_path: chunk.file_path.clone(),
                    score: final_score,
                    symbol_name: chunk.symbol_name.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    snippet: chunk.content.clone(),
                }
            })
            .collect();

        // Sort descending by score
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        scored
    }

    /// Returns list of files sorted alphabetically
    pub fn list_files(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Render directory tree structure with file stats and symbol counts
    pub fn format_tree(&self) -> String {
        if self.files.is_empty() {
            return "Codebase is empty or no files indexed yet.".to_string();
        }

        let mut output = String::new();
        output.push_str("📁 Project Structure:\n");

        let mut sorted_paths = self.list_files();
        sorted_paths.sort();

        for path in &sorted_paths {
            if let Some(node) = self.files.get(path) {
                let sym_count = node.symbols.len();
                let sym_info = if sym_count > 0 {
                    format!(" ({} symbols, {} lines)", sym_count, node.line_count)
                } else {
                    format!(" ({} lines)", node.line_count)
                };
                output.push_str(&format!("  ├── {}{}\n", path.display(), sym_info));

                // If symbols exist, list first 3 preview
                for sym in node.symbols.iter().take(3) {
                    output.push_str(&format!(
                        "  │     ↳ [{}] {} (L{}-L{})\n",
                        sym.kind, sym.name, sym.start_line, sym.end_line
                    ));
                }
                if node.symbols.len() > 3 {
                    output.push_str(&format!("  │     ↳ ... +{} more\n", node.symbols.len() - 3));
                }
            }
        }
        output
    }

    /// Render compact directory tree structure filtered by path, max_depth, and limit
    pub fn format_compact_tree(
        &self,
        path_filter: Option<&Path>,
        max_depth: Option<usize>,
        limit: Option<usize>,
    ) -> String {
        if self.files.is_empty() {
            return "Codebase is empty or no files indexed yet.".to_string();
        }

        let max_d = max_depth.unwrap_or(2);
        let max_items = limit.unwrap_or(50);
        let mut sorted_paths = self.list_files();
        sorted_paths.sort();

        let prefix_str = path_filter
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let prefix_clean = prefix_str.trim_matches('/');

        let mut lines = Vec::new();
        let mut total_matching = 0;
        let mut directories_seen = std::collections::HashSet::new();

        for path in &sorted_paths {
            let path_str = path.to_string_lossy();
            if !prefix_clean.is_empty() && !path_str.starts_with(prefix_clean) {
                continue;
            }

            total_matching += 1;
            if let Some(parent) = path.parent() {
                directories_seen.insert(parent.to_path_buf());
            }

            // Check depth relative to prefix
            let rel_str = if prefix_clean.is_empty() {
                path_str.as_ref()
            } else {
                path_str
                    .strip_prefix(prefix_clean)
                    .unwrap_or(path_str.as_ref())
                    .trim_start_matches('/')
            };

            let depth = rel_str.split('/').count();
            if depth > max_d {
                continue;
            }

            if lines.len() < max_items
                && let Some(node) = self.files.get(path)
            {
                lines.push(format!(
                    "{} ({} lines, {} symbols)",
                    path.display(),
                    node.line_count,
                    node.symbols.len()
                ));
            }
        }

        let mut output = lines.join("\n");
        if total_matching > lines.len() {
            output.push_str(&format!(
                "\n... [Showing {} of {} matching files across {} directories. Use 'path' parameter to inspect specific subdirectories]",
                lines.len(),
                total_matching,
                directories_seen.len()
            ));
        }

        if output.is_empty() {
            format!("No files found matching path '{}'.", prefix_clean)
        } else {
            output
        }
    }

    /// Get exact file content
    pub fn get_file_content(&self, path: &Path) -> Option<&str> {
        self.files.get(path).map(|node| node.content.as_str())
    }

    /// Get symbols in a specific file
    pub fn get_file_symbols(&self, path: &Path) -> Option<&[Symbol]> {
        self.files.get(path).map(|node| node.symbols.as_slice())
    }

    /// Get slice of lines (1-indexed, inclusive) from a file
    pub fn get_file_lines(
        &self,
        path: &Path,
        start_line: usize,
        end_line: usize,
    ) -> Option<String> {
        let node = self.files.get(path)?;
        let lines: Vec<&str> = node.content.lines().collect();
        if lines.is_empty() {
            return Some(String::new());
        }

        let start = start_line.saturating_sub(1).min(lines.len() - 1);
        let end = end_line.min(lines.len());

        if start >= end {
            return Some(String::new());
        }

        let mut result = String::new();
        let max_total_bytes = 24 * 1024; // 24 KB ceiling
        for (idx, line) in lines[start..end].iter().enumerate() {
            let actual_line = start + idx + 1;
            if result.len() > max_total_bytes {
                result.push_str(&format!(
                    "\n... [Output capped at 24KB limit at line {} of {}. Narrow requested line range]",
                    actual_line, end
                ));
                break;
            }
            if line.len() > 1000 {
                result.push_str(&format!(
                    "{:4} | {}... [line truncated at 1000 chars]\n",
                    actual_line,
                    safe_truncate_str(line, 1000)
                ));
            } else {
                result.push_str(&format!("{:4} | {}\n", actual_line, line));
            }
        }
        Some(result)
    }

    /// Find a file by exact or suffix match (e.g. "main.rs" matches "src/main.rs")
    pub fn find_file_match(&self, partial: &str) -> Option<PathBuf> {
        let partial_lower = partial.to_lowercase();
        // Exact match
        for path in self.files.keys() {
            if path.to_string_lossy().to_lowercase() == partial_lower {
                return Some(path.clone());
            }
        }
        // Suffix match (e.g. "src/embedder.rs" or "embedder.rs")
        for path in self.files.keys() {
            let p_lower = path.to_string_lossy().to_lowercase();
            if p_lower.ends_with(&format!("/{}", partial_lower)) || p_lower == partial_lower {
                return Some(path.clone());
            }
        }
        // Match filename directly
        for path in self.files.keys() {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                && file_name.to_lowercase() == partial_lower
            {
                return Some(path.clone());
            }
        }
        None
    }

    /// Agent-facing typed skeleton of a file (compact representation)
    pub fn get_file_skeleton(&self, path: &Path) -> Option<FileSkeleton> {
        let node = self.files.get(path)?;
        let symbols = node
            .symbols
            .iter()
            .map(|s| SymbolSkeleton {
                name: s.name.clone(),
                kind: s.kind.to_string(),
                lines: [s.start_line, s.end_line],
                signature: s.signature.clone(),
                doc: s.doc_comment.as_ref().and_then(|d| {
                    let mut lines = Vec::new();
                    for line in d.lines() {
                        let trimmed = line.trim();
                        if trimmed.is_empty() && !lines.is_empty() {
                            break;
                        }
                        if !trimmed.is_empty() && lines.len() < 3 {
                            lines.push(trimmed);
                        }
                    }
                    if lines.is_empty() {
                        None
                    } else {
                        Some(lines.join(" "))
                    }
                }),
            })
            .collect();

        Some(FileSkeleton {
            path: path.display().to_string(),
            total_lines: node.line_count,
            imports: node.imports.clone(),
            symbols,
        })
    }

    /// Find symbol definition(s) across the entire codebase
    pub fn find_symbol(&self, symbol_name: &str) -> Vec<(PathBuf, Symbol)> {
        self.find_symbol_filtered(symbol_name, None, None)
    }

    /// Find symbol definitions with optional file path filter and limit
    pub fn find_symbol_filtered(
        &self,
        symbol_name: &str,
        file_hint: Option<&Path>,
        limit: Option<usize>,
    ) -> Vec<(PathBuf, Symbol)> {
        let mut results = Vec::new();
        let sym_lower = symbol_name.to_lowercase();
        let suffix = format!("::{}", sym_lower);
        for (path, node) in &self.files {
            if let Some(hint) = file_hint
                && path != hint
                && !path.ends_with(hint)
            {
                continue;
            }
            for sym in &node.symbols {
                let name_lower = sym.name.to_lowercase();
                if name_lower == sym_lower || name_lower.ends_with(&suffix) {
                    results.push((path.clone(), sym.clone()));
                }
            }
        }
        // Deterministic relevance ranking:
        // 1. Exact case-sensitive match (+100) vs case-insensitive (+60) vs suffix (+20)
        // 2. Definition kind: Struct/Class/Trait/Interface (+40) > Function (+30) > Method (+15)
        // 3. Public visibility: "pub " or "export " (+25)
        // 4. Prominent path: lib.rs, main.rs, app.rs, index.ts (+30)
        // 5. Demote test/mock/example paths (-60)
        results.sort_by(|(path_a, sym_a), (path_b, sym_b)| {
            let score = |path: &Path, sym: &Symbol| -> i32 {
                let mut s = 0;
                if sym.name == symbol_name {
                    s += 100;
                } else if sym.name.eq_ignore_ascii_case(symbol_name) {
                    s += 60;
                } else {
                    s += 20;
                }
                match sym.kind {
                    SymbolKind::Struct
                    | SymbolKind::Class
                    | SymbolKind::Trait
                    | SymbolKind::Interface => s += 40,
                    SymbolKind::Function => s += 30,
                    SymbolKind::Method => s += 15,
                    SymbolKind::TypeAlias | SymbolKind::Enum => s += 20,
                    _ => s += 5,
                }
                if sym.signature.contains("pub ") || sym.signature.starts_with("export ") {
                    s += 25;
                }
                let p_str = path.to_string_lossy();
                if p_str.contains("/test")
                    || p_str.contains("/mock")
                    || p_str.contains("/example")
                    || p_str.contains("_test.")
                    || p_str.contains(".test.")
                {
                    s -= 60;
                } else if p_str.ends_with("lib.rs")
                    || p_str.ends_with("main.rs")
                    || p_str.ends_with("app.rs")
                    || p_str.ends_with("index.ts")
                    || p_str.ends_with("mod.rs")
                {
                    s += 30;
                }
                s
            };
            let sa = score(path_a, sym_a);
            let sb = score(path_b, sym_b);
            sb.cmp(&sa)
                .then_with(|| path_a.cmp(path_b))
                .then_with(|| sym_a.start_line.cmp(&sym_b.start_line))
        });

        if let Some(lim) = limit {
            results.truncate(lim);
        }
        results
    }

    /// Find all references / call-sites to a symbol across the codebase
    pub fn find_references(&self, symbol_name: &str) -> Vec<SymbolReference> {
        self.find_references_filtered(symbol_name, false)
    }

    /// Find references with optional code-only filtering
    pub fn find_references_filtered(
        &self,
        symbol_name: &str,
        code_only: bool,
    ) -> Vec<SymbolReference> {
        let mut references = Vec::new();
        if symbol_name.trim().is_empty() {
            return references;
        }
        let sym_lower = symbol_name.to_lowercase();
        let suffix = format!("::{}", sym_lower);

        for (path, node) in &self.files {
            if code_only && !is_code_file_node(node) {
                continue;
            }
            if !node.content.contains(symbol_name) || !is_word_match(&node.content, symbol_name) {
                continue;
            }
            let path_str = path.display().to_string();

            if let Some(ast_usages) =
                AstScanner::find_ast_usages(&node.content, node.extension.as_deref(), symbol_name)
            {
                for usage in ast_usages {
                    let line_num = usage.line;
                    let caller = node
                        .symbols
                        .iter()
                        .filter(|s| line_num >= s.start_line && line_num <= s.end_line)
                        .min_by_key(|s| s.end_line.saturating_sub(s.start_line));

                    let is_inside_target_def = node.symbols.iter().any(|s| {
                        let n_lower = s.name.to_lowercase();
                        (n_lower == sym_lower || n_lower.ends_with(&suffix))
                            && line_num >= s.start_line
                            && line_num <= s.end_line
                    });

                    let is_def = caller.is_some_and(|c| {
                        let c_lower = c.name.to_lowercase();
                        (c_lower == sym_lower || c_lower.ends_with(&suffix))
                            && (c.start_line == line_num || c.signature.contains(symbol_name))
                    });

                    if !is_def && !is_inside_target_def {
                        let is_call = usage.is_definite_code_call_or_ref
                            || is_direct_call_or_construction(&usage.line_text, symbol_name);

                        references.push(SymbolReference {
                            file: path_str.clone(),
                            line: line_num,
                            text: usage.line_text,
                            caller_symbol: caller.map(|c| c.name.clone()),
                            is_approximate: !is_call,
                        });
                    }
                }
            } else {
                for (idx, line) in node.content.lines().enumerate() {
                    let line_num = idx + 1;
                    if is_word_match(line, symbol_name) {
                        // Pick the most specific enclosing symbol (smallest line span)
                        let caller = node
                            .symbols
                            .iter()
                            .filter(|s| line_num >= s.start_line && line_num <= s.end_line)
                            .min_by_key(|s| s.end_line.saturating_sub(s.start_line));

                        // Avoid classifying the symbol's own definition or references inside its own definition
                        let is_inside_target_def = node.symbols.iter().any(|s| {
                            let n_lower = s.name.to_lowercase();
                            (n_lower == sym_lower || n_lower.ends_with(&suffix))
                                && line_num >= s.start_line
                                && line_num <= s.end_line
                        });

                        let is_def = caller.is_some_and(|c| {
                            let c_lower = c.name.to_lowercase();
                            (c_lower == sym_lower || c_lower.ends_with(&suffix))
                                && (c.start_line == line_num || c.signature.contains(symbol_name))
                        });

                        if !is_def && !is_inside_target_def {
                            let is_call = is_direct_call_or_construction(line, symbol_name);

                            references.push(SymbolReference {
                                file: path_str.clone(),
                                line: line_num,
                                text: line.trim().to_string(),
                                caller_symbol: caller.map(|c| c.name.clone()),
                                is_approximate: !is_call,
                            });
                        }
                    }
                }
            }
        }
        // Prioritize definite code calls over approximate references
        references.sort_by_key(|r| r.is_approximate);
        references
    }

    /// Get all callers of a function or method across the codebase, grouped by enclosing symbol
    pub fn get_callers(&self, symbol_name: &str) -> Vec<CallerInfo> {
        let refs = self.find_references_filtered(symbol_name, true);
        let mut grouped: std::collections::HashMap<(String, String), (CallerInfo, usize, bool)> =
            std::collections::HashMap::new();
        let mut order: Vec<(String, String)> = Vec::new();

        for r in refs {
            let caller_name = r
                .caller_symbol
                .unwrap_or_else(|| "<module-level>".to_string());
            let file = r.file;
            let key = (file.clone(), caller_name.clone());

            let is_approx = r.is_approximate;
            if let Some((info, count, has_definite)) = grouped.get_mut(&key) {
                *count += 1;
                if !is_approx {
                    *has_definite = true;
                    info.is_approximate = false;
                }
            } else {
                let file_path = PathBuf::from(&file);
                let caller_lines = self
                    .files
                    .get(&file_path)
                    .and_then(|n| n.symbols.iter().find(|s| s.name == caller_name))
                    .map(|s| [s.start_line, s.end_line])
                    .unwrap_or([r.line, r.line]);

                let info = CallerInfo {
                    caller_symbol: caller_name,
                    caller_file: file,
                    caller_lines,
                    call_line: r.line,
                    call_snippet: r.text,
                    call_count: None,
                    is_approximate: is_approx,
                };
                order.push(key.clone());
                grouped.insert(key, (info, 1, !is_approx));
            }
        }

        let mut callers = Vec::new();
        for key in order {
            if let Some((mut info, count, has_definite)) = grouped.remove(&key) {
                if count > 1 {
                    info.call_count = Some(count);
                }
                info.is_approximate = !has_definite;
                callers.push(info);
            }
        }
        callers
    }

    /// Retrieve the exact code implementation of a symbol (function, method, struct)
    pub fn get_symbol_body(
        &self,
        symbol_name: &str,
        file_hint: Option<&Path>,
    ) -> Option<SymbolBodyResult> {
        self.get_symbol_body_opt(symbol_name, file_hint, 160)
    }

    /// Retrieve the exact code implementation of a symbol with custom line limit
    pub fn get_symbol_body_opt(
        &self,
        symbol_name: &str,
        file_hint: Option<&Path>,
        max_lines: usize,
    ) -> Option<SymbolBodyResult> {
        let matches = self.find_symbol(symbol_name);
        if matches.is_empty() {
            return None;
        }

        // Select the best match based on file_hint, or implementation over header
        let selected_idx = if let Some(hint) = file_hint {
            matches
                .iter()
                .position(|(p, _)| p == hint || p.ends_with(hint))
                .unwrap_or(0)
        } else {
            // If multiple, prefer file that actually has multiline implementation over 1-line declaration
            let impl_idx = matches.iter().position(|(p, s)| {
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                ["cpp", "c", "cc", "rs", "py", "js", "ts", "go"].contains(&ext)
                    && s.end_line > s.start_line
            });
            impl_idx.unwrap_or(0)
        };

        let (chosen_path, chosen_sym) = &matches[selected_idx];

        // Collect other candidate locations only if ambiguous and NO file hint was specified.
        // If the caller provided a file hint, they already disambiguated which implementation they want.
        let candidates: Vec<String> = if file_hint.is_none() {
            matches
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != selected_idx)
                .filter(|(_, (p, s))| {
                    if p == chosen_path {
                        let is_nested = (s.start_line >= chosen_sym.start_line
                            && s.end_line <= chosen_sym.end_line)
                            || (chosen_sym.start_line >= s.start_line
                                && chosen_sym.end_line <= s.end_line);
                        !is_nested
                    } else {
                        true
                    }
                })
                .map(|(_, (p, s))| format!("{}:{}: {}", p.display(), s.start_line, s.signature))
                .collect()
        } else {
            Vec::new()
        };

        let node = self.files.get(chosen_path)?;
        let lines: Vec<&str> = node.content.lines().collect();
        let start = chosen_sym.start_line.saturating_sub(1);
        let end = chosen_sym.end_line.min(lines.len());

        let line_count = end.saturating_sub(start);
        let returned_count = line_count.min(max_lines);
        let (body, is_truncated, next_query_hint) = if line_count > max_lines {
            let mut truncated = lines[start..start + max_lines].join("\n");
            let remaining = line_count - max_lines;
            truncated.push_str(&format!(
                "\n// ... [truncated {} lines; use max_lines={} or get_lines to view full body]",
                remaining, line_count
            ));
            let hint = format!(
                "Symbol body truncated at line {} of {}. Pass max_lines={} to retrieve complete body, or call get_lines(path='{}', start={}, end={}) for remaining lines.",
                max_lines,
                line_count,
                line_count,
                chosen_path.display(),
                chosen_sym.start_line + max_lines,
                chosen_sym.end_line
            );
            (truncated, true, Some(hint))
        } else if start < end {
            let joined = lines[start..end].join("\n");
            let max_total_bytes = 32 * 1024;
            if joined.len() > max_total_bytes {
                let mut truncated = safe_truncate_str(&joined, max_total_bytes).to_string();
                truncated.push_str(
                    "\n// ... [truncated: exceeded 32KB byte limit. Inspect smaller fragments with get_lines]",
                );
                let hint = format!(
                    "Symbol body exceeded 32KB byte safety limit. Inspect with get_lines(path='{}', start={}, end={}).",
                    chosen_path.display(),
                    chosen_sym.start_line,
                    chosen_sym.end_line
                );
                (truncated, true, Some(hint))
            } else {
                (joined, false, None)
            }
        } else {
            (chosen_sym.signature.clone(), false, None)
        };

        Some(SymbolBodyResult {
            symbol_name: chosen_sym.name.clone(),
            file: chosen_path.display().to_string(),
            lines: [chosen_sym.start_line, chosen_sym.end_line],
            signature: chosen_sym.signature.clone(),
            body,
            total_lines: line_count,
            returned_lines: returned_count,
            is_truncated,
            next_query_hint,
            candidates,
        })
    }

    /// Get the 1-hop impact radius of a symbol: callers (who calls it) and callees (what it calls)
    pub fn get_impact_radius(&self, symbol_name: &str, file_hint: Option<&Path>) -> ImpactRadius {
        let all_callers = self.get_callers(symbol_name);
        let total_callers = all_callers.len();
        let callers_truncated = total_callers > 10;
        let callers: Vec<_> = all_callers.into_iter().take(10).collect();
        let callers_confidence = if callers.is_empty() {
            "approximate".to_string()
        } else if callers.iter().all(|c| !c.is_approximate) {
            "high".to_string()
        } else if callers.iter().any(|c| !c.is_approximate) {
            "mixed".to_string()
        } else {
            "approximate".to_string()
        };

        // Find the definition(s) of this symbol
        let decls = self.find_symbol(symbol_name);
        let matching_decl = if let Some(hint) = file_hint {
            decls
                .iter()
                .find(|(p, _)| p == hint || p.ends_with(hint))
                .or_else(|| decls.first())
        } else {
            decls.first()
        };

        let candidates: Vec<String> = if let Some((m_path, m_sym)) = matching_decl {
            decls
                .iter()
                .filter(|(p, s)| {
                    if p == m_path {
                        let is_nested = (s.start_line >= m_sym.start_line
                            && s.end_line <= m_sym.end_line)
                            || (m_sym.start_line >= s.start_line && m_sym.end_line <= s.end_line);
                        !is_nested
                    } else {
                        true
                    }
                })
                .map(|(p, s)| format!("{}:{}: {}", p.display(), s.start_line, s.signature))
                .collect()
        } else {
            Vec::new()
        };

        let mut callees = Vec::new();
        let mut file_str = None;
        let mut sig_str = None;

        if let Some((path, sym)) = matching_decl {
            file_str = Some(path.display().to_string());
            sig_str = Some(sym.signature.clone());

            // Extract the body of this symbol
            if let Some(node) = self.files.get(path) {
                let lines: Vec<&str> = node.content.lines().collect();
                let start = sym.start_line.saturating_sub(1);
                let end = sym.end_line.min(lines.len());
                if start < end {
                    let body_text = lines[start..end].join("\n");

                    // Check which functions or methods across the codebase are invoked inside this body
                    for (other_path, other_node) in &self.files {
                        for other_s in &other_node.symbols {
                            // Callees must be callable (Function or Method), not classes or structs
                            if other_s.kind != SymbolKind::Function
                                && other_s.kind != SymbolKind::Method
                                && other_s.kind != SymbolKind::Unknown
                            {
                                continue;
                            }

                            // Exclude the symbol itself
                            if other_s.name == sym.name {
                                continue;
                            }

                            // If sym is a class or enclosing symbol, exclude methods defined inside it
                            if other_path == path
                                && other_s.start_line >= sym.start_line
                                && other_s.end_line <= sym.end_line
                            {
                                continue;
                            }

                            // Check if other_s.name is called inside the body
                            if body_text.contains(other_s.name.as_str())
                                && is_callee_call(&body_text, &other_s.name)
                                && !callees.contains(&other_s.name)
                            {
                                callees.push(other_s.name.clone());
                                if callees.len() >= 10 {
                                    break;
                                }
                            }
                        }
                        if callees.len() >= 10 {
                            break;
                        }
                    }
                }
            }
        }

        callees.sort();
        callees.dedup();

        ImpactRadius {
            target_symbol: symbol_name.to_string(),
            file: file_str,
            signature: sig_str,
            total_callers,
            callers,
            callers_truncated,
            callers_confidence,
            callees,
            callees_confidence: "approximate".to_string(),
            candidates,
        }
    }
}

/// Check if a symbol appears in line as a direct call, constructor invocation, or template instantiation
pub fn is_direct_call_or_construction(line: &str, symbol_name: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return false;
    }

    let bytes = line.as_bytes();
    let name_bytes = symbol_name.as_bytes();
    let n_len = name_bytes.len();
    if bytes.len() < n_len {
        return false;
    }

    for (i, window) in bytes.windows(n_len).enumerate() {
        if window == name_bytes {
            // Check boundary before
            let left_ok = if i == 0 {
                true
            } else {
                let prev = bytes[i - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };

            if !left_ok {
                continue;
            }

            // Check boundary after
            let rest = line[i + n_len..].trim_start();

            // 1. Direct call or constructor: symbol(...) or symbol{...} or macro symbol!(...)
            if rest.starts_with('(') || rest.starts_with('{') || rest.starts_with("!(") {
                let prefix = line[..i].trim();
                let is_definition = prefix.ends_with("fn")
                    || prefix.ends_with("def")
                    || prefix.ends_with("func")
                    || prefix.ends_with("function")
                    || prefix.ends_with("void")
                    || prefix.ends_with("int")
                    || prefix.ends_with("bool")
                    || prefix.ends_with("auto");
                if !is_definition {
                    return true;
                }
            }

            // 2. Template instantiation: e.g. make_unique<Symbol>(...) or make_shared<Symbol>(...) or Vector<Symbol>()
            if let Some(after_gt) = rest.strip_prefix('>') {
                let after_gt = after_gt.trim_start();
                if after_gt.starts_with('(') || after_gt.starts_with('{') {
                    return true;
                }
            }

            // 3. Heap allocation: new Symbol(...) or new Symbol
            let prefix = line[..i].trim();
            if prefix.ends_with("new") {
                return true;
            }
        }
    }

    false
}

/// Check if a function/method name is invoked as a call in body text (e.g. `func(`)
pub fn is_callee_call(body_text: &str, func_name: &str) -> bool {
    if func_name.len() < 2 {
        return false;
    }
    let keywords = [
        "if",
        "for",
        "while",
        "switch",
        "catch",
        "return",
        "sizeof",
        "new",
        "delete",
        "typeof",
        "instanceof",
        "decltype",
        "throw",
        "case",
        "auto",
        "void",
        "int",
        "bool",
    ];
    if keywords.contains(&func_name) {
        return false;
    }

    for line in body_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }

        // If line is a C++ declaration/signature with const/override/virtual/=0, skip
        if trimmed.starts_with("virtual ")
            || trimmed.contains(") const")
            || trimmed.contains(") override")
            || trimmed.contains(") = 0")
            || trimmed.contains(") noexcept")
        {
            continue;
        }

        let bytes = line.as_bytes();
        let name_bytes = func_name.as_bytes();
        let n_len = name_bytes.len();
        if bytes.len() < n_len {
            continue;
        }

        for (i, window) in bytes.windows(n_len).enumerate() {
            if window == name_bytes {
                // Check boundary before
                let left_ok = if i == 0 {
                    true
                } else {
                    let prev = bytes[i - 1];
                    !prev.is_ascii_alphanumeric() && prev != b'_'
                };

                if !left_ok {
                    continue;
                }

                // Check boundary after: optional whitespace then '(' or '!('
                let rest = &line[i + n_len..];
                let rest_trimmed = rest.trim_start();
                if !rest_trimmed.starts_with('(') && !rest_trimmed.starts_with("!(") {
                    continue;
                }

                // Exclude function definition prefixes
                let prefix = line[..i].trim();
                let is_declaration = prefix.ends_with("fn")
                    || prefix.ends_with("def")
                    || prefix.ends_with("func")
                    || prefix.ends_with("function")
                    || prefix.ends_with("void")
                    || prefix.ends_with("int")
                    || prefix.ends_with("bool")
                    || prefix.ends_with("auto");

                if !is_declaration {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if a word appears as a distinct identifier in a line (word boundary match)
fn is_word_match(line: &str, word: &str) -> bool {
    let bytes = line.as_bytes();
    let word_bytes = word.as_bytes();
    let w_len = word_bytes.len();
    if w_len == 0 || bytes.len() < w_len {
        return false;
    }

    for (i, window) in bytes.windows(w_len).enumerate() {
        if window == word_bytes {
            // Check boundary before
            let left_ok = if i == 0 {
                true
            } else {
                let prev = bytes[i - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };

            // Check boundary after
            let right_ok = if i + w_len == bytes.len() {
                true
            } else {
                let next = bytes[i + w_len];
                !next.is_ascii_alphanumeric() && next != b'_'
            };

            if left_ok && right_ok {
                return true;
            }
        }
    }
    false
}

/// Helper to distinguish code files from documentation, schemas, and assets
fn is_code_file_node(node: &FileNode) -> bool {
    let non_code_extensions = [
        "md",
        "markdown",
        "json",
        "yaml",
        "yml",
        "toml",
        "txt",
        "csv",
        "xml",
        "html",
        "svg",
        "css",
        "scss",
        "changelog",
    ];
    if let Some(ref ext) = node.extension
        && non_code_extensions.contains(&ext.to_lowercase().as_str())
    {
        return false;
    }
    let code_extensions = [
        "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyw", "pyi", "c", "cpp", "cxx", "cc",
        "h", "hpp", "hxx", "go", "java", "kt", "kts", "swift", "rb", "cs", "php", "scala", "dart",
        "zig", "lua", "sh", "bash", "zsh",
    ];
    if let Some(ref ext) = node.extension
        && code_extensions.contains(&ext.to_lowercase().as_str())
    {
        return true;
    }
    !node.symbols.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FileNode, Symbol, SymbolKind};
    use std::path::PathBuf;

    fn make_test_node(rel_path: &str, content: &str, symbols: Vec<Symbol>) -> FileNode {
        let p = PathBuf::from(rel_path);
        // FNV-1a hash of content
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in content.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        FileNode {
            relative_path: p.clone(),
            absolute_path: p,
            extension: Some("rs".to_string()),
            line_count: content.lines().count(),
            size_bytes: content.len() as u64,
            content_hash: hash,
            last_modified: None,
            symbols,
            imports: Vec::new(),
            content: content.to_string(),
            vector: Vec::new(),
        }
    }

    #[test]
    fn test_get_symbol_body_exact() {
        let mut graph = VectorGraph::new();
        let content = "use std::io;\n\nfn process_payment() {\n    let amount = 100;\n    println!(\"{}\", amount);\n}\n";
        let sym = Symbol {
            name: "process_payment".to_string(),
            kind: SymbolKind::Function,
            start_line: 3,
            end_line: 6,
            signature: "fn process_payment()".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node("src/payment.rs", content, vec![sym]));

        let res = graph.get_symbol_body("process_payment", None);
        assert!(res.is_some());
        let body = res.unwrap();
        assert_eq!(body.symbol_name, "process_payment");
        assert_eq!(body.file, "src/payment.rs");
        assert_eq!(body.lines, [3, 6]);
        assert!(!body.is_truncated);
        assert!(body.candidates.is_empty());
        assert!(body.body.contains("println!(\"{}\", amount);"));
    }

    #[test]
    fn test_ambiguous_symbol_candidates() {
        let mut graph = VectorGraph::new();

        let sym_a = Symbol {
            name: "init".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 3,
            signature: "fn init() -> bool".to_string(),
            doc_comment: None,
        };
        let sym_b = Symbol {
            name: "init".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 3,
            signature: "fn init() -> Result<()>".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node(
            "src/module_a.rs",
            "fn init() -> bool {\n true \n}\n",
            vec![sym_a],
        ));
        graph.upsert_file(make_test_node(
            "src/module_b.rs",
            "fn init() -> Result<()> {\n Ok(()) \n}\n",
            vec![sym_b],
        ));

        // Query without file hint
        let res = graph.get_symbol_body("init", None).unwrap();
        assert_eq!(res.candidates.len(), 1);
        assert!(res.candidates[0].contains("init()"));

        // Query with file hint for module_b (should have zero candidates because caller disambiguated)
        let res_b = graph
            .get_symbol_body("init", Some(Path::new("src/module_b.rs")))
            .unwrap();
        assert_eq!(res_b.file, "src/module_b.rs");
        assert_eq!(res_b.candidates.len(), 0);
    }

    #[test]
    fn test_get_impact_radius_callers_and_callees() {
        let mut graph = VectorGraph::new();

        // 1. Caller file
        let caller_content = "fn run_job() {\n    execute_task();\n}\n";
        let caller_sym = Symbol {
            name: "run_job".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 3,
            signature: "fn run_job()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node(
            "src/caller.rs",
            caller_content,
            vec![caller_sym],
        ));

        // 2. Target file: calls helper_fn, but also mentions TypeName
        let target_content = "fn execute_task() {\n    let cfg = Config {};\n    helper_fn();\n}\n";
        let target_sym = Symbol {
            name: "execute_task".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 4,
            signature: "fn execute_task()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node(
            "src/target.rs",
            target_content,
            vec![target_sym],
        ));

        // 3. Child file (callee)
        let child_content = "fn helper_fn() {}\n";
        let child_sym = Symbol {
            name: "helper_fn".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 1,
            signature: "fn helper_fn()".to_string(),
            doc_comment: None,
        };
        // 4. Type symbol (struct - should NOT be included in callees)
        let type_sym = Symbol {
            name: "Config".to_string(),
            kind: SymbolKind::Struct,
            start_line: 1,
            end_line: 1,
            signature: "struct Config".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node(
            "src/helper.rs",
            child_content,
            vec![child_sym, type_sym],
        ));

        let impact = graph.get_impact_radius("execute_task", None);
        assert_eq!(impact.target_symbol, "execute_task");

        // Verify caller
        assert_eq!(impact.callers.len(), 1);
        assert_eq!(impact.callers[0].caller_symbol, "run_job");
        assert_eq!(impact.callers_confidence, "high");

        // Verify callees: helper_fn is included, but Config struct is filtered out
        assert_eq!(impact.callees, vec!["helper_fn".to_string()]);
        assert_eq!(impact.callees_confidence, "approximate");
    }

    #[test]
    fn test_is_callee_call_detection() {
        assert!(is_callee_call(
            "let res = calculate_hash();",
            "calculate_hash"
        ));
        assert!(is_callee_call(
            "foo.calculate_hash(arg1, arg2);",
            "calculate_hash"
        ));
        assert!(is_callee_call("calculate_hash();", "calculate_hash"));

        // Declarations should NOT match as calls
        assert!(!is_callee_call("fn calculate_hash() {", "calculate_hash"));
        assert!(!is_callee_call(
            "pub fn calculate_hash() -> usize {",
            "calculate_hash"
        ));
        assert!(!is_callee_call("void calculate_hash()", "calculate_hash"));
        assert!(!is_callee_call(
            "virtual void calculate_hash() const override;",
            "calculate_hash"
        ));

        // Mentions without call syntax should NOT match
        assert!(!is_callee_call("let x = calculate_hash;", "calculate_hash"));
        assert!(!is_callee_call(
            "// calculate_hash() commented out",
            "calculate_hash"
        ));
    }

    #[test]
    fn test_body_truncation() {
        let mut graph = VectorGraph::new();
        let mut long_lines = Vec::new();
        for i in 0..450 {
            long_lines.push(format!("    let x_{} = {};", i, i));
        }
        let content = format!("fn huge_function() {{\n{}\n}}\n", long_lines.join("\n"));
        let sym = Symbol {
            name: "huge_function".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 452,
            signature: "fn huge_function()".to_string(),
            doc_comment: None,
        };

        graph.upsert_file(make_test_node("src/huge.rs", &content, vec![sym]));

        let res = graph.get_symbol_body("huge_function", None).unwrap();
        assert!(res.is_truncated);
        assert!(res.body.contains("truncated"));
    }

    #[test]
    fn test_stale_import_edge_removal() {
        let mut graph = VectorGraph::new();

        // Target file
        let target_sym = Symbol {
            name: "target_func".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 1,
            signature: "fn target_func()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node(
            "src/target.rs",
            "fn target_func() {}",
            vec![target_sym],
        ));

        // Source file importing target
        let mut source_node = make_test_node(
            "src/source.rs",
            "use crate::target;\nfn run() { target_func(); }",
            vec![],
        );
        source_node.imports = vec!["target".to_string()];
        graph.upsert_file(source_node.clone());

        let src_idx = graph.node_indices[&PathBuf::from("src/source.rs")];
        let tgt_idx = graph.node_indices[&PathBuf::from("src/target.rs")];
        assert!(
            graph.graph.find_edge(src_idx, tgt_idx).is_some(),
            "Edge should exist initially"
        );

        // Now update source file to remove the import
        let mut updated_source =
            make_test_node("src/source.rs", "fn run() { /* no import */ }", vec![]);
        updated_source.imports = vec![];
        // Ensure content differs so it doesn't short-circuit
        updated_source.size_bytes = 100;
        updated_source.content_hash = 999;
        graph.upsert_file(updated_source);

        assert!(
            graph.graph.find_edge(src_idx, tgt_idx).is_none(),
            "Stale edge should have been cleared"
        );
    }

    #[test]
    fn test_monotonic_version_gating() {
        use std::time::{Duration, SystemTime};
        let mut graph = VectorGraph::new();

        let time_now = SystemTime::now();
        let time_past = time_now - Duration::from_secs(60);

        let mut node_v1 = make_test_node("src/app.rs", "fn v1() {}", vec![]);
        node_v1.last_modified = Some(time_now);
        node_v1.content_hash = 101;
        graph.upsert_file(node_v1);

        assert_eq!(graph.files[&PathBuf::from("src/app.rs")].content_hash, 101);

        // Attempt to upsert stale older version (older mtime)
        let mut node_stale = make_test_node("src/app.rs", "fn stale() {}", vec![]);
        node_stale.last_modified = Some(time_past);
        node_stale.content_hash = 99;
        graph.upsert_file(node_stale);

        // State should remain v1, rejecting the stale older write
        assert_eq!(graph.files[&PathBuf::from("src/app.rs")].content_hash, 101);
    }

    #[test]
    fn test_grouped_callers_with_count() {
        let mut graph = VectorGraph::new();
        // A function with multiple match arms calling the same symbol
        let content = "fn to_tokens() {\n    match effect {\n        WindowEffect::A => 1,\n        WindowEffect::B => 2,\n        WindowEffect::C => 3,\n    }\n}\n";
        let sym = Symbol {
            name: "to_tokens".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 7,
            signature: "fn to_tokens()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node("src/config.rs", content, vec![sym]));

        let callers = graph.get_callers("WindowEffect");
        // Exactly 1 caller group for to_tokens instead of 3 duplicates
        assert_eq!(callers.len(), 1);
        let caller = &callers[0];
        assert_eq!(caller.caller_symbol, "to_tokens");
        assert_eq!(caller.caller_file, "src/config.rs");
        assert_eq!(caller.call_count, Some(3));
        assert_eq!(caller.caller_lines, [1, 7]);
    }

    #[test]
    fn test_find_references_code_only() {
        let mut graph = VectorGraph::new();
        let code_node = make_test_node("src/lib.rs", "let effect = WindowEffect::Blur;", vec![]);
        let mut doc_node = make_test_node(
            "CHANGELOG.md",
            "# Release\nWindowEffect was modified.",
            vec![],
        );
        doc_node.extension = Some("md".to_string());

        graph.upsert_file(code_node);
        graph.upsert_file(doc_node);

        let all_refs = graph.find_references("WindowEffect");
        assert_eq!(all_refs.len(), 2);

        let code_refs = graph.find_references_filtered("WindowEffect", true);
        assert_eq!(code_refs.len(), 1);
        assert_eq!(code_refs[0].file, "src/lib.rs");
    }

    #[test]
    fn test_safe_truncate_str_utf8() {
        // Multi-byte CJK (3 bytes each: 🦀 is 4 bytes, 你 is 3 bytes, 好 is 3 bytes)
        let s = "你好世界🦀test";
        // "你" = bytes 0..3, "好" = bytes 3..6, "世" = bytes 6..9, "界" = bytes 9..12, "🦀" = bytes 12..16
        // Capping at 5 bytes should truncate to 3 bytes ("你") without panicking
        assert_eq!(safe_truncate_str(s, 5), "你");
        assert_eq!(safe_truncate_str(s, 6), "你好");
        // Capping at 14 bytes (middle of 4-byte 🦀) should safely stop before 🦀
        assert_eq!(safe_truncate_str(s, 14), "你好世界");
        assert_eq!(safe_truncate_str(s, 16), "你好世界🦀");
        assert_eq!(safe_truncate_str(s, 100), s);
    }

    #[test]
    fn test_chunk_exact_reconstruction() {
        let mut graph = VectorGraph::new();
        let lines: Vec<String> = (1..=20).map(|i| format!("    line_{};", i)).collect();
        let content = format!("fn big_func() {{\n{}\n}}\n", lines.join("\n"));
        let sym = Symbol {
            name: "big_func".to_string(),
            kind: SymbolKind::Function,
            start_line: 1,
            end_line: 22,
            signature: "fn big_func()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node("src/lib.rs", &content, vec![sym]));

        // Retrieve with max_lines = 10
        let res = graph.get_symbol_body_opt("big_func", Some(Path::new("src/lib.rs")), 10);
        assert!(res.is_some());
        let chunk1 = res.unwrap();
        assert!(chunk1.is_truncated);
        assert_eq!(chunk1.returned_lines, 10);
        assert_eq!(chunk1.total_lines, 22);

        // Parse hint to verify exact start and end lines
        let hint = chunk1.next_query_hint.expect("hint must exist");
        assert!(hint.contains("start=11, end=22"));

        // Get remainder using get_file_lines(start=11, end=22)
        let remainder = graph.get_file_lines(Path::new("src/lib.rs"), 11, 22).unwrap();
        let remainder_lines: Vec<&str> = remainder
            .lines()
            .map(|l| {
                if let Some((_, code)) = l.split_once('|') {
                    code.strip_prefix(' ').unwrap_or(code)
                } else {
                    l
                }
            })
            .collect();

        // Verify chunk1 returned lines + remainder lines match original content lines exactly
        let chunk1_code_lines: Vec<&str> = chunk1.body.lines().take(chunk1.returned_lines).collect();
        let combined: Vec<&str> = chunk1_code_lines.into_iter().chain(remainder_lines).collect();
        let original_lines: Vec<&str> = content.lines().collect();
        assert_eq!(combined, original_lines);
    }

    #[test]
    fn test_deterministic_relevance_ranking() {
        let mut graph = VectorGraph::new();
        // Mock symbol in test file
        let sym_mock = Symbol {
            name: "run".to_string(),
            kind: SymbolKind::Method,
            start_line: 10,
            end_line: 20,
            signature: "fn run(&self)".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node("tests/mocks/app_mock.rs", "fn run() {}", vec![sym_mock]));

        // Real entry point in lib.rs
        let sym_real = Symbol {
            name: "run".to_string(),
            kind: SymbolKind::Function,
            start_line: 5,
            end_line: 15,
            signature: "pub fn run()".to_string(),
            doc_comment: None,
        };
        graph.upsert_file(make_test_node("src/lib.rs", "pub fn run() {}", vec![sym_real]));

        let matches = graph.find_symbol_filtered("run", None, Some(5));
        assert_eq!(matches.len(), 2);
        // The real entry point in src/lib.rs must be ranked #1
        assert_eq!(matches[0].0, PathBuf::from("src/lib.rs"));
        assert_eq!(matches[0].1.name, "run");
        // The test mock must be demoted to #2
        assert_eq!(matches[1].0, PathBuf::from("tests/mocks/app_mock.rs"));
    }
}
