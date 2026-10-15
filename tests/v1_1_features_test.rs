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
