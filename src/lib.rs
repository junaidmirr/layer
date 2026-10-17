#![recursion_limit = "512"]
#![allow(unknown_lints)]
#![allow(clippy::manual_pop_if)]

pub mod context_compiler;
pub mod context_engine;
pub mod embedder;
pub mod graph;
pub mod harness;
pub mod history;
pub mod mcp;
pub mod models;
pub mod mutation;
pub mod package_indexer;
pub mod permission;
pub mod scaffold;
pub mod scanner;
pub mod telemetry;
pub mod universal_parser;
pub mod verifier;
pub mod walker;
pub mod watcher;
