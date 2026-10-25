use std::path::{Path, PathBuf};

use logging::Level;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tinybrowser-panic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_installed_hook_records_a_panic_in_the_log_file() {
    let dir = TempDir::new();
    let path = dir.path().join("log");
    logging::install(Level::Error, Some(path.clone()), std::time::SystemTime::now)
        .expect("install");
    let caught = std::panic::catch_unwind(|| panic!("hook probe {}", 7));
    assert!(caught.is_err());
    let contents = std::fs::read_to_string(&path).expect("read");
    assert!(contents.contains(" ERROR panic "), "{contents}");
    assert!(contents.contains("hook probe 7"), "{contents}");
    assert!(contents.contains("panic_hook.rs:"), "{contents}");
}
