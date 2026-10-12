use crate::models::{FileOperation, HistoryRecord};
use colored::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct HistoryCache {
    records: Vec<HistoryRecord>,
    next_id: usize,
    /// Keep snapshots of files across history versions: file_path -> list of (timestamp, content)
    snapshots: HashMap<PathBuf, Vec<(String, String)>>,
    /// Content hash / checksum map to detect moves and copies
    hash_to_path: HashMap<u64, PathBuf>,
}

impl Default for HistoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryCache {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            snapshots: HashMap::new(),
            hash_to_path: HashMap::new(),
        }
    }

    /// Record initial file creation / baseline indexing
    pub fn record_initial(&mut self, path: &Path, content: &str) {
        let hash = compute_hash(content);
        self.hash_to_path.insert(hash, path.to_path_buf());
        let list = self.snapshots.entry(path.to_path_buf()).or_default();
        if list.is_empty() {
            list.push((current_time_str(), content.to_string()));
        }
    }

    /// Record a file change (Created, Modified, Deleted, Moved, Copied)
    pub fn record_change(
        &mut self,
        path: &Path,
        old_content: Option<String>,
        new_content: Option<String>,
        symbols_before: &[String],
        symbols_after: &[String],
    ) -> HistoryRecord {
        let id = self.next_id;
        self.next_id += 1;
        let ts = current_time_str();

        let new_hash = new_content.as_deref().map(compute_hash);

        // Detect Move or Copy
        let operation = match (&old_content, &new_content) {
            (None, Some(new_c)) => {
                let h = compute_hash(new_c);
                if let Some(origin) = self.hash_to_path.get(&h) {
                    if origin != path {
                        if !origin.exists() {
                            FileOperation::Moved {
                                from: origin.clone(),
                                to: path.to_path_buf(),
                            }
                        } else {
                            FileOperation::Copied {
                                from: origin.clone(),
                                to: path.to_path_buf(),
                            }
                        }
                    } else {
                        FileOperation::Created
                    }
                } else {
                    FileOperation::Created
                }
            }
            (Some(_), Some(_)) => FileOperation::Modified,
            (Some(_), None) => FileOperation::Deleted,
            (None, None) => FileOperation::Modified,
        };

        // If moved, update origin mapping
        if let FileOperation::Moved { ref from, .. } = operation {
            self.hash_to_path.remove(&compute_hash(new_content.as_ref().unwrap()));
            self.hash_to_path.insert(compute_hash(new_content.as_ref().unwrap()), path.to_path_buf());
            let _ = from;
        } else if let Some(h) = new_hash {
            self.hash_to_path.insert(h, path.to_path_buf());
        }

        // Compute unified diff if applicable
        let diff = match (&old_content, &new_content) {
            (Some(old_c), Some(new_c)) => {
                if old_c == new_c {
                    None
                } else {
                    Some(generate_diff(path, old_c, new_c))
                }
            }
            (Some(old_c), None) => {
                Some(generate_deletion_diff(path, old_c))
            }
            (None, Some(new_c)) => {
                Some(generate_creation_diff(path, new_c))
            }
            (None, None) => None,
        };

        // Determine symbol differences
        let mut affected = Vec::new();
        for sym in symbols_before {
            if !symbols_after.contains(sym) {
                affected.push(format!("- {}", sym));
            }
        }
        for sym in symbols_after {
            if !symbols_before.contains(sym) {
                affected.push(format!("+ {}", sym));
            }
        }

        let desc = match &operation {
            FileOperation::Created => format!("File '{}' was created ({} lines)", path.display(), new_content.as_ref().map(|c| c.lines().count()).unwrap_or(0)),
            FileOperation::Modified => format!("File '{}' was modified", path.display()),
            FileOperation::Deleted => format!("File '{}' was deleted (previously {} lines)", path.display(), old_content.as_ref().map(|c| c.lines().count()).unwrap_or(0)),
            FileOperation::Moved { from, to } => format!("File moved from '{}' to '{}'", from.display(), to.display()),
            FileOperation::Copied { from, to } => format!("File copied from '{}' to '{}'", from.display(), to.display()),
        };

        // Store snapshot
        if let Some(new_c) = &new_content {
            self.snapshots.entry(path.to_path_buf()).or_default().push((ts.clone(), new_c.clone()));
        }

        let record = HistoryRecord {
            id,
            timestamp: ts,
            file_path: path.to_path_buf(),
            operation,
            description: desc,
            old_content,
            new_content,
            diff,
            affected_symbols: affected,
        };

        self.records.push(record.clone());
        record
    }

    /// Retrieve all history records (newest first or chronologically)
    pub fn get_all_records(&self) -> &[HistoryRecord] {
        &self.records
    }

    /// Retrieve history records for a specific file
    pub fn get_file_records(&self, path: &Path) -> Vec<&HistoryRecord> {
        self.records.iter().filter(|r| r.file_path == path).collect()
    }

    /// Retrieve a specific history record by ID
    pub fn get_record_by_id(&self, id: usize) -> Option<&HistoryRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Retrieve past snapshots of a file
    pub fn get_file_snapshots(&self, path: &Path) -> Option<&[(String, String)]> {
        self.snapshots.get(path).map(|v| v.as_slice())
    }

    /// Format history summary for CLI or REPL
    pub fn format_history(&self, target_file: Option<&Path>) -> String {
        let matching: Vec<&HistoryRecord> = match target_file {
            Some(p) => self.get_file_records(p),
            None => self.records.iter().collect(),
        };

        if matching.is_empty() {
            return "No codebase history recorded yet. Edits, creations, and deletions will appear here in real time.".to_string();
        }

        let mut out = String::new();
        out.push_str(&format!("📜 Codebase Change History ({} events):\n", matching.len()));
        out.push_str("======================================================================\n");

        for rec in matching.iter().rev() {
            let op_str = match &rec.operation {
                FileOperation::Created => format!("[{}]", rec.operation).green().bold(),
                FileOperation::Modified => format!("[{}]", rec.operation).yellow().bold(),
                FileOperation::Deleted => format!("[{}]", rec.operation).red().bold(),
                FileOperation::Moved { .. } => format!("[{}]", rec.operation).cyan().bold(),
                FileOperation::Copied { .. } => format!("[{}]", rec.operation).blue().bold(),
            };

            out.push_str(&format!(
                "#{:<3} {}  {}  {}\n",
                rec.id,
                rec.timestamp.dimmed(),
                op_str,
                rec.file_path.display().to_string().bold()
            ));
            out.push_str(&format!("     ↳ {}\n", rec.description));

            if !rec.affected_symbols.is_empty() {
                out.push_str(&format!("     ↳ Symbols: {}\n", rec.affected_symbols.join(", ")));
            }

            if let Some(diff) = &rec.diff {
                out.push_str("     ↳ Diff preview:\n");
                for line in diff.lines().take(12) {
                    let formatted_line = if line.starts_with('+') {
                        line.green()
                    } else if line.starts_with('-') {
                        line.red()
                    } else if line.starts_with('@') {
                        line.cyan()
                    } else {
                        line.normal()
                    };
                    out.push_str(&format!("       {}\n", formatted_line));
                }
                if diff.lines().count() > 12 {
                    out.push_str(&format!("       ... +{} more diff lines (use 'diff #{}')\n", diff.lines().count() - 12, rec.id));
                }
            }
            out.push_str("----------------------------------------------------------------------\n");
        }

        out
    }

    /// Format detailed diff and old content for a specific record ID
    pub fn format_record_detail(&self, id: usize) -> Option<String> {
        let rec = self.get_record_by_id(id)?;
        let mut out = String::new();
        out.push_str(&format!("📜 History Event #{}: {}\n", rec.id, rec.description));
        out.push_str(&format!("Timestamp: {}\n", rec.timestamp));
        out.push_str(&format!("File: {}\n", rec.file_path.display()));
        out.push_str(&format!("Operation: {}\n", rec.operation));
        out.push_str("======================================================================\n\n");

        if let Some(diff) = &rec.diff {
            out.push_str("Diff against prior state:\n");
            out.push_str("----------------------------------------------------------------------\n");
            for line in diff.lines() {
                let formatted = if line.starts_with('+') {
                    line.green()
                } else if line.starts_with('-') {
                    line.red()
                } else if line.starts_with('@') {
                    line.cyan()
                } else {
                    line.normal()
                };
                out.push_str(&format!("{}\n", formatted));
            }
            out.push_str("\n");
        }

        if let Some(old) = &rec.old_content {
            out.push_str("Older (Previous) File Content:\n");
            out.push_str("----------------------------------------------------------------------\n");
            for (idx, line) in old.lines().enumerate() {
                out.push_str(&format!("{:4} | {}\n", idx + 1, line));
            }
        } else {
            out.push_str("(No previous version — file was newly created)\n");
        }

        Some(out)
    }
}

