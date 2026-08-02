use std::fs;

use tempfile::TempDir;

use super::*;
use crate::mapping::schema::CodeRef;

fn root() -> (TempDir, CanonicalRoot) {
    let dir = tempfile::tempdir().unwrap();
    let root = CanonicalRoot::new(dir.path()).unwrap();
    (dir, root)
}

fn write(root: &CanonicalRoot, path: &str, content: &str) {
    let target = root.as_path().join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
}

fn apply() -> MutationOptions {
    MutationOptions {
        mode: MutationMode::Apply,
        expected_sha256: BTreeMap::new(),
    }
}

fn apply_for(root: &CanonicalRoot, paths: &[&str]) -> MutationOptions {
    let expected_sha256 = paths
        .iter()
        .map(|path| {
            (
                (*path).to_string(),
                sha256(&fs::read(root.as_path().join(path)).unwrap()),
            )
        })
        .collect();
    MutationOptions {
        mode: MutationMode::Apply,
        expected_sha256,
    }
}

fn requirement(id: &str) -> Requirement {
    Requirement {
        id: id.to_string(),
        capability: "math".to_string(),
        requirement: "caller supplied requirement".to_string(),
        verified: false,
        property_tested: false,
        property: None,
        code: None,
        contract: None,
        claimcheck: None,
        scenarios: None,
    }
}

#[test]
fn initializes_preview_apply_and_status_idempotently() {
    let (_dir, root) = root();
    let preview = init_project(&root, &MutationOptions::default()).unwrap();
    assert_eq!(
        preview.created,
        vec![
            MAPPING_PATH,
            ".forall/workflow/config.yaml",
            ".forall/AGENTS.md"
        ]
    );
    assert!(!root.as_path().join(MAPPING_PATH).exists());

    let first = init_project(&root, &apply()).unwrap();
    assert_eq!(
        first.created,
        vec![
            MAPPING_PATH,
            ".forall/workflow/config.yaml",
            ".forall/AGENTS.md"
        ]
    );
    let second = init_project(&root, &apply()).unwrap();
    assert_eq!(
        second.unchanged,
        vec![
            MAPPING_PATH,
            ".forall/workflow/config.yaml",
            ".forall/AGENTS.md"
        ]
    );
    let status = project_status(&root).unwrap();
    assert!(status.initialized);
    assert_eq!(status.requirement_count, 0);
    assert!(status.mapping_sha256.is_some());
}

#[test]
fn init_never_overwrites_existing_project_files() {
    let (_dir, root) = root();
    write(
        &root,
        MAPPING_PATH,
        "version: 1\nrequirements:\n  - id: existing\n    capability: core\n    requirement: preserve me\n",
    );
    write(
        &root,
        ".forall/workflow/config.yaml",
        "schema: custom\ncontext: preserve me\n",
    );
    let mapping_before = fs::read(root.as_path().join(MAPPING_PATH)).unwrap();
    let config_before = fs::read(root.as_path().join(".forall/workflow/config.yaml")).unwrap();

    let output = init_project(&root, &apply()).unwrap();
    assert_eq!(
        output.unchanged,
        vec![MAPPING_PATH, ".forall/workflow/config.yaml"]
    );
    assert_eq!(
        fs::read(root.as_path().join(MAPPING_PATH)).unwrap(),
        mapping_before
    );
    assert_eq!(
        fs::read(root.as_path().join(".forall/workflow/config.yaml")).unwrap(),
        config_before
    );
}

#[test]
fn discovers_three_languages_and_overloads() {
    let (_dir, root) = root();
    write(
        &root,
        "src/math.tsx",
        "export function clamp(x: number) {\n  return x;\n}\nexport const double = (x: number) => { return x * 2; };\n",
    );
    write(
        &root,
        "src/math.rs",
        "pub fn clamp(x: i32) -> i32 {\n    x\n}\nfn hidden() {}\n",
    );
    write(
        &root,
        "src/Math.java",
        "class Math {\n  public int clamp(int x) { return x; }\n  public int clamp(int x, int y) { return x; }\n  private int hidden() { return 0; }\n}\n",
    );
    let found = discover_symbols(
        &root,
        &[
            "src/math.tsx".into(),
            "src/math.rs".into(),
            "src/Math.java".into(),
        ],
    )
    .unwrap();
    assert_eq!(found.files.len(), 3);
    assert_eq!(found.files[0].path, "src/Math.java");
    assert_eq!(found.files[0].sha256.len(), 64);
    assert_eq!(found.symbols.len(), 5);
    assert_eq!(found.symbols[0].symbol, "clamp(int x)");
    assert_eq!(found.symbols[1].symbol, "clamp(int x, int y)");
    assert_eq!(found.symbols[2].symbol, "clamp");
    assert_eq!(found.symbols[3].symbol, "clamp");
    assert_eq!(found.symbols[4].symbol, "double");
}

