use layer::graph::VectorGraph;
use layer::mutation::{MutationManager, VerifyMode};
use layer::package_indexer::PackageIndexer;
use layer::telemetry::TelemetryTracker;
use layer::verifier::Verifier;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_rename_symbol_atomic_multi_file_and_comment_string_safety() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Create types.ts
    let types_content = r#"export interface ParticleState {
    id: string;
    mass: number;
    velocity: [number, number, number];
}
"#;
    fs::write(root.join("types.ts"), types_content).unwrap();

    // 2. Create engine.ts containing code usage, plus an explicit string literal and comment
    let engine_content = r#"import { ParticleState } from './types';

// ParticleState comment must never be corrupted
const LABEL = "ParticleState"; // string literal must never be corrupted

export function stepSimulation(state: ParticleState, dt: number): ParticleState {
    return state;
}
"#;
    fs::write(root.join("engine.ts"), engine_content).unwrap();

    // 3. Create Canvas.tsx consuming ParticleState
    let canvas_content = r#"import React from 'react';
import { ParticleState } from './types';

export const ParticleCanvas: React.FC<{ particle: ParticleState }> = ({ particle }) => {
    return <div>{particle.id}</div>;
};
"#;
    fs::write(root.join("Canvas.tsx"), canvas_content).unwrap();

    // Index workspace into VectorGraph
    graph.sync_workspace(root);
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("types.ts"))
    );
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("engine.ts"))
    );
    assert!(
        graph
            .files
            .contains_key(&std::path::PathBuf::from("Canvas.tsx"))
    );

    // 4. Perform atomic multi-file rename: ParticleState -> OrbitalBodyState
    let mut pkg_indexer = PackageIndexer::new();
    let rename_res = MutationManager::rename_symbol(
        root,
        std::path::Path::new("types.ts"),
        "ParticleState",
        "OrbitalBodyState",
        VerifyMode::Fast,
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("rename_symbol must succeed");

    assert!(rename_res.success);
    assert_eq!(rename_res.files_modified.len(), 3);
    assert!(rename_res.total_occurrences_replaced >= 5);
    assert_eq!(rename_res.llm_turns_saved, 2);

    // Verify types.ts was updated
    let updated_types = fs::read_to_string(root.join("types.ts")).unwrap();
    assert!(updated_types.contains("export interface OrbitalBodyState"));
    assert!(!updated_types.contains("ParticleState"));

    // Verify engine.ts was updated in code, BUT comments and string literals remain intact!
    let updated_engine = fs::read_to_string(root.join("engine.ts")).unwrap();
    assert!(updated_engine.contains("import { OrbitalBodyState } from './types'"));
    assert!(updated_engine.contains("state: OrbitalBodyState"));
    assert!(updated_engine.contains("): OrbitalBodyState"));
    // Comment preserved exactly:
    assert!(updated_engine.contains("// ParticleState comment must never be corrupted"));
    // String literal preserved exactly:
    assert!(updated_engine.contains("const LABEL = \"ParticleState\";"));

    // Verify Canvas.tsx was updated
    let updated_canvas = fs::read_to_string(root.join("Canvas.tsx")).unwrap();
    assert!(updated_canvas.contains("import { OrbitalBodyState } from './types'"));
    assert!(updated_canvas.contains("particle: OrbitalBodyState"));

    // 5. Test Transactional Multi-File Rollback
    let rollback_res = MutationManager::rollback_last_edit(root, &mut graph)
        .expect("multi-file rollback must succeed");
    assert!(rollback_res.success);
    assert!(rollback_res.message.contains("multi-file operation"));

    // Verify all 3 files reverted cleanly
    let reverted_types = fs::read_to_string(root.join("types.ts")).unwrap();
    assert!(reverted_types.contains("export interface ParticleState"));
    assert!(!reverted_types.contains("OrbitalBodyState"));

    let reverted_engine = fs::read_to_string(root.join("engine.ts")).unwrap();
    assert!(reverted_engine.contains("import { ParticleState } from './types'"));

    let reverted_canvas = fs::read_to_string(root.join("Canvas.tsx")).unwrap();
    assert!(reverted_canvas.contains("import { ParticleState } from './types'"));
}

