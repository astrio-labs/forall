use std::fs;
use std::process::Command;

use forall_utils_cargo_bin::cargo_bin;

#[test]
fn standalone_binary_initializes_embedded_assets_from_unrelated_cwd() {
    let project = tempfile::tempdir().expect("temporary project");
    let home = tempfile::tempdir().expect("temporary Forall home");
    let bin = cargo_bin("forall-workflow").expect("forall-workflow test binary");

    let init = Command::new(&bin)
        .args(["init", "--agents-md"])
        .current_dir(project.path())
        .env("FORALL_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .expect("run standalone init");
    assert!(
        init.status.success(),
        "init failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );

    assert_eq!(
        fs::read(project.path().join(".forall/AGENTS.md")).expect("installed AGENTS.md"),
        include_bytes!("../schemas/forall/templates/AGENTS.md")
    );

    for (name, expected) in [
        (
            "forall-apply",
            include_bytes!("../schemas/forall/skills/forall-apply/SKILL.md").as_slice(),
        ),
        (
            "forall-archive",
            include_bytes!("../schemas/forall/skills/forall-archive/SKILL.md").as_slice(),
        ),
        (
            "forall-continue",
            include_bytes!("../schemas/forall/skills/forall-continue/SKILL.md").as_slice(),
        ),
        (
            "forall-propose",
            include_bytes!("../schemas/forall/skills/forall-propose/SKILL.md").as_slice(),
        ),
        (
            "forall-verify",
            include_bytes!("../schemas/forall/skills/forall-verify/SKILL.md").as_slice(),
        ),
    ] {
        assert_eq!(
            fs::read(home.path().join("skills").join(name).join("SKILL.md"))
                .unwrap_or_else(|err| panic!("installed {name}: {err}")),
            expected
        );
    }

    let propose = Command::new(&bin)
        .args(["propose", "embedded-assets"])
        .current_dir(project.path())
        .env("FORALL_HOME", home.path())
        .env("HOME", home.path())
        .output()
        .expect("run standalone propose");
    assert!(
        propose.status.success(),
        "propose failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&propose.stdout),
        String::from_utf8_lossy(&propose.stderr)
    );

    let proposal = fs::read_to_string(
        project
            .path()
            .join(".forall/workflow/changes/embedded-assets/proposal.md"),
    )
    .expect("rendered embedded proposal");
    assert!(proposal.contains("# Proposal: embedded-assets"));
    assert!(proposal.contains("author.* tools before native verify.*"));
}