#[test]
fn golden_contract_edits_for_three_languages() {
    let (_dir, root) = root();
    write(
        &root,
        "src/math.ts",
        "export function clamp(x: number) {\n  return x;\n}\n",
    );
    write(
        &root,
        "src/math.rs",
        "pub fn clamp(x: i32) -> i32 {\n    x\n}\n",
    );
    write(
        &root,
        "src/Math.java",
        "class Math {\n  public int clamp(int x) { return x; }\n}\n",
    );
    let mutation = apply_for(&root, &["src/math.ts", "src/math.rs", "src/Math.java"]);
    scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![
                ContractTarget {
                    file: "src/math.ts".into(),
                    symbol: "clamp".into(),
                    requires: vec!["Number.isFinite(x)".into()],
                    ensures: vec!["result === x".into()],
                },
                ContractTarget {
                    file: "src/math.rs".into(),
                    symbol: "clamp".into(),
                    requires: vec!["x >= 0".into()],
                    ensures: vec!["result == x".into()],
                },
                ContractTarget {
                    file: "src/Math.java".into(),
                    symbol: "clamp(int x)".into(),
                    requires: vec!["x >= 0;".into()],
                    ensures: vec![r"\result == x;".into()],
                },
            ],
            mutation,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/math.ts")).unwrap(),
        "export function clamp(x: number) {\n  //@ requires Number.isFinite(x)\n  //@ ensures result === x\n  return x;\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/math.rs")).unwrap(),
        "pub fn clamp(x: i32) -> i32 \n    requires x >= 0,\n    ensures result == x,\n{\n    x\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/Math.java")).unwrap(),
        "class Math {\n  //@ requires x >= 0;\n  //@ ensures \\result == x;\n  public int clamp(int x) { return x; }\n}\n"
    );
}

#[test]
fn contracts_open_the_body_past_braces_in_signatures() {
    let (_dir, root) = root();
    // Every signature here carries a `{` before the one that opens the body:
    // an object parameter, an object return type, and a generic bound.
    write(
        &root,
        "src/pricing.ts",
        "export function total(order: { net: number }, rate: number): number {\n  return order.net * rate;\n}\n",
    );
    write(
        &root,
        "src/shape.ts",
        "export function shape(x: number): { ok: boolean } {\n  return { ok: x > 0 };\n}\n",
    );
    write(
        &root,
        "src/apply.rs",
        "pub fn apply<F: Fn(u32) -> u32>(f: F, x: u32) -> u32 {\n    f(x)\n}\n",
    );
    let mutation = apply_for(&root, &["src/pricing.ts", "src/shape.ts", "src/apply.rs"]);
    scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![
                ContractTarget {
                    file: "src/pricing.ts".into(),
                    symbol: "total".into(),
                    requires: vec!["rate > 0".into()],
                    ensures: vec![],
                },
                ContractTarget {
                    file: "src/shape.ts".into(),
                    symbol: "shape".into(),
                    requires: vec!["x !== 0".into()],
                    ensures: vec![],
                },
                ContractTarget {
                    file: "src/apply.rs".into(),
                    symbol: "apply".into(),
                    requires: vec!["x > 0".into()],
                    ensures: vec![],
                },
            ],
            mutation,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/pricing.ts")).unwrap(),
        "export function total(order: { net: number }, rate: number): number {\n  //@ requires rate > 0\n  return order.net * rate;\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/shape.ts")).unwrap(),
        "export function shape(x: number): { ok: boolean } {\n  //@ requires x !== 0\n  return { ok: x > 0 };\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/apply.rs")).unwrap(),
        "pub fn apply<F: Fn(u32) -> u32>(f: F, x: u32) -> u32 \n    requires x > 0,\n{\n    f(x)\n}\n"
    );
}

