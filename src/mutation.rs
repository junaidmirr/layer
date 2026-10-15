use crate::graph::VectorGraph;
use crate::package_indexer::PackageIndexer;
use crate::scanner::AstScanner;
use crate::verifier::{DiagnosticItem, VerificationReport, Verifier};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyMode {
    Fast,
    Full,
    None,
}

impl VerifyMode {
    pub fn parse(mode: Option<&str>, legacy_auto_verify: Option<bool>) -> Self {
        if let Some(m) = mode {
            match m.to_lowercase().as_str() {
                "fast" => VerifyMode::Fast,
                "full" => VerifyMode::Full,
                "none" => VerifyMode::None,
                _ => VerifyMode::Fast,
            }
        } else if let Some(legacy) = legacy_auto_verify {
            if legacy {
                VerifyMode::Fast
            } else {
                VerifyMode::None
            }
        } else {
            VerifyMode::Fast
        }
    }
}

#[derive(Debug, Clone)]
pub struct MutationSnapshot {
    pub rel_path: PathBuf,
    pub previous_content: Option<String>,
    pub previous_hash: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerReport {
    pub triggered: bool,
    pub rolled_back: bool,
    pub reason: String,
    pub cascading_error_count: usize,
    pub broken_downstream_files: Vec<String>,
    pub advisory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownstreamUpdateItem {
    pub file: String,
    pub line: usize,
    pub symbol: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResult {
    pub success: bool,
    pub operation: String,
    pub path: String,
    pub bytes_written: usize,
    pub line_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub circuit_breaker: Option<CircuitBreakerReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_downstream_updates: Vec<DownstreamUpdateItem>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResult {
    pub success: bool,
    pub path: String,
    pub previous_bytes: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSymbolReport {
    pub success: bool,
    pub old_name: String,
    pub new_name: String,
    pub files_modified: Vec<String>,
    pub total_occurrences_replaced: usize,
    pub llm_turns_saved: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<VerificationReport>,
    pub message: String,
}

pub struct MutationManager;

impl MutationManager {
    /// Write full content to a file with snapshotting, instant graph sync,
    /// two-tier verification (Fast/Full/None), and cascading error circuit breaker.
    pub fn write_file(
        root: &Path,
        rel_path: &Path,
        content: &str,
        verify_mode: VerifyMode,
        auto_rollback_on_cascade: bool,
        graph: &mut VectorGraph,
        pkg_indexer: Option<&mut PackageIndexer>,
    ) -> Result<MutationResult, String> {
        let abs_path = root.join(rel_path);

        // 1. Take lightweight pre-edit snapshot
        let prev_content = if abs_path.exists() {
            fs::read_to_string(&abs_path).ok()
        } else {
            None
        };

        let prev_hash = prev_content.as_ref().map(|c| {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in c.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h
        });

        let snapshot = MutationSnapshot {
            rel_path: rel_path.to_path_buf(),
            previous_content: prev_content.clone(),
            previous_hash: prev_hash,
        };
        graph.last_mutation_snapshot = Some(snapshot);

        // 2. Ensure parent directory exists and write content
        if let Some(parent) = abs_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create parent directory '{}': {}",
                    parent.display(),
                    e
                )
            })?;
        }

        fs::write(&abs_path, content)
            .map_err(|e| format!("Failed to write to '{}': {}", abs_path.display(), e))?;

        // 3. Immediately refresh file in VectorGraph
        graph.sync_workspace(root);

        let bytes_written = content.len();
        let line_count = content.lines().count();

        // 4. Run verification based on verify_mode
        let verification = match verify_mode {
            VerifyMode::None => None,
            VerifyMode::Fast => {
                let v = Verifier::verify_fast_ast_only(root, graph, pkg_indexer);
                graph.last_verification = Some(v.diagnostics.clone());
                Some(v)
            }
            VerifyMode::Full => {
                let v = Verifier::verify_workspace_with_circuit_breaker(root, graph, pkg_indexer);
                graph.last_verification = Some(v.diagnostics.clone());
                Some(v)
            }
        };

        // If verification is disabled, return early
        let verif = match verification {
            None => {
                return Ok(MutationResult {
                    success: true,
                    operation: "write".to_string(),
                    path: rel_path.to_string_lossy().to_string(),
                    bytes_written,
                    line_count,
                    verification: None,
                    circuit_breaker: None,
                    pending_downstream_updates: Vec::new(),
                    message: format!(
                        "Successfully wrote {} bytes to '{}'.",
                        bytes_written,
                        rel_path.display()
                    ),
                });
            }
            Some(v) => v,
        };

        // 5. Partition errors: local errors in target file vs downstream errors
        let rel_path_str = rel_path.to_string_lossy();
        let normalize = |p: &str| -> String {
            p.strip_prefix("./")
                .unwrap_or(p)
                .replace('\\', "/")
                .to_string()
        };
        let target_norm = normalize(&rel_path_str);

        let new_errors = verif
            .comparison
            .as_ref()
            .map(|c| &c.new_errors)
            .unwrap_or(&verif.diagnostics);

        let (local_errors, downstream_errors): (Vec<&DiagnosticItem>, Vec<&DiagnosticItem>) =
            new_errors
                .iter()
                .partition(|e| normalize(&e.file) == target_norm);

        let has_local_fatal = local_errors.iter().any(|e| {
            e.code.as_deref() == Some("SYNTAX_ERROR")
                || e.code.as_deref() == Some("TS2307")
                || e.code.as_deref() == Some("E0432")
                || e.code.as_deref() == Some("E0433")
        });

        let target_file_is_invalid = has_local_fatal || local_errors.len() >= 4;

        if target_file_is_invalid {
            let reason = if has_local_fatal {
                let first_fatal = local_errors
                    .iter()
                    .find(|e| {
                        e.code.as_deref() == Some("SYNTAX_ERROR")
                            || e.code.as_deref() == Some("TS2307")
                            || e.code.as_deref() == Some("E0432")
                            || e.code.as_deref() == Some("E0433")
                    })
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Fatal syntax/import error in target file".to_string());
                format!("Fatal error in '{}': {}", rel_path.display(), first_fatal)
            } else {
                format!(
                    "Cascading compilation failure in '{}': {} new error(s).",
                    rel_path.display(),
                    local_errors.len()
                )
            };

            let rolled_back = if auto_rollback_on_cascade {
                if let Some(ref old_content) = prev_content {
                    let _ = fs::write(&abs_path, old_content);
                } else {
                    let _ = fs::remove_file(&abs_path);
                }
                graph.sync_workspace(root);
                true
            } else {
                false
            };

            let circuit_breaker = Some(CircuitBreakerReport {
                triggered: true,
                rolled_back,
                reason,
                cascading_error_count: local_errors.len(),
                broken_downstream_files: vec![],
                advisory: if rolled_back {
                    "Auto-rolled back to previous state. Inspect error before reapplying changes."
                        .to_string()
                } else {
                    "Fatal error in target file. Use `rollback_last_edit` to revert immediately."
                        .to_string()
                },
            });

            if rolled_back {
                return Ok(MutationResult {
                    success: false,
                    operation: "write (rolled back)".to_string(),
                    path: rel_path.to_string_lossy().to_string(),
                    bytes_written: 0,
                    line_count: 0,
                    verification: Some(verif),
                    circuit_breaker,
                    pending_downstream_updates: Vec::new(),
                    message: format!(
                        "Circuit Breaker Triggered: Edit to '{}' was automatically reverted due to fatal errors.",
                        rel_path.display()
                    ),
                });
            }
        } else if !downstream_errors.is_empty() {
            // Target file is valid! Do NOT rollback interface/type edits when local errors are 0.
            let broken_downstream: Vec<String> = {
                let mut set = std::collections::BTreeSet::new();
                for d in &downstream_errors {
                    set.insert(d.file.clone());
                }
                set.into_iter().collect()
            };

            let pending_downstream_updates: Vec<DownstreamUpdateItem> = downstream_errors
                .iter()
                .map(|d| {
                    let sym = Verifier::extract_identifiers_from_diagnostic(&d.message)
                        .into_iter()
                        .next();
                    DownstreamUpdateItem {
                        file: d.file.clone(),
                        line: d.line,
                        symbol: sym,
                        message: d.message.clone(),
                    }
                })
                .collect();

            let circuit_breaker = Some(CircuitBreakerReport {
                triggered: true,
                rolled_back: false,
                reason: format!(
                    "Multi-file refactor in progress: Target file '{}' is valid, but {} downstream file(s) require updates: {}",
                    rel_path.display(),
                    broken_downstream.len(),
                    broken_downstream.join(", ")
                ),
                cascading_error_count: downstream_errors.len(),
                broken_downstream_files: broken_downstream.clone(),
                advisory: "Target file edit preserved. Proceed to update downstream files sequentially using pending_downstream_updates."
                    .to_string(),
            });

            return Ok(MutationResult {
                success: true,
                operation: "write".to_string(),
                path: rel_path.to_string_lossy().to_string(),
                bytes_written,
                line_count,
                verification: Some(verif),
                circuit_breaker,
                pending_downstream_updates,
                message: format!(
                    "Successfully wrote '{}'. Multi-file refactor in progress: {} downstream file(s) require follow-up updates.",
                    rel_path.display(),
                    broken_downstream.len()
                ),
            });
        }

        Ok(MutationResult {
            success: true,
            operation: "write".to_string(),
            path: rel_path.to_string_lossy().to_string(),
            bytes_written,
            line_count,
            verification: Some(verif),
            circuit_breaker: None,
            pending_downstream_updates: Vec::new(),
            message: format!(
                "Successfully wrote '{}' and verified with 0 circuit breaker faults.",
                rel_path.display()
            ),
        })
    }

    /// Surgically edit a file by replacing a target snippet with replacement
    #[allow(clippy::too_many_arguments)]
    pub fn edit_file(
        root: &Path,
        rel_path: &Path,
        target: &str,
        replacement: &str,
        verify_mode: VerifyMode,
        auto_rollback_on_cascade: bool,
        graph: &mut VectorGraph,
        pkg_indexer: Option<&mut PackageIndexer>,
    ) -> Result<MutationResult, String> {
        let abs_path = root.join(rel_path);
        if !abs_path.exists() {
            return Err(format!("File '{}' does not exist.", rel_path.display()));
        }

        let existing = fs::read_to_string(&abs_path)
            .map_err(|e| format!("Failed to read '{}': {}", abs_path.display(), e))?;

        if !existing.contains(target) {
            return Err(format!(
                "Target content not found in '{}'. Ensure target exactly matches existing file contents.",
                rel_path.display()
            ));
        }

        let matches_count = existing.matches(target).count();
        if matches_count > 1 {
            return Err(format!(
                "Target content found {} times in '{}'. Provide a larger target snippet to disambiguate.",
                matches_count,
                rel_path.display()
            ));
        }

        let new_content = existing.replacen(target, replacement, 1);
        Self::write_file(
            root,
            rel_path,
            &new_content,
            verify_mode,
            auto_rollback_on_cascade,
            graph,
            pkg_indexer,
        )
    }

    /// Atomically rename an AST identifier across the definition file and all dependent files
    pub fn rename_symbol(
        root: &Path,
        rel_path: &Path,
        old_name: &str,
        new_name: &str,
        verify_mode: VerifyMode,
        graph: &mut VectorGraph,
        pkg_indexer: Option<&mut PackageIndexer>,
    ) -> Result<RenameSymbolReport, String> {
        let old_clean = old_name.trim();
        let new_clean = new_name.trim();

        if old_clean.is_empty() || new_clean.is_empty() {
            return Err("Symbol names cannot be empty.".to_string());
        }
        if old_clean == new_clean {
            return Err(format!(
                "New name '{}' is identical to old name.",
                new_clean
            ));
        }

        // Validate new identifier
        if !new_clean
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            || new_clean.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(format!("'{}' is not a valid identifier.", new_clean));
        }

        let abs_path = root.join(rel_path);
        if !abs_path.exists() {
            return Err(format!(
                "Target file '{}' does not exist.",
                rel_path.display()
            ));
        }

        // Determine scope of candidate files:
        // 1. The target file itself
        // 2. All downstream dependents of target file
        // 3. Any indexed file that imports or mentions old_name
        let mut candidate_files: HashSet<PathBuf> = HashSet::new();
        candidate_files.insert(crate::graph::normalize_relative_path(rel_path));

        let dependents = graph.get_downstream_dependents(rel_path);
        for dep in dependents {
            candidate_files.insert(crate::graph::normalize_relative_path(&dep));
        }

        for (p, node) in &graph.files {
            if node.content.contains(old_clean) {
                candidate_files.insert(p.clone());
            }
        }

        // Gather occurrences per candidate file
        let mut affected_files_data = Vec::new();
        for file_rel in &candidate_files {
            let file_abs = root.join(file_rel);
            if !file_abs.exists() {
                continue;
            }
            let Ok(content) = fs::read_to_string(&file_abs) else {
                continue;
            };
            let ext = file_rel
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();

            let occurrences = AstScanner::find_identifier_occurrences(&content, &ext, old_clean);
            if !occurrences.is_empty() {
                affected_files_data.push((file_rel.clone(), file_abs, content, occurrences));
            }
        }

        if affected_files_data.is_empty() {
            return Ok(RenameSymbolReport {
                success: true,
                old_name: old_clean.to_string(),
                new_name: new_clean.to_string(),
                files_modified: vec![],
                total_occurrences_replaced: 0,
                llm_turns_saved: 0,
                verification: None,
                message: format!(
                    "Symbol '{}' was not found in any code occurrences in target or dependent files.",
                    old_clean
                ),
            });
        }

        // Build transactional snapshot
        let mut snapshots = Vec::new();
        for (rel, _abs, content, _occ) in &affected_files_data {
            snapshots.push(MutationSnapshot {
                rel_path: rel.clone(),
                previous_content: Some(content.clone()),
                previous_hash: None,
            });
        }

        // Apply replacements in descending byte order per file
        let mut total_occurrences_replaced = 0;
        let mut files_modified = Vec::new();

        for (rel, abs, content, mut occurrences) in affected_files_data {
            // Sort descending so earlier byte offsets don't shift
            occurrences.sort_by_key(|b| std::cmp::Reverse(b.0));
            total_occurrences_replaced += occurrences.len();

            let mut new_content = content.clone();
            for (start, end) in occurrences {
                if start <= end && end <= new_content.len() {
                    new_content.replace_range(start..end, new_clean);
                }
            }

            if let Err(e) = fs::write(&abs, &new_content) {
                // Roll back any files already written!
                for s in &snapshots {
                    if let Some(ref prev) = s.previous_content {
                        let _ = fs::write(root.join(&s.rel_path), prev);
                    }
                }
                return Err(format!(
                    "Failed to write modified file '{}': {}",
                    abs.display(),
                    e
                ));
            }

            files_modified.push(rel.to_string_lossy().to_string());
        }

        // Sync workspace graph immediately
        graph.sync_workspace(root);

        // Verification phase
        let mut verification_report = None;
        if verify_mode != VerifyMode::None {
            // Check AST syntax across workspace
            let precheck_diags =
                Verifier::instant_ast_precheck(root, &graph.files, &graph.path_aliases);
            let has_syntax_error = precheck_diags
                .iter()
                .any(|d| d.code.as_deref() == Some("SYNTAX_ERROR"));

            if has_syntax_error {
                // Auto-rollback all touched files!
                for s in &snapshots {
                    if let Some(ref prev) = s.previous_content {
                        let _ = fs::write(root.join(&s.rel_path), prev);
                    }
                }
                graph.sync_workspace(root);
                return Err(
                    "Rename caused syntax error across affected files; automatically rolled back all changes."
                        .to_string(),
                );
            }

            if verify_mode == VerifyMode::Full {
                let rep = Verifier::verify_workspace_with_circuit_breaker(root, graph, pkg_indexer);
                verification_report = Some(rep);
            }
        }

        // Store multi-file snapshot for user rollback
        graph.last_multi_file_snapshot = Some(snapshots);

        let k = files_modified.len();
        let llm_turns_saved = k.saturating_sub(1);

        Ok(RenameSymbolReport {
            success: true,
            old_name: old_clean.to_string(),
            new_name: new_clean.to_string(),
            files_modified,
            total_occurrences_replaced,
            llm_turns_saved,
            verification: verification_report,
            message: format!(
                "Successfully renamed '{}' to '{}' across {} file(s) ({} total occurrences replaced). Saved {} LLM turns.",
                old_clean, new_clean, k, total_occurrences_replaced, llm_turns_saved
            ),
        })
    }

