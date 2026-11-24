//! Micro-change workflow, properties.yaml validation, and Python property tests.

mod support;

use support::{
    change_dir, complete_micro_change, evaluate_gate, expect_allow, expect_block, init_project,
    propose_change, require_node, require_python,
};

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::hooks::paths::ToolInputCarrier;
use forall_workflow::mapping::schema::IssueSeverity;
use forall_workflow::verify::properties::validate_change_properties;
use forall_workflow::verify::report::read_verify_report;
use forall_workflow::workflow::change_class::ChangeClass;
use forall_workflow::workflow::schema::load_workflow_schema;
use forall_workflow::workflow::state::compute_change_status;
use forall_workflow::workflow::status::build_workflow_status;

#[test]
fn micro_change_reaches_apply_ready_without_design_or_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "fix-copy", true);
    complete_micro_change(root, "fix-copy");

    let schema = load_workflow_schema(root).expect("schema");
    let status = compute_change_status(root, &schema, "fix-copy");
    assert_eq!(status.workflow_class, ChangeClass::Micro);
    assert!(status.apply_ready);
    assert!(
        !status
            .artifacts
            .iter()
            .any(|a| a.id == "design" || a.id == "verification")
    );

    let snapshot = build_workflow_status(root, Some("fix-copy"));
    assert_eq!(snapshot.change_class, "micro");
    assert!(snapshot.apply_ready);
}

#[test]
fn properties_yaml_happy_path_has_no_critical_issues() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    std::fs::write(
        change_dir(root, "add-clamp").join("properties.yaml"),
        r#"properties:
  - id: clamp-bounds-prop
    requirement_id: clamp-bounds
    kind: unit
    statement: "lo <= clamp(x,lo,hi) <= hi"
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: false
"#,
    )
    .unwrap();

    let reqs = forall_workflow::mapping::load::requirements_for_check(root, Some("add-clamp"))
        .expect("requirements");
    let issues = validate_change_properties(root, "add-clamp", &reqs);
    assert!(
        !issues.iter().any(|i| i.severity == IssueSeverity::Critical),
        "{issues:?}"
    );
}

#[test]
fn properties_yaml_rejects_unknown_requirement_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    std::fs::write(
        change_dir(root, "add-clamp").join("properties.yaml"),
        r#"properties:
  - id: orphan-prop
    requirement_id: does-not-exist
    kind: unit
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: false
"#,
    )
    .unwrap();

    assert_ne!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        }),
        0
    );

    let report = read_verify_report(root, Some("add-clamp")).expect("report");
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.phase == "properties" && i.message.contains("does-not-exist"))
    );
}

#[test]
fn properties_yaml_rejects_missing_test_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    std::fs::write(
        change_dir(root, "add-clamp").join("properties.yaml"),
        r#"properties:
  - id: clamp-bounds-prop
    requirement_id: clamp-bounds
    kind: unit
    test: .forall/scenarios/missing.property.ts
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: false
"#,
    )
    .unwrap();

    assert_ne!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        }),
        0
    );

    let report = read_verify_report(root, Some("add-clamp")).expect("report");
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.phase == "properties" && i.message.contains("missing.property.ts"))
    );
}

#[test]
fn properties_yaml_accepts_valid_refines_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    std::fs::write(
        change_dir(root, "add-clamp").join("properties.yaml"),
        r#"properties:
  - id: parent-prop
    requirement_id: clamp-bounds
    kind: unit
  - id: child-prop
    requirement_id: clamp-bounds
    kind: unit
    refines: parent-prop
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: false
"#,
    )
    .unwrap();

    let reqs = forall_workflow::mapping::load::requirements_for_check(root, Some("add-clamp"))
        .expect("requirements");
    let issues = validate_change_properties(root, "add-clamp", &reqs);
    assert!(
        !issues.iter().any(|i| i.severity == IssueSeverity::Critical),
        "{issues:?}"
    );
}

#[test]
fn python_property_test_passes_with_structured_runner() {
    require_python();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    std::fs::create_dir_all(change_dir(root, "add-clamp")).unwrap();
    std::fs::create_dir_all(root.join(".forall/scenarios")).unwrap();
    std::fs::write(
        forall_workflow::paths::workflow_active_path(root),
        "add-clamp\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".forall/scenarios/clamp-bounds.property.py"),
        r#"def runPropertyTests():
    import os
    return {
        "ok": True,
        "seed": int(os.environ.get("FORALL_PBT_SEED", "42")),
        "examplesRun": int(os.environ.get("FORALL_PBT_EXAMPLES", "100")),
    }
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: true
    property:
      file: .forall/scenarios/clamp-bounds.property.py
"#,
    )
    .unwrap();

    assert_eq!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 7,
            pbt_examples: 50,
        }),
        0
    );
}