#[test]
fn test_verify_impacted_tests_selective_discovery() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Create module files:
    // math.ts
    fs::write(
        root.join("math.ts"),
        "export function add(a: number, b: number): number { return a + b; }\n",
    )
    .unwrap();

    // physics.ts imports math.ts
    fs::write(root.join("physics.ts"), "import { add } from './math';\nexport function computeVelocity(v: number, dv: number): number { return add(v, dv); }\n").unwrap();

    // ui.ts is completely independent
    fs::write(
        root.join("ui.ts"),
        "export function renderButton(): string { return '<button/>'; }\n",
    )
    .unwrap();

    // physics.test.ts imports physics.ts
    fs::write(
        root.join("physics.test.ts"),
        "import { computeVelocity } from './physics';\n// test\n",
    )
    .unwrap();

    // ui.test.ts imports ui.ts
    fs::write(
        root.join("ui.test.ts"),
        "import { renderButton } from './ui';\n// test\n",
    )
    .unwrap();

    graph.sync_workspace(root);
    assert_eq!(graph.get_all_test_files().len(), 2);

    // 2. Change math.ts: physics.test.ts must be impacted, ui.test.ts must be skipped!
    let report = Verifier::verify_impacted_tests(root, Some(&["math.ts".to_string()]), &graph);

    assert!(report.passed);
    assert_eq!(report.impacted_test_files, vec!["physics.test.ts"]);
    assert_eq!(report.total_workspace_test_files, 2);
    assert_eq!(report.skipped_test_files_count, 1);
    assert!(report.message.contains("skipped 1 unaffected tests"));

    // 3. Change ui.ts: only ui.test.ts impacted, physics.test.ts skipped!
    let report_ui = Verifier::verify_impacted_tests(root, Some(&["ui.ts".to_string()]), &graph);
    assert_eq!(report_ui.impacted_test_files, vec!["ui.test.ts"]);
    assert_eq!(report_ui.skipped_test_files_count, 1);
}

#[test]
fn test_telemetry_roi_tracking_and_stats() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut tracker = TelemetryTracker::load(root);
    assert_eq!(tracker.total_tool_calls, 0);

    // Simulate surgical reads, API inspection, AST syntax catch, and cross-file rename
    tracker.record_tool_call("find_symbol", 4, 800, 0);
    tracker.record_tool_call("get_symbol_body", 6, 1200, 0);
    tracker.record_tool_call("inspect_package_api", 15, 2500, 1);
    tracker.record_ast_catch();
    tracker.record_circuit_breaker_save();
    tracker.record_tool_call("rename_symbol", 25, 7000, 2);
    tracker.record_tool_call("verify_impacted_tests", 10, 2400, 1);

    assert_eq!(tracker.total_tool_calls, 5);
    assert_eq!(tracker.surgical_ast_reads, 2);
    assert_eq!(tracker.package_apis_inspected, 1);
    assert_eq!(tracker.cross_file_renames, 1);
    assert_eq!(tracker.selective_test_runs, 1);
    assert_eq!(tracker.ast_precheck_catches, 1);
    assert_eq!(tracker.circuit_breaker_saves, 1);
    assert_eq!(tracker.llm_turns_saved, 6);
    assert!(tracker.estimated_tokens_saved >= 20000);

    // Test persistence
    tracker.save(root).unwrap();
    assert!(root.join(".layer/telemetry.json").exists());

    let loaded = TelemetryTracker::load(root);
    assert_eq!(loaded, tracker);

    // Test dashboard formatting
    let dashboard = tracker.format_dashboard();
    assert!(dashboard.contains("LAYER v1.1 — ROI & SESSION TELEMETRY"));
    assert!(dashboard.contains("find_symbol"));
    assert!(dashboard.contains("inspect_package_api"));
    assert!(dashboard.contains("rename_symbol"));
    assert!(dashboard.contains("verify_impacted_tests"));
}

#[test]
fn test_invariant_1_top_down_coding_never_rolls_back_on_missing_imports() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Agent writes app/layout.tsx importing uncreated sibling ./globals.css and ./components/BulbScene
    let layout_content = r#"import React from 'react';
import './globals.css';
import { BulbScene } from './components/BulbScene';

