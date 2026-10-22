use extensionlab_adapter_train_htlc::{TrainHtlcAdapter, REVISION};
use extensionlab_core::{
    invariants::{CheckStatus, InvariantKind},
    matrix,
    report::{Report, RunResult, RunStatus, StepOutcome},
    runner::Runner,
};

fn report(matrix: &str) -> Option<Report> {
    let mut adapter = TrainHtlcAdapter::default();
    if !adapter.so_path.is_file() {
        assert!(
            std::env::var_os("EXTENSIONLAB_REQUIRE_TARGET_BINARIES").is_none(),
            "required Train binary missing: {}",
            adapter.so_path.display()
        );
        eprintln!("skipping Train golden tests: fetch/build the pinned target first");
        return None;
    }
    Some(
        Runner::default()
            .run(&mut adapter, &matrix::preset(matrix).unwrap())
            .unwrap(),
    )
}
fn balance<'a>(run: &'a RunResult, label: &str) -> &'a extensionlab_core::report::BalanceRow {
    run.balances.iter().find(|b| b.label == label).unwrap()
}
fn check(run: &RunResult, kind: InvariantKind) -> CheckStatus {
    run.invariants
        .iter()
        .find(|c| c.invariant == kind)
        .unwrap()
        .status
}

#[test]
fn missing_binary_is_a_harness_error() {
    let mut adapter = TrainHtlcAdapter {
        so_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing/train.so"),
    };
    let runner = Runner::default();
    let case = runner
        .plan(&adapter, &matrix::preset("baseline").unwrap())
        .unwrap()
        .remove(0);
    let run = runner.run_case(&mut adapter, &case).unwrap();
    assert_eq!(run.status, RunStatus::Error);
    assert!(run
        .harness_error
        .unwrap()
        .contains("fetch_targets.sh train"));
}

#[test]
fn baselines_pass_and_permanent_delegate_mints_are_rejected_before_custody() {
    let Some(report) = report("default") else {
        return;
    };
    assert_eq!(
        (
            report.summary.total,
            report.summary.pass,
            report.summary.rejected,
            report.summary.fail,
            report.summary.error
        ),
        (22, 10, 8, 4, 0)
    );
    for token in ["spl_token", "token_2022"] {
        for exit in ["redeem", "refund"] {
            assert_eq!(
                report.run(&format!("{token}/lock_{exit}")).unwrap().status,
                RunStatus::Pass
            );
        }
    }
    for scenario in [
        "lock_redeem",
        "lock_refund",
        "lock_drain_redeem",
        "lock_drain_refund",
    ] {
        let run = report
            .run(&format!("permanent_delegate@token/{scenario}"))
            .unwrap();
        assert_eq!(run.status, RunStatus::Rejected);
        assert_eq!(balance(run, "sender").actual_after, 100_000_000);
        assert!(!balance(run, "vault").exists_after);
        assert!(run
            .first_failed_tx()
            .unwrap()
            .1
            .logs
            .iter()
            .any(|l| l.contains("UnsupportedMintExtension")));
    }
    let program = report
        .environment
        .programs
        .iter()
        .find(|p| p.name == "train_htlc")
        .unwrap();
    assert!(program.source.contains(REVISION));
    assert_eq!(program.sha256.as_ref().unwrap().len(), 64);
}

#[test]
fn catching_the_close_error_does_not_recover_a_failed_cpi_in_this_runtime() {
    let Some(report) = report("default") else {
        return;
    };
    for exit in ["redeem", "refund"] {
        let run = report
            .run(&format!("transfer_fee_200bps@token/lock_{exit}"))
            .unwrap();
        assert_eq!(run.status, RunStatus::Fail);
        let (_, failure) = run.first_failed_tx().unwrap();
        assert!(failure.error.as_ref().unwrap().contains("Custom(35)"));
        assert!(failure.logs.iter().any(|l| l.contains("CloseAccount")));
        assert!(!failure
            .logs
            .iter()
            .any(|l| l.contains("vault close skipped")));
        assert_eq!(balance(run, "sender").actual_after, 90_000_000);
        assert_eq!(balance(run, "recipient").actual_after, 0);
        assert_eq!(
            (
                balance(run, "vault").actual_after,
                balance(run, "vault").withheld_after
            ),
            (9_800_000, 200_000)
        );
        assert_eq!(
            check(run, InvariantKind::CreditedEqualsReceived),
            CheckStatus::Held
        );
        assert_eq!(check(run, InvariantKind::Conservation), CheckStatus::Held);
        assert_eq!(
            check(run, InvariantKind::Refundability),
            CheckStatus::Violated
        );
    }
}

#[test]
fn harvest_unblocks_both_net_accounted_exits_and_other_extensions_are_exercised() {
    let Some(report) = report("all") else { return };
    assert_eq!(
        (
            report.summary.total,
            report.summary.pass,
            report.summary.rejected,
            report.summary.fail,
            report.summary.error
        ),
        (30, 12, 12, 6, 0)
    );
    for exit in ["redeem", "refund"] {
        let run = report
            .run(&format!("transfer_fee_200bps@token/lock_harvest_{exit}"))
            .unwrap();
        assert_eq!(run.status, RunStatus::Pass, "{run:#?}");
        assert!(run
            .steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Succeeded));
        assert_eq!(run.steps[1].transactions.len(), 1);
        assert!(!balance(run, "vault").exists_after);
        assert_eq!(
            check(run, InvariantKind::CreditedEqualsReceived),
            CheckStatus::Held
        );
        assert_eq!(
            check(run, InvariantKind::ExpectedBalances),
            CheckStatus::Held
        );
        assert_eq!(check(run, InvariantKind::Refundability), CheckStatus::Held);
        if exit == "redeem" {
            assert_eq!(balance(run, "recipient").actual_after, 9_604_000);
        } else {
            assert_eq!(balance(run, "sender").actual_after, 99_604_000);
        }
        let hook = report
            .run(&format!("transfer_hook_undeployed@token/lock_{exit}"))
            .unwrap();
        assert_eq!(hook.status, RunStatus::Rejected);
        assert!(hook
            .first_failed_tx()
            .unwrap()
            .1
            .logs
            .iter()
            .any(|l| l.contains("UnsupportedMintExtension")));
        let paused = report
            .run(&format!("pausable@token/lock_pause_{exit}"))
            .unwrap();
        assert_eq!(paused.status, RunStatus::Fail);
        assert_eq!(
            paused.diagnosis.as_ref().unwrap().rule,
            "pausable.paused_mint"
        );
    }
}
