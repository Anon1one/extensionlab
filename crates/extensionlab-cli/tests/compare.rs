use extensionlab_core::report::{Report, RunStatus, Summary};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "extensionlab-cli-{}-{}-{}",
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

#[test]
fn report_commands_reject_incomplete_and_duplicate_evidence() {
    let scratch = Scratch::new();
    let path = scratch.file("malformed.json");
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../extensionlab-core/tests/fixtures/example_report.json"
    ))
    .unwrap();
    let mut incomplete = original.clone();
    incomplete["runs"] = serde_json::json!([incomplete["runs"][0].clone()]);
    let mut duplicate = original.clone();
    let same_id = duplicate["runs"][0]["id"].clone();
    duplicate["runs"][1]["id"] = same_id;
    let mut missing_steps = original.clone();
    missing_steps["runs"][0]["steps"] = serde_json::json!([]);
    let mut flipped_rejected = original.clone();
    let mut flipped_not_applicable = original.clone();
    for (report, status) in [
        (&mut flipped_rejected, "rejected"),
        (&mut flipped_not_applicable, "not_applicable"),
    ] {
        for run in report["runs"].as_array_mut().unwrap() {
            if run["status"] == "fail" {
                run["status"] = serde_json::json!(status);
                run["invariants"] = serde_json::json!([]);
            }
        }
        report["summary"]["fail"] = serde_json::json!(0);
        if status == "rejected" {
            report["summary"]["rejected"] = serde_json::json!(3);
        } else {
            report["summary"]["not_applicable"] = serde_json::json!(2);
        }
    }
    let mut flipped_statuses = original;
    for run in flipped_statuses["runs"].as_array_mut().unwrap() {
        if run["status"] == "fail" {
            run["status"] = serde_json::json!("pass");
        }
    }
    flipped_statuses["summary"]["pass"] = serde_json::json!(3);
    flipped_statuses["summary"]["fail"] = serde_json::json!(0);
    for report in [
        incomplete,
        duplicate,
        flipped_statuses,
        missing_steps,
        flipped_rejected,
        flipped_not_applicable,
    ] {
        std::fs::write(&path, serde_json::to_vec(&report).unwrap()).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_extensionlab"))
            .args(["report", "exit-code"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(3), "{result:?}");
        let result = Command::new(env!("CARGO_BIN_EXE_extensionlab"))
            .arg("regress")
            .arg(&path)
            .args(["--run", "spl_token/make_refund"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(3), "{result:?}");
    }
}