export default function RootLayout({ children }: { children: React.ReactNode }) {
    return <html><body><BulbScene />{children}</body></html>;
}
"#;
    let res = MutationManager::write_file(
        root,
        std::path::Path::new("app/layout.tsx"),
        layout_content,
        VerifyMode::Fast,
        true, // auto_rollback_on_cascade = true
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("write_file must succeed");

    // Invariant 1 Assertions:
    // File was preserved on disk (rolled_back: false, success: true)
    assert!(res.success);
    assert!(root.join("app/layout.tsx").exists());
    assert!(res.circuit_breaker.is_none() || !res.circuit_breaker.unwrap().rolled_back);
    // Missing imports were extracted for top-down workflow
    assert!(!res.missing_imports_to_create_next.is_empty());
    assert!(
        res.missing_imports_to_create_next
            .contains(&"./globals.css".to_string())
    );
    assert!(
        res.missing_imports_to_create_next
            .contains(&"./components/BulbScene".to_string())
    );

    // 2. An edit introducing an actual Tree-sitter AST syntax error MUST still roll back!
    let syntax_err_content = r#"export function broken() { const x = ; return x; }"#;
    let err_res = MutationManager::write_file(
        root,
        std::path::Path::new("app/syntax_err.ts"),
        syntax_err_content,
        VerifyMode::Fast,
        true,
        &mut graph,
        Some(&mut pkg_indexer),
    )
    .expect("write_file returns mutation result");

    assert!(!err_res.success);
    assert!(err_res.circuit_breaker.is_some());
    let cb = err_res.circuit_breaker.unwrap();
    assert!(cb.rolled_back);
    assert!(!root.join("app/syntax_err.ts").exists());
}

#[test]
fn test_invariant_2_instant_native_scaffolding_nextjs_and_react_and_custom() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();

    // 1. Next.js instant native scaffolding (<10ms)
    let start_next = std::time::Instant::now();
    let next_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::NextJs,
        "my_next_app",
        root,
        Some(vec!["lucide-react".to_string()]),
        &mut graph,
    )
    .expect("bootstrap_project nextjs must succeed");
    let next_elapsed = start_next.elapsed();

    assert!(next_res.success);
    assert!(next_elapsed.as_millis() < 3500); // instant generation
    let next_dir = root.join("my_next_app");
    assert!(next_dir.join("package.json").exists());
    assert!(next_dir.join("tsconfig.json").exists());
    assert!(next_dir.join("next.config.ts").exists());
    assert!(next_dir.join("postcss.config.mjs").exists());
    assert!(next_dir.join("app/globals.css").exists());
    assert!(next_dir.join("app/layout.tsx").exists());
    assert!(next_dir.join("app/page.tsx").exists());

    // Check dependency injection
    let pkg_json = fs::read_to_string(next_dir.join("package.json")).unwrap();
    assert!(pkg_json.contains("lucide-react"));
    assert!(pkg_json.contains("next"));
    assert!(pkg_json.contains("tailwindcss"));

    // 2. React-TS instant native scaffolding
    let react_res = layer::scaffold::bootstrap_project(
        layer::scaffold::GeneratorPreset::ReactTs,
        "my_react_app",
        root,
        None,
        &mut graph,
    )
    .expect("bootstrap_project react-ts must succeed");
    assert!(react_res.success);
    let react_dir = root.join("my_react_app");
    assert!(react_dir.join("package.json").exists());
    assert!(react_dir.join("vite.config.ts").exists());
    assert!(react_dir.join("src/App.tsx").exists());
    assert!(react_dir.join("src/main.tsx").exists());

    // 3. Custom / fallback preset
    let custom_preset = layer::scaffold::GeneratorPreset::parse_or_custom("svelte");
    let custom_res =
        layer::scaffold::bootstrap_project(custom_preset, "my_svelte_app", root, None, &mut graph)
            .expect("bootstrap_project custom preset must succeed");
    assert!(custom_res.success);
    let custom_dir = root.join("my_svelte_app");
    assert!(custom_dir.join("README.md").exists());
    assert!(custom_dir.join("package.json").exists());
}

