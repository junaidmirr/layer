use layer::graph::VectorGraph;
use layer::models::SymbolKind;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use tempfile::tempdir;

#[test]
fn test_create_file_and_immediately_query_symbols() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // Initially workspace is empty
    graph.sync_workspace(root);
    assert!(graph.files.is_empty());

    // Create src/calculator.rs
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let calc_file = src_dir.join("calculator.rs");
    fs::write(
        &calc_file,
        "pub struct Calculator;\n\nimpl Calculator {\n    pub fn add(a: i32, b: i32) -> i32 { a + b }\n}\n",
    )
    .unwrap();

    // Synchronize and query immediately
    graph.sync_workspace(root);
    let rel_path = PathBuf::from("src/calculator.rs");
    assert!(graph.files.contains_key(&rel_path));

    let symbols = graph
        .get_file_symbols(&rel_path)
        .expect("symbols must exist");
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0].name, "Calculator");
    assert_eq!(symbols[0].kind, SymbolKind::Struct);
    assert_eq!(symbols[1].name, "add");
    assert_eq!(symbols[1].kind, SymbolKind::Method);
    assert!(
        symbols[1]
            .signature
            .contains("pub fn add(a: i32, b: i32) -> i32")
    );
}

#[test]
fn test_modify_file_and_retrieve_updated_signature() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let user_file = src_dir.join("user.rs");

    // Version 1
    fs::write(
        &user_file,
        "pub fn get_user(id: u64) -> String {\n    format!(\"user_{}\", id)\n}\n",
    )
    .unwrap();
    graph.sync_workspace(root);

    let rel_path = PathBuf::from("src/user.rs");
    let syms_v1 = graph.get_file_symbols(&rel_path).unwrap();
    assert_eq!(syms_v1.len(), 1);
    assert_eq!(syms_v1[0].name, "get_user");
    assert!(
        syms_v1[0]
            .signature
            .starts_with("pub fn get_user(id: u64) -> String")
    );

    // Version 2: Update signature with Result and extra parameter
    // Add brief sleep to guarantee mtime ticks on coarse filesystems if needed
    thread::sleep(std::time::Duration::from_millis(15));
    fs::write(
        &user_file,
        "pub fn get_user(id: u64, include_profile: bool) -> Result<String, ()> {\n    Ok(format!(\"user_{}\", id))\n}\n",
    )
    .unwrap();
    graph.sync_workspace(root);

    let syms_v2 = graph.get_file_symbols(&rel_path).unwrap();
    assert_eq!(syms_v2.len(), 1);
    assert_eq!(syms_v2[0].name, "get_user");
    assert!(
        syms_v2[0]
            .signature
            .starts_with("pub fn get_user(id: u64, include_profile: bool) -> Result<String, ()>")
    );
}

#[test]
fn test_rename_file_and_verify_obsolete_paths_and_edges_disappear() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let old_file = src_dir.join("old_auth.rs");
    let new_file = src_dir.join("new_auth.rs");
    let main_file = src_dir.join("main.rs");

    fs::write(&old_file, "pub fn authenticate() -> bool { true }\n").unwrap();
    fs::write(
        &main_file,
        "mod old_auth;\nuse old_auth::authenticate;\nfn main() { authenticate(); }\n",
    )
    .unwrap();

    graph.sync_workspace(root);
    let old_rel = PathBuf::from("src/old_auth.rs");
    let main_rel = PathBuf::from("src/main.rs");
    assert!(graph.files.contains_key(&old_rel));
    assert!(graph.files.contains_key(&main_rel));

    // Verify edge exists from main to old_auth
    let main_idx = graph.node_indices[&main_rel];
    let old_idx = graph.node_indices[&old_rel];
    assert!(graph.graph.contains_edge(main_idx, old_idx));

    // Rename file on disk and update main.rs
    fs::rename(&old_file, &new_file).unwrap();
    fs::write(
        &main_file,
        "mod new_auth;\nuse new_auth::authenticate;\nfn main() { authenticate(); }\n",
    )
    .unwrap();

    graph.sync_workspace(root);

    let new_rel = PathBuf::from("src/new_auth.rs");
    // Obsolete path must no longer be in files
    assert!(!graph.files.contains_key(&old_rel));
    // New path must exist
    assert!(graph.files.contains_key(&new_rel));
    // Obsolete node index must not exist
    assert!(!graph.node_indices.contains_key(&old_rel));

    // Edge must now point from main to new_auth, old edge must be gone
    let main_idx_new = graph.node_indices[&main_rel];
    let new_idx = graph.node_indices[&new_rel];
    assert!(graph.graph.contains_edge(main_idx_new, new_idx));
}

