use std::io;
use std::path::Path;

use include_dir::{Dir, File};

pub static BUNDLED_SCHEMA_DIR: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/schemas/forall");

pub fn bundled_file(path: impl AsRef<Path>) -> io::Result<&'static File<'static>> {
    let path = path.as_ref();
    BUNDLED_SCHEMA_DIR.get_file(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("bundled workflow asset not found: {}", path.display()),
        )
    })
}

pub fn bundled_text(path: impl AsRef<Path>) -> io::Result<&'static str> {
    let path = path.as_ref();
    bundled_file(path)?.contents_utf8().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bundled workflow asset is not UTF-8: {}", path.display()),
        )
    })
}