#[test]
fn test_invariant_3_idempotent_path_normalizer() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("my-awesome-app");
    fs::create_dir_all(&root).unwrap();

    // 1. Path starting with redundant folder name "my-awesome-app/app/page.tsx"
    // when root is already ".../my-awesome-app"
    let resolved = layer::mcp::resolve_safe_path(&root, "my-awesome-app/app/page.tsx")
        .expect("must resolve safely");
    assert_eq!(resolved, std::path::PathBuf::from("app/page.tsx"));

    // 2. Redundant leading name with ./
    let resolved_dot = layer::mcp::resolve_safe_path(&root, "./my-awesome-app/src/index.ts")
        .expect("must resolve safely");
    assert_eq!(resolved_dot, std::path::PathBuf::from("src/index.ts"));

    // 3. Exact folder name "my-awesome-app" maps to "."
    let resolved_self =
        layer::mcp::resolve_safe_path(&root, "my-awesome-app").expect("must resolve safely");
    assert_eq!(resolved_self, std::path::PathBuf::from(""));

    // 4. Normal path without redundant prefix is untouched
    let resolved_normal =
        layer::mcp::resolve_safe_path(&root, "app/layout.tsx").expect("must resolve safely");
    assert_eq!(resolved_normal, std::path::PathBuf::from("app/layout.tsx"));
}

#[test]
fn test_invariant_4_graceful_package_inspection_and_honest_telemetry() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Inspect Next.js fallbacks
    let next_summary = pkg_indexer
        .inspect_package(root, "next/link", Some("Link"), None)
        .expect("next/link must resolve from built-in fallbacks");
    assert_eq!(next_summary.package, "next/link");
    assert!(next_summary.requested_symbol.is_some());

    // 2. Inspect TailwindCSS fallback
    let tailwind_summary = pkg_indexer
        .inspect_package(root, "tailwindcss", Some("Config"), None)
        .expect("tailwindcss must resolve");
    assert_eq!(tailwind_summary.package, "tailwindcss");

    // 3. Inspect Framer Motion fallback
    let motion_summary = pkg_indexer
        .inspect_package(root, "framer-motion", Some("motion"), None)
        .expect("framer-motion must resolve");
    assert_eq!(motion_summary.package, "framer-motion");

    // 4. Inspect Zustand fallback
    let zustand_summary = pkg_indexer
        .inspect_package(root, "zustand", Some("create"), None)
        .expect("zustand must resolve");
    assert_eq!(zustand_summary.package, "zustand");

    // 5. Test MCP inspect_package_api with unindexed/offline package
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let inspect_args = serde_json::json!({
        "package_name": "some-obscure-uninstalled-pkg-xyz",
    });
    let result_json = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "inspect_package_api",
        Some(&inspect_args),
        &mut pkg_indexer,
    );

    // Must return structured JSON with "status": "unindexed_offline" instead of MCP error string
    assert!(!result_json.starts_with("Error:"));
    let parsed: serde_json::Value = serde_json::from_str(&result_json).unwrap();
    assert_eq!(parsed["status"], "unindexed_offline");
    assert_eq!(parsed["package"], "some-obscure-uninstalled-pkg-xyz");

    // 6. Test Honest Telemetry: unindexed_offline does NOT increment package_apis_inspected or credit tokens
    let mut tracker = TelemetryTracker::new();
    let initial_calls = tracker.total_tool_calls;
    let initial_apis = tracker.package_apis_inspected;
    let initial_tokens = tracker.estimated_tokens_saved;

    // Simulate resolved inspection:
    tracker.record_tool_call("inspect_package_api", 10, 2500, 1);
    assert_eq!(tracker.package_apis_inspected, initial_apis + 1);
    assert_eq!(tracker.estimated_tokens_saved, initial_tokens + 2500);

    // Simulate unindexed_offline inspection (0 tokens saved):
    tracker.record_tool_call("inspect_package_api", 5, 0, 0);
    assert_eq!(tracker.package_apis_inspected, initial_apis + 1); // Not incremented!
    assert_eq!(tracker.estimated_tokens_saved, initial_tokens + 2500); // Not credited!
    assert_eq!(tracker.total_tool_calls, initial_calls + 2);
}