#[test]
fn python_property_failure_surfaces_in_check_report() {
    require_python();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    std::fs::create_dir_all(change_dir(root, "add-clamp")).unwrap();
    std::fs::create_dir_all(root.join(".forall/scenarios")).unwrap();
    std::fs::write(
        forall_workflow::paths::workflow_active_path(root),
        "add-clamp\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".forall/scenarios/clamp-bounds.property.py"),
        r#"def runPropertyTests():
    return {
        "ok": False,
        "seed": 99,
        "examplesRun": 10,
        "message": "PYTHON_PBT_FAILURE_MARKER",
        "counterexample": {"x": 1, "lo": 5, "hi": 2},
    }
"#,
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: true
    property:
      file: .forall/scenarios/clamp-bounds.property.py
"#,
    )
    .unwrap();

    // The same requirement must still run after its delta becomes part of
    // the version-1 project mapping.
    std::fs::copy(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        forall_workflow::paths::forall_mapping_path(root),
    )
    .unwrap();
    for change in [Some("add-clamp"), None] {
        let status = run_check(CheckOptions {
            root: Some(root),
            change,
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 99,
            pbt_examples: 10,
        });
        assert_ne!(status, 0, "expected a failing check in scope {change:?}");

        let report = read_verify_report(root, change).expect("report");
        let issue = report
            .issues
            .iter()
            .find(|i| i.phase == "property-tests" && i.severity == IssueSeverity::Critical)
            .expect("property-tests CRITICAL");
        assert!(issue.message.contains("PYTHON_PBT_FAILURE_MARKER"));
        assert_eq!(issue.pbt.as_ref().map(|p| p.seed), Some(99));
        assert!(issue.counterexample.is_some());
    }
}

#[test]
fn micro_change_blocks_app_code_until_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "fix-copy", true);

    expect_block(evaluate_gate(
        root,
        "apply_patch",
        ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "patch": "*** Add File: src/ui.ts\n+export {}\n"
            })),
            tool_input_snake: None,
        },
    ));
}

#[test]
fn micro_change_allows_app_code_after_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "fix-copy", true);
    complete_micro_change(root, "fix-copy");

    expect_allow(evaluate_gate(
        root,
        "apply_patch",
        ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "patch": "*** Add File: src/ui.ts\n+export const label = 'Save';\n"
            })),
            tool_input_snake: None,
        },
    ));
}

#[test]
fn typescript_property_test_runs_when_node_present() {
    require_node();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    std::fs::create_dir_all(change_dir(root, "add-clamp")).unwrap();
    std::fs::create_dir_all(root.join(".forall/scenarios")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/clamp.ts"),
        "export function clamp(x: number, lo: number, hi: number): number {\n  if (x < lo) return lo;\n  if (x > hi) return hi;\n  return x;\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".forall/scenarios/clamp-bounds.property.ts"),
        include_str!("../schemas/forall/templates/clamp-bounds.property.ts"),
    )
    .unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"private":true,"type":"module","dependencies":{"fast-check":"^3.23.2"}}"#,
    )
    .unwrap();
    assert!(
        std::process::Command::new("npm")
            .args(["install", "--silent"])
            .current_dir(root)
            .status()
            .expect("npm")
            .success()
    );

    std::fs::write(
        forall_workflow::paths::workflow_active_path(root),
        "add-clamp\n",
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: false
    property_tested: true
    property:
      file: .forall/scenarios/clamp-bounds.property.ts
      symbol: clamp
    code:
      file: src/clamp.ts
      symbols: [clamp]
"#,
    )
    .unwrap();

    assert_eq!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        }),
        0
    );
}

/// Runs a property runner script directly and returns its stdout payload.
///
/// Goes through the real script rather than a hand-written payload, because
/// the classification being asserted lives in the script and a fixture would
/// have kept passing while it was wrong.
fn run_property_runner(
    interpreter: &str,
    script: &str,
    property_file: &std::path::Path,
) -> serde_json::Value {
    run_property_runner_with_env(interpreter, script, property_file, &[])
}