#[test]
fn test_delete_file_and_verify_no_stale_symbols_or_dependencies_remain() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let helper_file = src_dir.join("helper.rs");
    let main_file = src_dir.join("main.rs");

    fs::write(&helper_file, "pub fn helper_calc() -> i32 { 42 }\n").unwrap();
    fs::write(
        &main_file,
        "mod helper;\nfn main() { helper::helper_calc(); }\n",
    )
    .unwrap();

    graph.sync_workspace(root);
    let helper_rel = PathBuf::from("src/helper.rs");
    assert!(graph.files.contains_key(&helper_rel));
    assert!(
        !graph
            .find_symbol_filtered("helper_calc", None, None)
            .is_empty()
    );

    // Delete helper.rs and update main.rs
    fs::remove_file(&helper_file).unwrap();
    fs::write(&main_file, "fn main() {}\n").unwrap();

    graph.sync_workspace(root);

    // Verify helper is completely gone from files, symbol index, and graph
    assert!(!graph.files.contains_key(&helper_rel));
    assert!(!graph.node_indices.contains_key(&helper_rel));
    assert!(
        graph
            .find_symbol_filtered("helper_calc", None, None)
            .is_empty()
    );
    assert!(graph.get_file_symbols(&helper_rel).is_none());
}

#[test]
fn test_rapid_sequential_writes_across_dependent_modules() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // Module A: types
    let types_file = src_dir.join("types.rs");
    fs::write(
        &types_file,
        "pub struct OrderId(pub u64);\npub struct Order { pub id: OrderId }\n",
    )
    .unwrap();
    graph.sync_workspace(root);
    assert_eq!(graph.files.len(), 1);

    // Module B: repo (depends on types)
    let repo_file = src_dir.join("repo.rs");
    fs::write(
        &repo_file,
        "use crate::types::{Order, OrderId};\npub trait OrderRepo {\n    fn find(&self, id: OrderId) -> Option<Order>;\n}\n",
    )
    .unwrap();
    graph.sync_workspace(root);
    assert_eq!(graph.files.len(), 2);

    // Module C: service (depends on repo and types)
    let service_file = src_dir.join("service.rs");
    fs::write(
        &service_file,
        "use crate::types::OrderId;\nuse crate::repo::OrderRepo;\npub struct OrderService<R: OrderRepo> { repo: R }\n",
    )
    .unwrap();
    graph.sync_workspace(root);
    assert_eq!(graph.files.len(), 3);

    // Verify all symbols and files resolve cleanly across all 3 modules
    let rel_types = PathBuf::from("src/types.rs");
    let rel_repo = PathBuf::from("src/repo.rs");
    let rel_service = PathBuf::from("src/service.rs");

    assert!(graph.files.contains_key(&rel_types));
    assert!(graph.files.contains_key(&rel_repo));
    assert!(graph.files.contains_key(&rel_service));

    let repo_syms = graph.get_file_symbols(&rel_repo).unwrap();
    assert_eq!(repo_syms[0].name, "OrderRepo");
    assert_eq!(repo_syms[0].kind, SymbolKind::Trait);

    let service_syms = graph.get_file_symbols(&rel_service).unwrap();
    assert_eq!(service_syms[0].name, "OrderService");
    assert_eq!(service_syms[0].kind, SymbolKind::Struct);
}

#[test]
fn test_parse_failure_and_recovery_after_valid_write() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path();
    let mut graph = VectorGraph::new();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let draft_file = src_dir.join("draft.rs");

    // Write incomplete/broken syntax during agent drafting
    fs::write(
        &draft_file,
        "pub fn incomplete_func(a: i32, \n// missing closing syntax",
    )
    .unwrap();
    graph.sync_workspace(root);

    let rel_path = PathBuf::from("src/draft.rs");
    // File is indexed without panic
    assert!(graph.files.contains_key(&rel_path));

    // Agent finishes write with valid syntax
    thread::sleep(std::time::Duration::from_millis(15));
    fs::write(
        &draft_file,
        "pub fn incomplete_func(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    graph.sync_workspace(root);

    let syms = graph.get_file_symbols(&rel_path).unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0].name, "incomplete_func");
    assert!(
        syms[0]
            .signature
            .starts_with("pub fn incomplete_func(a: i32, b: i32) -> i32")
    );
}

#[test]
fn test_concurrent_access_during_synchronization() {
    let dir = tempdir().expect("failed to create tempdir");
    let root = dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let graph_arc = Arc::new(Mutex::new(VectorGraph::new()));

    // Spawn 4 reader threads querying symbols while writer thread creates files
    let mut handles = Vec::new();

    let g_writer = Arc::clone(&graph_arc);
    let r_writer = root.clone();
    let s_writer = src_dir.clone();
    handles.push(thread::spawn(move || {
        for i in 0..10 {
            let f = s_writer.join(format!("mod_{}.rs", i));
            fs::write(&f, format!("pub fn worker_fn_{}() -> i32 {{ {} }}\n", i, i)).unwrap();
            let mut g = g_writer.lock().unwrap();
            g.sync_workspace(&r_writer);
        }
    }));

    for _ in 0..3 {
        let g_reader = Arc::clone(&graph_arc);
        handles.push(thread::spawn(move || {
            for _ in 0..20 {
                let g = g_reader.lock().unwrap();
                let _ = g.find_symbol_filtered("worker_fn_5", None, None);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Final check
    let g = graph_arc.lock().unwrap();
    assert_eq!(g.files.len(), 10);
    let matches = g.find_symbol_filtered("worker_fn_5", None, None);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].1.name, "worker_fn_5");
}