fn compute_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in s.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn current_time_str() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02} UTC", hours, mins, s)
}

/// Compute simple unified line diff
pub fn generate_diff(path: &Path, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut diff = Vec::new();
    diff.push(format!("--- a/{}", path.display()));
    diff.push(format!("+++ b/{}", path.display()));

    let mut i = 0;
    let mut j = 0;

    while i < old_lines.len() || j < new_lines.len() {
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            i += 1;
            j += 1;
        } else {
            // Find difference block
            let start_i = i;
            let start_j = j;

            // Simple lookahead
            let mut match_found = false;
            let mut next_i = i;
            let mut next_j = j;

            for di in 0..10 {
                for dj in 0..10 {
                    if di == 0 && dj == 0 {
                        continue;
                    }
                    if i + di < old_lines.len() && j + dj < new_lines.len() && old_lines[i + di] == new_lines[j + dj] {
                        next_i = i + di;
                        next_j = j + dj;
                        match_found = true;
                        break;
                    }
                }
                if match_found {
                    break;
                }
            }

            let end_i = if match_found { next_i } else { old_lines.len() };
            let end_j = if match_found { next_j } else { new_lines.len() };

            diff.push(format!("@@ -{},{} +{},{} @@", start_i + 1, end_i - start_i, start_j + 1, end_j - start_j));

            for k in start_i..end_i {
                diff.push(format!("-{}", old_lines[k]));
            }
            for k in start_j..end_j {
                diff.push(format!("+{}", new_lines[k]));
            }

            i = end_i;
            j = end_j;
        }
    }

    diff.join("\n")
}

