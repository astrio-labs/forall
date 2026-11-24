use std::fs;
use std::path::Path;

fn main() {
    let schemas_dir = Path::new("schemas/forall");
    println!("cargo:rerun-if-changed={}", schemas_dir.display());
    visit_dir(schemas_dir);
}

fn visit_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            visit_dir(&path);
        }
    }
}