    /// Roll back the most recent mutation from in-memory snapshot (supports single-file and multi-file atomic rollbacks)
    pub fn rollback_last_edit(
        root: &Path,
        graph: &mut VectorGraph,
    ) -> Result<RollbackResult, String> {
        // Multi-file atomic rollback
        if let Some(multi) = graph.last_multi_file_snapshot.take() {
            let mut total_bytes = 0;
            let mut restored_files = Vec::new();
            for s in &multi {
                let abs = root.join(&s.rel_path);
                if let Some(ref prev) = s.previous_content {
                    let _ = fs::write(&abs, prev);
                    total_bytes += prev.len();
                } else if abs.exists() {
                    let _ = fs::remove_file(&abs);
                }
                restored_files.push(s.rel_path.to_string_lossy().to_string());
            }
            graph.sync_workspace(root);
            return Ok(RollbackResult {
                success: true,
                path: restored_files.join(", "),
                previous_bytes: total_bytes,
                message: format!(
                    "Successfully rolled back multi-file operation across {} file(s).",
                    restored_files.len()
                ),
            });
        }

        // Single-file rollback
        let snapshot = match graph.last_mutation_snapshot.take() {
            Some(s) => s,
            None => return Err("No previous edit snapshot available for rollback.".to_string()),
        };

        let abs_path = root.join(&snapshot.rel_path);

        match snapshot.previous_content {
            Some(old_content) => {
                let bytes = old_content.len();
                fs::write(&abs_path, &old_content).map_err(|e| {
                    format!("Failed to restore file '{}': {}", abs_path.display(), e)
                })?;
                graph.sync_workspace(root);
                Ok(RollbackResult {
                    success: true,
                    path: snapshot.rel_path.to_string_lossy().to_string(),
                    previous_bytes: bytes,
                    message: format!(
                        "Successfully restored '{}' to previous state ({} bytes).",
                        snapshot.rel_path.display(),
                        bytes
                    ),
                })
            }
            None => {
                // File was created in the last step, rollback means deleting it
                if abs_path.exists() {
                    let _ = fs::remove_file(&abs_path);
                }
                graph.sync_workspace(root);
                Ok(RollbackResult {
                    success: true,
                    path: snapshot.rel_path.to_string_lossy().to_string(),
                    previous_bytes: 0,
                    message: format!(
                        "Successfully deleted newly created file '{}'.",
                        snapshot.rel_path.display()
                    ),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_mutation_write_and_rollback() {
        let dir = tempdir().unwrap();
        let mut graph = VectorGraph::new();

        let rel_file = Path::new("src/main.rs");
        let initial_code = "fn main() { println!(\"v1\"); }\n";

        // 1. Initial write
        let res1 = MutationManager::write_file(
            dir.path(),
            rel_file,
            initial_code,
            VerifyMode::None,
            true,
            &mut graph,
            None,
        )
        .expect("write must succeed");
        assert!(res1.success);
        assert_eq!(res1.bytes_written, initial_code.len());
        assert!(dir.path().join(rel_file).exists());

        // 2. Edit file
        let res2 = MutationManager::edit_file(
            dir.path(),
            rel_file,
            "v1",
            "v2",
            VerifyMode::None,
            true,
            &mut graph,
            None,
        )
        .expect("edit must succeed");
        assert!(res2.success);

        let content_after_edit = fs::read_to_string(dir.path().join(rel_file)).unwrap();
        assert!(content_after_edit.contains("v2"));

        // 3. Rollback edit
        let rollback_res = MutationManager::rollback_last_edit(dir.path(), &mut graph)
            .expect("rollback must succeed");
        assert!(rollback_res.success);

        let content_after_rollback = fs::read_to_string(dir.path().join(rel_file)).unwrap();
        assert!(content_after_rollback.contains("v1"));
        assert!(!content_after_rollback.contains("v2"));
    }
}
