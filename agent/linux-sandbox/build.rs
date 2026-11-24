fn main() {
    println!("cargo:rerun-if-env-changed=FORALL_BWRAP_SHA256");
    println!("cargo:rerun-if-env-changed=CODEX_BWRAP_SHA256");
}