#[test]
fn discovered_signatures_keep_nested_parentheses() {
    let (_dir, root) = root();
    write(
        &root,
        "src/lib.rs",
        "pub fn label(map: Vec<u32>, cb: fn(u32) -> u32) -> u32 {\n    0\n}\n",
    );
    let discovered = discover_symbols(&root, &["src/lib.rs".to_string()]).unwrap();
    assert_eq!(
        discovered.symbols[0].signature,
        "label(map: Vec<u32>, cb: fn(u32) -> u32)"
    );
}

#[test]
fn contracts_reach_bodies_that_do_not_balance() {
    let (_dir, root) = root();
    // The `{` inside this regex literal never closes, so the body cannot be
    // balanced. The insertion point is still known and must still be used.
    write(
        &root,
        "src/brace.ts",
        "export function hasBrace(s: string): boolean {\n  return /{/.test(s);\n}\n",
    );
    let mutation = apply_for(&root, &["src/brace.ts"]);
    scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![ContractTarget {
                file: "src/brace.ts".into(),
                symbol: "hasBrace".into(),
                requires: vec!["s.length > 0".into()],
                ensures: vec![],
            }],
            mutation,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/brace.ts")).unwrap(),
        "export function hasBrace(s: string): boolean {\n  //@ requires s.length > 0\n  return /{/.test(s);\n}\n"
    );
}

#[test]
fn discovery_handles_deeply_nested_generic_bounds() {
    let (_dir, root) = root();
    write(
        &root,
        "src/parse.rs",
        "pub fn parse<T: IntoIterator<Item = Vec<u8>>>(input: T) -> usize {\n    0\n}\n",
    );
    let discovered = discover_symbols(&root, &["src/parse.rs".to_string()]).unwrap();
    assert_eq!(
        discovered
            .symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["parse"]
    );
}

#[test]
fn discovery_finds_generic_and_annotated_arrow_functions() {
    let (_dir, root) = root();
    write(
        &root,
        "src/pick.ts",
        "export function pick<T extends { id: string }>(v: T): string {\n  return v.id;\n}\n",
    );
    write(
        &root,
        "src/scale.ts",
        "export const scale = (x: number): number => {\n  return x;\n};\n",
    );
    let discovered = discover_symbols(
        &root,
        &["src/pick.ts".to_string(), "src/scale.ts".to_string()],
    )
    .unwrap();
    let names: Vec<_> = discovered
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(names, ["pick", "scale"]);
}

#[test]
fn contract_missing_and_ambiguous_symbols_are_structured_errors() {
    let (_dir, root) = root();
    write(
        &root,
        "src/Math.java",
        "class Math {\n public int f(int x) { return x; }\n public int f(long x) { return 0; }\n}\n",
    );
    for (symbol, code) in [
        ("missing", AuthoringErrorCode::NotFound),
        ("f", AuthoringErrorCode::AmbiguousSymbol),
    ] {
        let err = scaffold_contracts(
            &root,
            &ScaffoldContractsRequest {
                contracts: vec![ContractTarget {
                    file: "src/Math.java".into(),
                    symbol: symbol.into(),
                    requires: vec!["true;".into()],
                    ensures: vec![],
                }],
                mutation: apply_for(&root, &["src/Math.java"]),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, code);
    }
}

#[test]
fn existing_contracts_and_crlf_are_idempotent() {
    let (_dir, root) = root();
    write(
        &root,
        "src/math.ts",
        "export function clamp(x: number) {\r\n  return x;\r\n}\r\n",
    );
    let request = |mutation| ScaffoldContractsRequest {
        contracts: vec![ContractTarget {
            file: "src/math.ts".into(),
            symbol: "clamp".into(),
            requires: vec!["x >= 0".into()],
            ensures: vec!["result >= 0".into()],
        }],
        mutation,
    };
    scaffold_contracts(&root, &request(apply_for(&root, &["src/math.ts"]))).unwrap();
    let once = fs::read(root.as_path().join("src/math.ts")).unwrap();
    assert!(once.windows(2).any(|window| window == b"\r\n"));
    let second = scaffold_contracts(&root, &request(apply_for(&root, &["src/math.ts"]))).unwrap();
    assert_eq!(second.unchanged, vec!["src/math.ts"]);
    assert_eq!(fs::read(root.as_path().join("src/math.ts")).unwrap(), once);
}

#[test]
fn identical_clauses_are_scoped_to_each_symbol() {
    let (_dir, root) = root();
    write(
        &root,
        "src/functions.ts",
        "export function first(x: number) {\n  //@ requires x >= 0\n  return x;\n}\n\nexport function second(x: number) {\n  return x;\n}\n",
    );
    scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![ContractTarget {
                file: "src/functions.ts".into(),
                symbol: "second".into(),
                requires: vec!["x >= 0".into()],
                ensures: vec![],
            }],
            mutation: apply_for(&root, &["src/functions.ts"]),
        },
    )
    .unwrap();
    let source = fs::read_to_string(root.as_path().join("src/functions.ts")).unwrap();
    assert_eq!(source.matches("//@ requires x >= 0").count(), 2);
}