fn generate_creation_diff(path: &Path, new: &str) -> String {
    let mut diff = Vec::new();
    diff.push(format!("--- /dev/null"));
    diff.push(format!("+++ b/{}", path.display()));
    diff.push(format!("@@ -0,0 +1,{} @@", new.lines().count()));
    for line in new.lines().take(20) {
        diff.push(format!("+{}", line));
    }
    if new.lines().count() > 20 {
        diff.push(format!("+... (total {} lines)", new.lines().count()));
    }
    diff.join("\n")
}

fn generate_deletion_diff(path: &Path, old: &str) -> String {
    let mut diff = Vec::new();
    diff.push(format!("--- a/{}", path.display()));
    diff.push(format!("+++ /dev/null"));
    diff.push(format!("@@ -1,{} +0,0 @@", old.lines().count()));
    for line in old.lines().take(20) {
        diff.push(format!("-{}", line));
    }
    if old.lines().count() > 20 {
        diff.push(format!("-... (total {} lines)", old.lines().count()));
    }
    diff.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_cache_modification_and_diff() {
        let mut cache = HistoryCache::new();
        let path = Path::new("src/test.rs");

        cache.record_initial(path, "fn hello() {\n    println!(\"world\");\n}\n");

        let before = vec!["hello".to_string()];
        let after = vec!["hello".to_string(), "goodbye".to_string()];

        let rec = cache.record_change(
            path,
            Some("fn hello() {\n    println!(\"world\");\n}\n".to_string()),
            Some("fn hello() {\n    println!(\"world modified\");\n}\nfn goodbye() {}\n".to_string()),
            &before,
            &after,
        );

        assert_eq!(rec.id, 1);
        assert_eq!(rec.operation, FileOperation::Modified);
        assert!(rec.diff.is_some());
        assert!(rec.diff.unwrap().contains("-    println!(\"world\");"));
        assert_eq!(rec.affected_symbols, vec!["+ goodbye".to_string()]);

        let detail = cache.format_record_detail(1).expect("Record should exist");
        assert!(detail.contains("Older (Previous) File Content:"));
        assert!(detail.contains("println!(\"world\");"));
    }

    #[test]
    fn test_history_deletion() {
        let mut cache = HistoryCache::new();
        let path = Path::new("temp.txt");

        let rec = cache.record_change(
            path,
            Some("deleted text content".to_string()),
            None,
            &[],
            &[],
        );

        assert_eq!(rec.operation, FileOperation::Deleted);
        assert!(rec.old_content.is_some());
        assert!(rec.new_content.is_none());
        assert!(rec.diff.unwrap().contains("--- a/temp.txt"));
    }
}
