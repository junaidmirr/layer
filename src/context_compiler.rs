use crate::graph::VectorGraph;
use crate::models::{Symbol, SymbolKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
pub struct TaskContextPacket {
    pub task: String,
    pub interfaces: Vec<TaskContextItem>,
    pub types: Vec<TaskContextItem>,
    pub errors: Vec<TaskContextItem>,
    pub imports: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_dependencies: Vec<String>,
    pub budget: TaskContextBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Interface,
    Error,
    Type,
    Function,
}

struct ScoredCandidate {
    symbol: Symbol,
    file_path: PathBuf,
    score: i32,
    category: Category,
}

pub struct ContextCompiler;

impl ContextCompiler {
    /// Compiles a task-specific, token-bounded context packet using deterministic graph analysis
    pub fn compile_task_context(
        graph: &VectorGraph,
        task: &str,
        target_path: Option<&Path>,
        max_tokens: usize,
    ) -> TaskContextPacket {
        let task_lower = task.to_lowercase();
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
        ]
        .into_iter()
        .collect();

        let raw_keywords: Vec<String> = task_lower
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| !w.is_empty() && w.len() > 1 && !stop_words.contains(w))
            .map(|s| s.to_string())
            .collect();

        let mut candidates = Vec::new();

        for (path, node) in &graph.files {
            let path_str = path.to_string_lossy().to_lowercase();

            // Graph proximity scoring
            let mut proximity_score = 0;
            if let Some(target) = target_path {
                let target_rel = target.to_string_lossy().to_lowercase();
                if path_str == target_rel {
                    proximity_score += 60;
                } else if let Some(target_dir) = target.parent()
                    && path.starts_with(target_dir)
                {
                    proximity_score += 30;
                }
            }

            for sym in &node.symbols {
                let name_lower = sym.name.to_lowercase();
                let mut match_score = 0;

                // Match against keywords
                for kw in &raw_keywords {
                    if name_lower == *kw {
                        match_score += 100;
                    } else if name_lower.contains(kw) || kw.contains(&name_lower) {
                        match_score += 40;
                    }
                }

                // Check direct presence in task string
                if task_lower.contains(&name_lower) {
                    match_score += 80;
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
                    Category::Interface => 40,
                    Category::Error => 35,
                    Category::Type => 25,
                    Category::Function => 10,
                };

                let total_score = match_score + proximity_score + category_bonus;

                // Only retain items that have genuine task relevance or proximity
                if match_score > 0 || (proximity_score > 0 && category != Category::Function) {
                    candidates.push(ScoredCandidate {
                        symbol: sym.clone(),
                        file_path: path.clone(),
                        score: total_score,
                        category,
                    });
                }
            }
        }

        // Deterministic sort: descending by score, then path, then start_line
        candidates.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
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

        let mut interfaces = Vec::new();
        let mut types = Vec::new();
        let mut errors = Vec::new();
        let mut imports_set = HashSet::new();
        let mut relevant_files = HashSet::new();

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
                + 60; // structural overhead
            let item_tokens = item_chars.div_ceil(4);

            if estimated_tokens + item_tokens > max_tokens {
                is_truncated = true;
                omitted_count += 1;
                omitted_symbols.push(candidate.symbol.name);
                continue;
            }

            estimated_tokens += item_tokens;
            relevant_files.insert(candidate.file_path.clone());

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
        for file_path in &relevant_files {
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
            // If import starts with crate:: or super::, check if target module exists
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

        TaskContextPacket {
            task: task.to_string(),
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

        // Interface trait
        let sym_repo = Symbol {
            name: "UserRepository".to_string(),
            kind: SymbolKind::Trait,
            start_line: 1,
            end_line: 10,
            signature: "pub trait UserRepository".to_string(),
            doc_comment: Some("Repository for users".to_string()),
        };
        // Model type
        let sym_user = Symbol {
            name: "User".to_string(),
            kind: SymbolKind::Struct,
            start_line: 12,
            end_line: 20,
            signature: "pub struct User".to_string(),
            doc_comment: None,
        };
        // Error enum
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
    }

    #[test]
    fn test_context_budgeting_enforces_ceiling_and_reports_omissions() {
        let mut graph = VectorGraph::new();

        // Insert 10 struct symbols
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

        // Very tight token budget of 100 tokens
        let packet = ContextCompiler::compile_task_context(
            &graph,
            "UserDataModel",
            Some(Path::new("src/models.rs")),
            100,
        );

        // Budget ceiling enforced
        assert!(packet.budget.is_truncated);
        assert!(packet.budget.omitted_count > 0);
        assert!(!packet.budget.omitted_symbols.is_empty());
        assert!(packet.budget.estimated_tokens <= 120); // within minor padding boundary
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
