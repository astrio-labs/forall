//! Brownfield mapping discover from exported TypeScript/TSX symbols.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;

use forall_workflow::commands::mapping_discover::{
    MappingDiscoverCommandOptions, run_mapping_discover_command,
};
use forall_workflow::commands::workflow::run_init;
use forall_workflow::mapping::discover::discover_exported_symbols;
use forall_workflow::mapping::load::load_project_mapping;

fn write_tsx_project(root: &Path) {
    run_init(root, false, false, false);
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("src/utils")).unwrap();
    fs::write(
        root.join("src/components/Hero.tsx"),
        r#"export function Hero() {
  return <section>Namaste</section>;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/utils/format.ts"),
        "export function formatName(name: string): string {\n  return name.trim();\n}\n",
    )
    .unwrap();
}

#[test]
fn discover_exported_symbols_finds_tsx_exports() {
    let source = r#"export function Hero() { return null; }
export const Footer = () => null;
"#;
    let names = discover_exported_symbols(source);
    assert!(names.contains(&"Hero".to_string()));
    assert!(names.contains(&"Footer".to_string()));
}

#[test]
fn discover_adds_spec_tracked_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tsx_project(root);

    assert_eq!(
        run_mapping_discover_command(MappingDiscoverCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );

    let mapping = load_project_mapping(root).expect("mapping");
    let hero = mapping
        .requirements
        .iter()
        .find(|req| {
            req.code.as_ref().is_some_and(|code| {
                code.file == "src/components/Hero.tsx" && code.symbols == ["Hero"]
            })
        })
        .expect("Hero requirement");
    assert!(!hero.verified);
    assert!(hero.requirement.contains("Hero"));

    let format = mapping
        .requirements
        .iter()
        .find(|req| {
            req.code.as_ref().is_some_and(|code| {
                code.file == "src/utils/format.ts" && code.symbols == ["formatName"]
            })
        })
        .expect("formatName requirement");
    assert!(!format.verified);
}

#[test]
fn discover_dry_run_does_not_modify_mapping_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tsx_project(root);
    let mapping_path = forall_workflow::paths::forall_mapping_path(root);
    let before = fs::read_to_string(&mapping_path).unwrap();

    assert_eq!(
        run_mapping_discover_command(MappingDiscoverCommandOptions {
            root: Some(root),
            dry_run: true,
            json: false,
        }),
        0
    );

    let after = fs::read_to_string(&mapping_path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn second_discover_skips_existing_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_tsx_project(root);

    assert_eq!(
        run_mapping_discover_command(MappingDiscoverCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );
    let first_count = load_project_mapping(root).unwrap().requirements.len();

    assert_eq!(
        run_mapping_discover_command(MappingDiscoverCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );
    let second_count = load_project_mapping(root).unwrap().requirements.len();
    assert_eq!(first_count, second_count);
}
