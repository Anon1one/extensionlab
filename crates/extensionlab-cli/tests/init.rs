use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "extensionlab-init-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating test directory {}: {error}", path.display()),
            }
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn init_creates_an_isolated_crate_and_refuses_to_overwrite_it() {
    let scratch = Scratch::new();
    let out = scratch.0.join("adapter with spaces");
    let sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let create = || {
        Command::new(env!("CARGO_BIN_EXE_extensionlab"))
            .args(["init", "--name", "trial-vault", "--out"])
            .arg(&out)
            .arg("--extensionlab-path")
            .arg(&sdk)
            .output()
            .unwrap()
    };
    let first = create();
    assert!(first.status.success(), "{first:?}");
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("[workspace]"));
    assert!(manifest.contains("name = \"trial-vault\""));
    let source = std::fs::read_to_string(out.join("src/lib.rs")).unwrap();
    assert!(source.contains("load the protocol's real program binary"));
    assert!(source.contains("state protocol balance expectations"));
    std::fs::write(out.join("src/lib.rs"), "user-owned edits").unwrap();
    assert_eq!(create().status.code(), Some(3));
    assert_eq!(
        std::fs::read_to_string(out.join("src/lib.rs")).unwrap(),
        "user-owned edits"
    );
}

#[test]
fn init_rejects_names_and_invalid_sdk_paths_before_writing_files() {
    let scratch = Scratch::new();
    let out = scratch.0.join("adapter");
    for name in ["../escape", "bad name", "AName", "quote\"name"] {
        let result = Command::new(env!("CARGO_BIN_EXE_extensionlab"))
            .args(["init", "--name", name, "--out"])
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!out.exists());
    }
    let result = Command::new(env!("CARGO_BIN_EXE_extensionlab"))
        .args(["init", "--name", "trial", "--out"])
        .arg(&out)
        .arg("--extensionlab-path")
        .arg(&scratch.0)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(3));
    assert!(!out.exists());
}
