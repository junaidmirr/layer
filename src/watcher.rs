use crate::graph::VectorGraph;
use crate::walker::CodebaseScanner;
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct CodeWatcher {
    root_path: PathBuf,
    graph: Arc<Mutex<VectorGraph>>,
}

impl CodeWatcher {
    pub fn new(root_path: PathBuf, graph: Arc<Mutex<VectorGraph>>) -> Self {
        Self { root_path, graph }
    }

    /// Run the watcher loop on a separate background thread
    pub fn start_in_background(self) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let (tx, rx) = channel();
            let mut debouncer = match new_debouncer(Duration::from_millis(250), tx) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Failed to initialize file watcher: {}", e);
                    return;
                }
            };

            if let Err(e) = debouncer
                .watcher()
                .watch(&self.root_path, notify::RecursiveMode::Recursive)
            {
                eprintln!(
                    "Failed to watch root directory {}: {}",
                    self.root_path.display(),
                    e
                );
                return;
            }

            let scanner = CodebaseScanner::new(&self.root_path);

            for res in rx {
                match res {
                    Ok(events) => {
                        for event in events {
                            let path = event.path;
                            // Check if ignored or in target/.git
                            if should_ignore_change(&path) {
                                continue;
                            }

                            let mut graph_lock = match self.graph.lock() {
                                Ok(g) => g,
                                Err(p) => p.into_inner(),
                            };

                            let rel_path = match path.strip_prefix(&self.root_path) {
                                Ok(p) => p.to_path_buf(),
                                Err(_) => path.clone(),
                            };

                            if event.kind == DebouncedEventKind::Any {
                                if path.exists() && path.is_file() {
                                    if let Some(file_node) = scanner.scan_file(&path) {
                                        graph_lock.upsert_file(file_node);
                                        eprintln!(
                                            "\x1b[32m[Real-time Update]\x1b[0m Incremental re-indexed: {}",
                                            rel_path.display()
                                        );
                                    }
                                } else {
                                    graph_lock.remove_file(&rel_path);
                                    eprintln!(
                                        "\x1b[33m[Real-time Update]\x1b[0m Removed file from index: {}",
                                        rel_path.display()
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("Watcher error: {:?}", error);
                    }
                }
            }
        })
    }
}

fn should_ignore_change(path: &Path) -> bool {
    for comp in path.components() {
        let name = comp.as_os_str().to_string_lossy();
        if name == "target"
            || name == ".git"
            || name == "node_modules"
            || name == "build"
            || name.starts_with('.')
        {
            return true;
        }
    }
    false
}
