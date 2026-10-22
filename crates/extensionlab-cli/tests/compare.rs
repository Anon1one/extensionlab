use extensionlab_core::report::{Report, RunStatus, Summary};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "extensionlab-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, report: &mut Report) {
    report.summary = Summary::of(&report.runs);
    report.matrix.cases = report.runs.len();
    std::fs::write(path, report.to_json().unwrap()).unwrap();
}

#[test]
fn comparison_command_writes_evidence_and_propagates_gate_exit_codes() {
    let scratch = Scratch::new();
    let before = scratch.file("before.json");
    let after = scratch.file("after.json");
    let out = scratch.file("comparison");
    let mut report = Report::from_json(include_str!(
        "../../extensionlab-core/tests/fixtures/example_report.json"
    ))
    .unwrap();
    write(&before, &mut report);
    write(&after, &mut report);
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_extensionlab"))
            .args(["report", "compare"])
            .arg(&before)
            .arg(&after)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap()
    };
    let same = run();
    assert_eq!(same.status.code(), Some(0), "{same:?}");
    assert!(out.join("comparison.json").is_file());
    assert!(std::fs::read_to_string(out.join("comparison.md"))
        .unwrap()
        .contains("4 unchanged cases"));
    report.runs.pop();
    write(&after, &mut report);
    assert_eq!(
        run().status.code(),
        Some(1),
        "removing a case must fail coverage"
    );
    report.runs[0].status = RunStatus::Error;
    write(&after, &mut report);
    assert_eq!(
        run().status.code(),
        Some(3),
        "harness failures must never turn green"
    );
    std::fs::write(&after, "{broken JSON").unwrap();
    assert_eq!(run().status.code(), Some(3));
}

#[test]
fn runtime_hash_requires_a_binary_path() {
    let output = Command::new(env!("CARGO_BIN_EXE_extensionlab"))
        .args([
            "run",
            "--adapter",
            "escrow",
            "--token-2022-sha256",
            &"a".repeat(64),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--token-2022-so"));
}
