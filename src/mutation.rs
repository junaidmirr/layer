use crate::graph::VectorGraph;
use crate::package_indexer::PackageIndexer;
use crate::verifier::{DiagnosticItem, VerificationReport, Verifier};
use serde::{Deserialize, Serialize};
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

    /// Roll back the most recent mutation from in-memory snapshot
    pub fn rollback_last_edit(
        root: &Path,
        graph: &mut VectorGraph,
    ) -> Result<RollbackResult, String> {
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