fn run_property_runner_with_env(
    interpreter: &str,
    script: &str,
    property_file: &std::path::Path,
    env: &[(&str, &std::path::Path)],
) -> serde_json::Value {
    let runner = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join(script);
    let cwd = property_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut command = std::process::Command::new(interpreter);
    if interpreter == "python3" {
        // The missing-library fixture must be independent of the developer's
        // site packages. Tests may still supply their own fixture PYTHONPATH.
        command.arg("-S").env_remove("PYTHONPATH");
    }
    command.arg(&runner).arg(property_file).current_dir(cwd);
    for (key, value) in env {
        command.env(key, value);
    }
    let out = match command.output() {
        Ok(out) => out,
        Err(err) => panic!("could not run {script} with {interpreter}: {err}"),
    };
    serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "runner stdout was not JSON ({err}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A dependency somebody declared and did not install means the check never
/// happened. A name nobody declared, or a file that is not there, means the
/// author's own property module is broken, and that has to keep failing the
/// check — downgrading it to a warning would let a property that can never
/// run sit under a passing check forever.
#[test]
fn node_separates_an_uninstalled_dependency_from_a_broken_one() {
    require_node();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"proj","type":"module","dependencies":{"the-generator":"^1.0.0"}}"#,
    )
    .unwrap();

    let declared = root.join("declared.property.mjs");
    std::fs::write(
        &declared,
        "import 'the-generator';\nexport default async () => ({ ok: true });\n",
    )
    .unwrap();
    let gap = run_property_runner("node", "run-property.mjs", &declared);
    assert_eq!(gap["ok"], serde_json::json!(false));
    assert_eq!(
        gap["toolUnavailable"],
        serde_json::json!(true),
        "a declared dependency that was never installed is a tooling gap: {gap}"
    );

    // Nobody asked for this package, so it is a name the author got wrong
    // rather than an install that did not happen.
    let undeclared = root.join("undeclared.property.mjs");
    std::fs::write(
        &undeclared,
        "import 'the-generatr';\nexport default async () => ({ ok: true });\n",
    )
    .unwrap();
    let typo = run_property_runner("node", "run-property.mjs", &undeclared);
    assert_eq!(
        typo["toolUnavailable"],
        serde_json::json!(false),
        "a package nobody declared is the author's defect: {typo}"
    );

    // A generator library whose own dependency is missing is still a gap:
    // walking up from the importer reaches that library's manifest first.
    let lib = root.join("node_modules").join("the-generator");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(
        lib.join("package.json"),
        r#"{"name":"the-generator","version":"1.0.0","type":"module","main":"index.js","dependencies":{"its-own-dep":"^1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(lib.join("index.js"), "import 'its-own-dep';\nexport default 1;\n").unwrap();
    let half_installed = run_property_runner("node", "run-property.mjs", &declared);
    assert_eq!(
        half_installed["toolUnavailable"],
        serde_json::json!(true),
        "a half-installed generator library did not falsify anything: {half_installed}"
    );

    let broken_import = root.join("broken-import.property.mjs");
    std::fs::write(
        &broken_import,
        "import './typo-does-not-exist.js';\nexport default async () => ({ ok: true });\n",
    )
    .unwrap();
    let defect = run_property_runner("node", "run-property.mjs", &broken_import);
    assert_eq!(defect["ok"], serde_json::json!(false));
    assert_eq!(
        defect["toolUnavailable"],
        serde_json::json!(false),
        "a relative import that resolves to nothing is the author's defect: {defect}"
    );

    let absent = root.join("never-written.property.mjs");
    let missing_file = run_property_runner("node", "run-property.mjs", &absent);
    assert_eq!(
        missing_file["toolUnavailable"],
        serde_json::json!(false),
        "a property module that is not there did not fail to find a tool: {missing_file}"
    );
}

/// Python reports only a bare module name, so the generator libraries are
/// named explicitly. Treating every `ModuleNotFoundError` as a tooling gap
/// turned a typo in the import of the code under test into a warning.
#[test]
fn python_separates_the_generator_library_from_the_project_s_own_modules() {
    require_python();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let missing_library = root.join("missing-library.property.py");
    std::fs::write(
        &missing_library,
        "import hypothesis\n\n\ndef runPropertyTests():\n    return {'ok': True}\n",
    )
    .unwrap();
    let gap = run_property_runner("python3", "run-property.py", &missing_library);
    assert_eq!(
        gap["toolUnavailable"],
        serde_json::json!(true),
        "Hypothesis missing is a tooling gap: {gap}"
    );

    let broken_import = root.join("broken-import.property.py");
    std::fs::write(
        &broken_import,
        "import module_the_author_typoed\n\n\ndef runPropertyTests():\n    return {'ok': True}\n",
    )
    .unwrap();
    let defect = run_property_runner("python3", "run-property.py", &broken_import);
    assert_eq!(
        defect["toolUnavailable"],
        serde_json::json!(false),
        "a typo in the import of the code under test is the author's defect: {defect}"
    );

    // A library importing one of its own missing dependencies raises with
    // that dependency's name, not its own. Matching the name alone called a
    // half-installed Hypothesis a failed property; the traceback says where
    // the import was really attempted.
    let libs = root.join("libs");
    std::fs::create_dir_all(libs.join("hypothesis")).unwrap();
    std::fs::write(
        libs.join("hypothesis").join("__init__.py"),
        "import a_dependency_of_hypothesis\n",
    )
    .unwrap();
    let half_installed = run_property_runner_with_env(
        "python3",
        "run-property.py",
        &missing_library,
        &[("PYTHONPATH", libs.as_path())],
    );
    assert_eq!(
        half_installed["toolUnavailable"],
        serde_json::json!(true),
        "a half-installed generator library did not falsify anything: {half_installed}"
    );
}
