use crate::models::FileNode;
use crate::scanner::AstScanner;
use ignore::WalkBuilder;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CodebaseScanner {
    root_path: PathBuf,
}

impl CodebaseScanner {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root_path.into(),
        }
    }

    /// Recursively scan codebase respecting .gitignore and binary file boundaries
    pub fn scan_all(&self) -> Vec<FileNode> {
        let walker = WalkBuilder::new(&self.root_path)
            .hidden(true) // skip hidden (.git, etc.)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        let paths: Vec<PathBuf> = walker
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .map(|e| e.into_path())
            .collect();

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);

        if paths.len() < 16 || num_threads <= 1 {
            return paths.iter().filter_map(|p| self.scan_file(p)).collect();
        }

        let chunk_size = paths.len().div_ceil(num_threads);
        let mut results = Vec::with_capacity(paths.len());

        std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(num_threads);
            for chunk in paths.chunks(chunk_size) {
                handles.push(s.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|p| self.scan_file(p))
                        .collect::<Vec<FileNode>>()
                }));
            }
            for handle in handles {
                if let Ok(mut part) = handle.join() {
                    results.append(&mut part);
                }
            }
        });

        results
    }

    /// Read and parse a single file into a FileNode
    pub fn scan_file(&self, abs_path: &Path) -> Option<FileNode> {
        // Skip common large/binary file types
        if is_likely_binary_or_asset(abs_path) {
            return None;
        }

        let metadata = fs::metadata(abs_path).ok()?;
        // Skip files larger than 2MB
        if metadata.len() > 2 * 1024 * 1024 {
            return None;
        }

        let content = match fs::read_to_string(abs_path) {
            Ok(c) => c,
            Err(_) => return None, // Binary or invalid UTF-8
        };

        let relative_path = match abs_path.strip_prefix(&self.root_path) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => abs_path.to_path_buf(),
        };

        let extension = abs_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_string());

        let line_count = content.lines().count();
        let (symbols, imports) =
            AstScanner::extract_symbols_and_imports(&content, extension.as_deref());

        // Fast 64-bit FNV-1a content hash for deterministic version verification
        let mut hasher = 0xcbf29ce484222325u64;
        for byte in content.as_bytes() {
            hasher ^= *byte as u64;
            hasher = hasher.wrapping_mul(0x100000001b3);
        }

        Some(FileNode {
            relative_path,
            absolute_path: abs_path.to_path_buf(),
            extension,
            line_count,
            size_bytes: metadata.len(),
            content_hash: hasher,
            last_modified: metadata.modified().ok(),
            symbols,
            imports,
            content,
            vector: Vec::new(), // Populated by VectorGraph
        })
    }
}

fn is_likely_binary_or_asset(path: &Path) -> bool {
    let binary_extensions = [
        "png", "jpg", "jpeg", "gif", "ico", "svg", "webp", "pdf", "zip", "tar", "gz", "bz2", "7z",
        "rar", "exe", "dll", "so", "dylib", "bin", "o", "a", "wasm", "pyc", "class", "lock",
    ];

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if binary_extensions.contains(&ext_lower.as_str()) {
            return true;
        }
    }

    // Ignore .git, target/, node_modules/ in path components
    for component in path.components() {
        let comp_str = component.as_os_str().to_string_lossy();
        if comp_str == ".git"
            || comp_str == "target"
            || comp_str == "node_modules"
            || comp_str == ".gemini"
        {
            return true;
        }
    }

    false
}