#[test]
fn expression_arrows_are_not_discovered_as_contract_targets() {
    let (_dir, root) = root();
    write(
        &root,
        "src/functions.ts",
        "export const expression = (x: number) => x + 1;\nexport const block = (x: number) => {\n  return x + 1;\n};\n",
    );
    let discovered = discover_symbols(&root, &["src/functions.ts".into()]).unwrap();
    assert_eq!(discovered.symbols.len(), 1);
    assert_eq!(discovered.symbols[0].name, "block");
}

#[test]
fn upserts_requirements_and_rejects_request_conflicts() {
    let (_dir, root) = root();
    init_project(&root, &apply()).unwrap();
    let request = UpsertRequirementsRequest {
        requirements: vec![requirement("req-1")],
        mutation: apply_for(&root, &[MAPPING_PATH]),
    };
    upsert_requirements(&root, &request).unwrap();
    assert_eq!(project_status(&root).unwrap().requirement_count, 1);

    let err = upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![requirement("same"), requirement("same")],
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, AuthoringErrorCode::Conflict);
}

#[test]
fn malformed_mapping_is_rejected_without_replacement() {
    let (_dir, root) = root();
    write(&root, MAPPING_PATH, "not: [valid");
    let before = fs::read(root.as_path().join(MAPPING_PATH)).unwrap();
    let err = upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![requirement("req")],
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, AuthoringErrorCode::Malformed);
    assert_eq!(fs::read(root.as_path().join(MAPPING_PATH)).unwrap(), before);
}

#[test]
fn stale_hash_blocks_mutation() {
    let (_dir, root) = root();
    write(&root, "src/a.ts", "export function a() {}\n");
    let mut expected_sha256 = BTreeMap::new();
    expected_sha256.insert("src/a.ts".into(), "00".repeat(32));
    let err = scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![ContractTarget {
                file: "src/a.ts".into(),
                symbol: "a".into(),
                requires: vec!["true".into()],
                ensures: vec![],
            }],
            mutation: MutationOptions {
                mode: MutationMode::Apply,
                expected_sha256,
            },
        },
    )
    .unwrap_err();
    assert_eq!(err.code, AuthoringErrorCode::StaleContent);
    assert_eq!(
        fs::read_to_string(root.as_path().join("src/a.ts")).unwrap(),
        "export function a() {}\n"
    );
}

#[test]
fn stale_hash_prevents_all_files_in_a_multi_file_mutation() {
    let (_dir, root) = root();
    write(&root, "src/a.ts", "export function a() {}\n");
    write(&root, "src/z.ts", "export function z() {}\n");
    let before_a = fs::read(root.as_path().join("src/a.ts")).unwrap();
    let before_z = fs::read(root.as_path().join("src/z.ts")).unwrap();
    let mut expected_sha256 = BTreeMap::new();
    expected_sha256.insert("src/a.ts".into(), sha256(&before_a));
    expected_sha256.insert("src/z.ts".into(), "00".repeat(32));

    let error = scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![
                ContractTarget {
                    file: "src/a.ts".into(),
                    symbol: "a".into(),
                    requires: vec!["true".into()],
                    ensures: vec![],
                },
                ContractTarget {
                    file: "src/z.ts".into(),
                    symbol: "z".into(),
                    requires: vec!["true".into()],
                    ensures: vec![],
                },
            ],
            mutation: MutationOptions {
                mode: MutationMode::Apply,
                expected_sha256,
            },
        },
    )
    .unwrap_err();

    assert_eq!(error.code, AuthoringErrorCode::StaleContent);
    assert_eq!(fs::read(root.as_path().join("src/a.ts")).unwrap(), before_a);
    assert_eq!(fs::read(root.as_path().join("src/z.ts")).unwrap(), before_z);
}

