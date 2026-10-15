use layer::verifier::{DiagnosticItem, Verifier};
use std::fs;

#[test]
fn test_real_cargo_check_diagnostics_and_resolution_cycle() {
    let temp_dir = std::env::temp_dir().join(format!("layer_verify_rust_{}", std::process::id()));
    let src_dir = temp_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let cargo_toml = r#"[package]
name = "fixture_test"
version = "0.1.0"
edition = "2024"
"#;
    fs::write(temp_dir.join("Cargo.toml"), cargo_toml).unwrap();

    // 1. Write file with intentional type error
    let broken_code = r#"fn main() {
    let x: u32 = "invalid string";
}
"#;
    fs::write(src_dir.join("main.rs"), broken_code).unwrap();

    // First verification run
    let report1 = Verifier::verify_workspace(&temp_dir, None);
    assert!(!report1.success, "Compilation must fail on type error");
    assert!(report1.error_count >= 1);
    assert_eq!(report1.project_type, "rust");

    let primary_err = report1
        .diagnostics
        .iter()
        .find(|d| d.file.contains("main.rs") && d.code.as_deref() == Some("E0308"))
        .expect("Must find E0308 mismatched types");
    assert_eq!(primary_err.line, 2);

    let comp1 = report1.comparison.expect("Comparison must be present");
    assert!(!comp1.new_errors.is_empty());
    assert!(comp1.resolved_errors.is_empty());
    assert!(comp1.persistent_errors.is_empty());

    // 2. Second verification without changing the broken file: error must be PERSISTENT
    let report2 = Verifier::verify_workspace(&temp_dir, Some(&report1.diagnostics));
    assert!(!report2.success);
    let comp2 = report2.comparison.expect("Comparison must be present");
    assert!(
        comp2
            .persistent_errors
            .iter()
            .any(|e| e.code.as_deref() == Some("E0308")),
        "E0308 must be marked as persistent error"
    );
    assert!(
        comp2.new_errors.is_empty(),
        "No new errors should be detected"
    );
    assert!(
        comp2.resolved_errors.is_empty(),
        "No resolved errors should be detected"
    );

    // 3. Fix the error
    let fixed_code = r#"fn main() {
    let x: u32 = 42;
    println!("{}", x);
}
"#;
    fs::write(src_dir.join("main.rs"), fixed_code).unwrap();

    // Third verification run: error must be RESOLVED and build must succeed
    let report3 = Verifier::verify_workspace(&temp_dir, Some(&report2.diagnostics));
    assert!(report3.success, "Build should succeed after fix");
    assert_eq!(report3.error_count, 0);

    let comp3 = report3.comparison.expect("Comparison must be present");
    assert!(comp3.new_errors.is_empty(), "No new errors on fixed build");
    assert!(
        comp3.persistent_errors.is_empty(),
        "No persistent errors on fixed build"
    );
    assert!(
        comp3
            .resolved_errors
            .iter()
            .any(|e| e.code.as_deref() == Some("E0308")),
        "E0308 must be reported in resolved_errors"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_newly_introduced_and_persisting_typescript_diagnostics() {
    let initial_prev = vec![DiagnosticItem::new(
        "src/physics/engine.ts",
        12,
        5,
        "error",
        Some("TS2322".to_string()),
        "Type 'string' is not assignable to type 'number'.",
    )];

    // Agent attempts fix on engine.ts (fails to fix TS2322) and edits Canvas.tsx (introduces TS2304)
    let current_diags = vec![
        DiagnosticItem::new(
            "src/physics/engine.ts",
            14, // line shifted
            5,
            "error",
            Some("TS2322".to_string()),
            "Type 'string' is not assignable to type 'number'.",
        ),
        DiagnosticItem::new(
            "src/components/Canvas.tsx",
            50,
            9,
            "error",
            Some("TS2304".to_string()),
            "Cannot find name 'WebGLRenderingContext'.",
        ),
    ];

    let comp = Verifier::compare_diagnostics(Some(&initial_prev), &current_diags);

    // 1. TS2304 is newly introduced
    assert_eq!(comp.new_errors.len(), 1);
    assert_eq!(comp.new_errors[0].code.as_deref(), Some("TS2304"));
    assert_eq!(comp.new_errors[0].file, "src/components/Canvas.tsx");

    // 2. TS2322 persisted
    assert_eq!(comp.persistent_errors.len(), 1);
    assert_eq!(comp.persistent_errors[0].code.as_deref(), Some("TS2322"));
    assert_eq!(comp.persistent_errors[0].file, "src/physics/engine.ts");

    // 3. None were resolved
    assert!(comp.resolved_errors.is_empty());
}
