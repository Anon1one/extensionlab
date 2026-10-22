use extensionlab_adapter_sablier_merkle_instant::{
    FundingPolicy, SablierMerkleAdapter, DEPOSIT, REVISION,
};
use extensionlab_core::{
    invariants::{CheckStatus, InvariantKind},
    matrix,
    report::{FailOn, Report, RunResult, RunStatus, StepOutcome},
    runner::Runner,
};

fn report(policy: FundingPolicy, matrix: &str) -> Option<Report> {
    let mut adapter = SablierMerkleAdapter {
        funding_policy: policy,
        ..Default::default()
    };
    if !adapter.so_path.is_file() {
        assert!(
            std::env::var_os("EXTENSIONLAB_REQUIRE_TARGET_BINARIES").is_none(),
            "required Sablier binary missing: {}",
            adapter.so_path.display()
        );
        eprintln!("skipping Sablier golden tests: fetch/build the pinned target first");
        return None;
    }
    Some(
        Runner::default()
            .run(&mut adapter, &matrix::preset(matrix).unwrap())
            .unwrap(),
    )
}
fn check(run: &RunResult, kind: InvariantKind) -> CheckStatus {
    run.invariants
        .iter()
        .find(|c| c.invariant == kind)
        .unwrap()
        .status
}
fn balance<'a>(run: &'a RunResult, label: &str) -> &'a extensionlab_core::report::BalanceRow {
    run.balances.iter().find(|b| b.label == label).unwrap()
}

#[test]
fn missing_binary_is_a_harness_error() {
    let mut adapter = SablierMerkleAdapter {
        so_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing/airdrop.so"),
        ..Default::default()
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
        .contains("fetch_targets.sh solsab"));
}

#[test]
fn nominal_campaign_funding_leaves_a_fee_bearing_claim_underfunded() {
    let Some(report) = report(FundingPolicy::Nominal, "default") else {
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
        (18, 8, 4, 6, 0)
    );
    assert_eq!(report.exit_code(FailOn::Fail), 1);
    for token in ["spl_token", "token_2022"] {
        for exit in ["claim", "clawback"] {
            assert_eq!(
                report
                    .run(&format!("{token}/fund_campaign_{exit}"))
                    .unwrap()
                    .status,
                RunStatus::Pass
            );
        }
    }
    for exit in ["claim", "clawback"] {
        let run = report
            .run(&format!("transfer_fee_200bps@airdrop/fund_campaign_{exit}"))
            .unwrap();
        assert_eq!(run.status, RunStatus::Fail);
        assert!(run.harness_error.is_none());
        let (_, failure) = run.first_failed_tx().unwrap();
        assert!(failure.error.as_ref().unwrap().contains("Custom(1)"));
        assert!(failure
            .logs
            .iter()
            .any(|l| l.to_ascii_lowercase().contains("insufficient funds")));
        assert_eq!(
            (
                balance(run, "campaign_vault").actual_after,
                balance(run, "campaign_vault").withheld_after
            ),
            (9_800_000, 200_000)
        );
        assert_eq!(balance(run, "creator").actual_after, 90_000_000);
        assert_eq!(balance(run, "recipient").actual_after, 0);
        assert_eq!(check(run, InvariantKind::Conservation), CheckStatus::Held);
        assert_eq!(
            check(run, InvariantKind::Refundability),
            CheckStatus::Violated
        );
    }
    let program = report
        .environment
        .programs
        .iter()
        .find(|p| p.name == "sablier_merkle_instant")
        .unwrap();
    assert!(program.source.contains(REVISION));
    assert!(program.source.contains("BUSL-1.1"));
    assert_eq!(program.sha256.as_ref().unwrap().len(), 64);
}

#[test]
fn fee_aware_client_funding_makes_the_same_unmodified_program_settle() {
    let Some(report) = report(FundingPolicy::FeeAware, "default") else {
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
        (18, 10, 4, 4, 0)
    );
    for exit in ["claim", "clawback"] {
        let run = report
            .run(&format!("transfer_fee_200bps@airdrop/fund_campaign_{exit}"))
            .unwrap();
        assert_eq!(run.status, RunStatus::Pass, "{run:#?}");
        assert!(run
            .steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Succeeded));
        assert_eq!(
            (
                balance(run, "campaign_vault").actual_after,
                balance(run, "campaign_vault").withheld_after
            ),
            (0, 204_082)
        );
        assert_eq!(
            check(run, InvariantKind::ExpectedBalances),
            CheckStatus::Held
        );
        assert_eq!(check(run, InvariantKind::Refundability), CheckStatus::Held);
        if exit == "claim" {
            assert_eq!(balance(run, "recipient").actual_after, 9_800_000);
            assert_eq!(balance(run, "creator").actual_after, 89_795_918);
        } else {
            assert_eq!(balance(run, "creator").actual_after, 99_595_918);
        }
    }
}

#[test]
fn real_delegate_drain_and_pause_are_detected_while_direct_cpi_guard_funding_works() {
    let Some(report) = report(FundingPolicy::Nominal, "all") else {
        return;
    };
    assert_eq!(report.summary.error, 0);
    for exit in ["claim", "clawback"] {
        let run = report
            .run(&format!(
                "permanent_delegate@airdrop/fund_campaign_drain_{exit}"
            ))
            .unwrap();
        assert_eq!(run.status, RunStatus::Fail);
        assert_eq!(run.steps[1].outcome, StepOutcome::Succeeded);
        assert_eq!(run.steps[1].transactions.len(), 1);
        assert_eq!(balance(run, "issuer[airdrop]").actual_after, DEPOSIT);
        assert_eq!(
            run.diagnosis.as_ref().unwrap().rule,
            "permanent_delegate.custody_exclusive"
        );
        let paused = report
            .run(&format!("pausable@airdrop/fund_campaign_pause_{exit}"))
            .unwrap();
        assert_eq!(paused.status, RunStatus::Fail);
        assert_eq!(
            paused.diagnosis.as_ref().unwrap().rule,
            "pausable.paused_mint"
        );
        assert_eq!(
            report
                .run(&format!("cpi_guard@airdrop/fund_campaign_{exit}"))
                .unwrap()
                .status,
            RunStatus::Pass
        );
    }
}