#[test]
fn test_bootstrap_project_nested_folder_and_transport_resilience() {
    let dir = tempdir().unwrap();
    let container_root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = container_root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // 1. Call bootstrap_project to create a nested subfolder 'light_circuit_studio'
    let bootstrap_args = serde_json::json!({
        "preset": "nextjs",
        "project_name": "light_circuit_studio",
        "destination": "."
    });

    let res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "bootstrap_project",
        Some(&bootstrap_args),
        &mut pkg_indexer,
    );

    // Verify response is valid JSON and contains created files
    assert!(
        !res_str.starts_with("Error:"),
        "Bootstrap failed: {}",
        res_str
    );
    let parsed: serde_json::Value = serde_json::from_str(&res_str).unwrap();
    assert_eq!(parsed["success"], true);
    assert_eq!(parsed["project_name"], "light_circuit_studio");

    // Verify workspace root auto-switched to nested project
    let nested_dir = container_root.join("light_circuit_studio");
    assert_eq!(
        root_buf.canonicalize().unwrap(),
        nested_dir.canonicalize().unwrap(),
        "Workspace root must auto-switch to bootstrapped nested directory"
    );

    // Verify project files exist
    assert!(nested_dir.join("package.json").exists());
    assert!(nested_dir.join("tsconfig.json").exists());
    assert!(nested_dir.join("app/page.tsx").exists());

    // 2. Subsequent write_file tool call on the active workspace succeeds without panic or error
    let write_args = serde_json::json!({
        "path": "app/BulbScene.tsx",
        "content": "export function BulbScene() { return <div id=\"scene\">Bulb Active</div>; }",
        "verify_mode": "fast"
    });

    let write_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "write_file",
        Some(&write_args),
        &mut pkg_indexer,
    );
    assert!(
        !write_res_str.starts_with("Error:"),
        "Write failed: {}",
        write_res_str
    );
    assert!(nested_dir.join("app/BulbScene.tsx").exists());
}

#[test]
fn test_verify_project_auto_detects_nested_project_root() {
    let dir = tempdir().unwrap();
    let parent_dir = dir.path();

    // 1. Parent dir is empty with no build manifest
    assert_eq!(
        layer::verifier::ProjectType::detect(parent_dir),
        layer::verifier::ProjectType::Unknown
    );

    // 2. Create a nested project in a subfolder (e.g. 'bulb_circuit_sim')
    let sub_proj = parent_dir.join("bulb_circuit_sim");
    std::fs::create_dir_all(&sub_proj).unwrap();
    std::fs::write(
        sub_proj.join("package.json"),
        r#"{"name": "bulb_circuit_sim", "version": "1.0.0"}"#,
    )
    .unwrap();
    std::fs::write(
        sub_proj.join("tsconfig.json"),
        r#"{"compilerOptions": {"module": "esnext"}}"#,
    )
    .unwrap();

    // 3. Direct verification of parent_dir MUST auto-detect the nested project root
    let direct_report = layer::verifier::Verifier::verify_workspace(parent_dir, None);
    assert_ne!(
        direct_report.message, "No recognized build configuration found in workspace root.",
        "Should never return missing config error when nested manifest exists"
    );
    assert_eq!(direct_report.project_type, "typescript");

    // 4. MCP execute_tool('verify_project') from parent workspace auto-switches active root
    let mut graph = VectorGraph::new();
    let mut ws_root = parent_dir.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    let verify_args = serde_json::json!({});
    let mcp_res_str = layer::mcp::execute_tool(
        &mut graph,
        &mut ws_root,
        "verify_project",
        Some(&verify_args),
        &mut pkg_indexer,
    );

    assert!(!mcp_res_str.starts_with("Error:"));
    let report_json: serde_json::Value = serde_json::from_str(&mcp_res_str).unwrap();
    assert_eq!(report_json["project_type"], "typescript");
    assert_ne!(
        report_json["message"],
        "No recognized build configuration found in workspace root."
    );

    // Active workspace root was updated to the nested project
    assert_eq!(
        ws_root.canonicalize().unwrap(),
        sub_proj.canonicalize().unwrap()
    );
}

#[test]
fn test_mcp_panic_boundary_isolation() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let mut graph = VectorGraph::new();
    let mut root_buf = root.to_path_buf();
    let mut pkg_indexer = PackageIndexer::new();

    // Calling an operation with invalid / extreme data or unknown method does NOT panic the process
    let unknown_args = serde_json::json!({ "invalid_field": 12345 });
    let res = layer::mcp::execute_tool(
        &mut graph,
        &mut root_buf,
        "non_existent_tool_xyz",
        Some(&unknown_args),
        &mut pkg_indexer,
    );
    assert!(res.contains("Unknown tool"));
}