#[test]
fn traversal_absolute_and_symlink_escapes_are_rejected() {
    let (dir, root) = root();
    for path in ["../escape.ts", "/tmp/escape.ts"] {
        let err = discover_symbols(&root, &[path.into()]).unwrap_err();
        assert_eq!(err.code, AuthoringErrorCode::UnsafePath);
    }

    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("escape.ts"),
        "export function escape() {}",
    )
    .unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let err = discover_symbols(&root, &["link/escape.ts".into()]).unwrap_err();
        assert_eq!(err.code, AuthoringErrorCode::UnsafePath);
    }
}

#[test]
fn atomic_write_preserves_permissions_and_leaves_no_temp_file() {
    let (_dir, root) = root();
    write(&root, "src/a.ts", "export function a() {}\n");
    let path = root.as_path().join("src/a.ts");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o744)).unwrap();
    }
    scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![ContractTarget {
                file: "src/a.ts".into(),
                symbol: "a".into(),
                requires: vec!["true".into()],
                ensures: vec![],
            }],
            mutation: apply_for(&root, &["src/a.ts"]),
        },
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o744
        );
    }
    assert!(
        fs::read_dir(root.as_path().join("src"))
            .unwrap()
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .contains(".forall-write-"))
    );
}

#[test]
fn validates_missing_files_symbols_and_safe_mapping_paths() {
    let (_dir, root) = root();
    init_project(&root, &apply()).unwrap();
    let mut missing_file = requirement("missing-file");
    missing_file.code = Some(CodeRef {
        file: "src/missing.ts".into(),
        symbols: vec!["nope".into()],
    });
    let mut unsafe_file = requirement("unsafe");
    unsafe_file.code = Some(CodeRef {
        file: "../escape.ts".into(),
        symbols: vec!["escape".into()],
    });
    upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![missing_file, unsafe_file],
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap();
    let validation = validate_authoring(&root).unwrap();
    assert!(!validation.valid);
    assert_eq!(validation.issues.len(), 2);
}

#[test]
fn scaffolds_property_and_merges_mapping_link() {
    let (_dir, root) = root();
    init_project(&root, &apply()).unwrap();
    upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![requirement("clamp-bounds")],
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap();
    let body = "export default async function runPropertyTests() {\n  return { ok: true };\n}\n";
    let output = scaffold_property(
        &root,
        &ScaffoldPropertyRequest {
            requirement_id: "clamp-bounds".into(),
            body: body.into(),
            symbol: Some("runPropertyTests".into()),
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap();
    assert!(
        output
            .created
            .contains(&".forall/scenarios/clamp-bounds.property.ts".into())
    );
    let mapping: Mapping =
        serde_yaml::from_slice(&fs::read(root.as_path().join(MAPPING_PATH)).unwrap()).unwrap();
    let req = &mapping.requirements[0];
    assert!(req.property_tested);
    assert_eq!(
        req.property.as_ref().unwrap().file,
        ".forall/scenarios/clamp-bounds.property.ts"
    );
    assert_eq!(
        fs::read_to_string(
            root.as_path()
                .join(".forall/scenarios/clamp-bounds.property.ts")
        )
        .unwrap(),
        body
    );
}

#[test]
fn property_conflicts_and_missing_body_are_rejected() {
    let (_dir, root) = root();
    init_project(&root, &apply()).unwrap();
    let mut req = requirement("prop");
    req.property = Some(PropertyRef {
        file: ".forall/scenarios/other.property.ts".into(),
        symbol: None,
    });
    req.property_tested = true;
    upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![req],
            mutation: apply_for(&root, &[MAPPING_PATH]),
        },
    )
    .unwrap();
    let request = ScaffoldPropertyRequest {
        requirement_id: "prop".into(),
        body: "export default () => ({ ok: true });\n".into(),
        symbol: None,
        mutation: apply_for(&root, &[MAPPING_PATH]),
    };
    assert_eq!(
        scaffold_property(&root, &request).unwrap_err().code,
        AuthoringErrorCode::Conflict
    );
    let mut empty = request;
    empty.body = " ".into();
    assert_eq!(
        scaffold_property(&root, &empty).unwrap_err().code,
        AuthoringErrorCode::Malformed
    );
}
