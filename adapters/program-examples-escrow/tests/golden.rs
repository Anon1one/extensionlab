use extensionlab_adapter_program_examples_escrow::{
    ProgramExamplesEscrowAdapter, DEPOSIT, INITIAL_BALANCE, REVISION,
};
use extensionlab_core::{
    adapter::Adapter,
    invariants::{CheckStatus, InvariantKind},
    matrix::{self, IssuerAction, Scenario, Step},
    report::{FailOn, RunResult, RunStatus, StepOutcome},
    runner::Runner,
};

fn adapter() -> Option<ProgramExamplesEscrowAdapter> {
    let adapter = ProgramExamplesEscrowAdapter::default();
    if adapter.so_path.is_file() {
        return Some(adapter);
    }
    assert!(
        std::env::var_os("EXTENSIONLAB_REQUIRE_TARGET_BINARIES").is_none(),
        "required program-examples binary missing: {}",
        adapter.so_path.display()
    );
    eprintln!(
        "skipping upstream escrow run: {} missing; fetch the pinned target and build it first",
        adapter.so_path.display()
    );
    None
}

fn rule(run: &RunResult) -> Option<&str> {
    run.diagnosis.as_ref().map(|d| d.rule.as_str())
}

fn check(run: &RunResult, invariant: InvariantKind) -> CheckStatus {
    run.invariants
        .iter()
        .find(|c| c.invariant == invariant)
        .unwrap()
        .status
}

#[test]
fn missing_binary_is_a_harness_error() {
    let mut adapter = ProgramExamplesEscrowAdapter {
        so_path: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing/escrow.so"),
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
        .contains("scripts/fetch_targets.sh"));
}

#[test]
fn default_matrix_confirms_both_fee_blocked_exits() {
    let Some(mut adapter) = adapter() else { return };
    let report = Runner::default()
        .run(&mut adapter, &matrix::preset("default").unwrap())
        .unwrap();
    assert_eq!(report.summary.total, 32);
    assert_eq!(report.summary.pass, 17);
    assert_eq!(report.summary.not_applicable, 2);
    assert_eq!(report.summary.rejected, 4);
    assert_eq!(report.summary.fail, 9);
    assert_eq!(report.summary.error, 0);
    assert_eq!(report.exit_code(FailOn::Fail), 1);
    for exit in ["take_offer", "refund_offer"] {
        let run = report
            .run(&format!(
                "permanent_delegate@mint_b/make_offer_drain_{exit}"
            ))
            .unwrap();
        assert_eq!(run.status, RunStatus::NotApplicable);
        assert_eq!(run.steps[1].outcome, StepOutcome::NotApplicable);
        assert!(run.steps[1].transactions.is_empty());
    }
    for run in &report.runs {
        assert!(
            run.harness_error.is_none(),
            "{}: {:?}",
            run.id,
            run.harness_error
        );
        assert_eq!(
            check(run, InvariantKind::Conservation),
            CheckStatus::Held,
            "{}",
            run.id
        );
    }
    for config in ["spl_token", "token_2022"] {
        for exit in ["take_offer", "refund_offer"] {
            let id = format!("{config}/make_offer_{exit}");
            let run = report.run(&id).unwrap();
            assert_eq!(run.status, RunStatus::Pass, "{id}: {:?}", run.invariants);
            assert_eq!(run.baseline, config == "spl_token");
        }
    }
    for exit in ["take_offer", "refund_offer"] {
        let id = format!("transfer_fee_200bps@mint_a/make_offer_{exit}");
        let run = report.run(&id).unwrap();
        assert_eq!(run.status, RunStatus::Fail);
        assert_eq!(rule(run), Some("transfer_fee.close_with_withheld"));
        let (_, tx) = run.first_failed_tx().unwrap();
        assert!(tx.error.as_ref().unwrap().contains("Custom(35)"));
        assert!(tx
            .logs
            .iter()
            .any(|l| l.contains("withheld fee balance is zero")));
        assert_eq!(
            check(run, InvariantKind::Refundability),
            CheckStatus::Violated
        );
        let vault = run.balances.iter().find(|b| b.label == "vault").unwrap();
        assert_eq!(
            (vault.actual_after, vault.withheld_after),
            (9_800_000, 200_000)
        );
        assert!(vault.exists_after);
        // The failed exit rolls back transfers and does not charge the taker B.
        let taker_b = run
            .balances
            .iter()
            .find(|b| b.label == "taker_ata_b")
            .unwrap();
        assert_eq!(taker_b.actual_after, INITIAL_BALANCE);
    }
    let program = report
        .environment
        .programs
        .iter()
        .find(|p| p.name == adapter.name())
        .unwrap();
    assert!(program.source.contains(REVISION));
    assert_eq!(program.sha256.as_deref().map(str::len), Some(64));
}

#[test]
fn harvest_unblocks_both_exits_but_nominal_amounts_still_fail() {
    let Some(mut adapter) = adapter() else { return };
    let runner = Runner::default();
    let planned = runner
        .plan(&adapter, &matrix::preset("default").unwrap())
        .unwrap();
    for exit in ["take_offer", "refund_offer"] {
        let mut case = planned
            .iter()
            .find(|c| c.id == format!("transfer_fee_200bps@mint_a/make_offer_{exit}"))
            .unwrap()
            .clone();
        case.id = format!("transfer_fee_200bps@mint_a/make_harvest_{exit}");
        case.scenario = Scenario {
            name: format!("make_harvest_{exit}"),
            steps: vec![
                Step::flow("make_offer"),
                Step::Issuer {
                    action: IssuerAction::HarvestWithheld {
                        slot: "mint_a".into(),
                    },
                },
                Step::flow(exit),
            ],
            requires_extension: Some("transfer_fee".into()),
        };
        let run = runner.run_case(&mut adapter, &case).unwrap();
        assert!(run.harness_error.is_none(), "{:?}", run.harness_error);
        assert!(run
            .steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Succeeded));
        assert_eq!(
            run.status,
            RunStatus::Fail,
            "nominal expectations must remain unchanged"
        );
        assert_eq!(rule(&run), Some("transfer_fee.received_equals_sent"));
        assert_eq!(check(&run, InvariantKind::Refundability), CheckStatus::Held);
        assert_eq!(check(&run, InvariantKind::Closeability), CheckStatus::Held);
        let label = if exit == "take_offer" {
            "taker_ata_a"
        } else {
            "maker_ata_a"
        };
        let recipient = run.balances.iter().find(|b| b.label == label).unwrap();
        let expected = if exit == "take_offer" {
            9_604_000
        } else {
            INITIAL_BALANCE - DEPOSIT + 9_604_000
        };
        assert_eq!(recipient.actual_after, expected);
        assert_eq!(recipient.withheld_after, 196_000);
    }
}

#[test]
fn permanent_delegate_moves_real_custody_before_each_exit() {
    let Some(mut adapter) = adapter() else { return };
    let runner = Runner::default();
    for case in runner
        .plan(&adapter, &matrix::preset("default").unwrap())
        .unwrap()
        .into_iter()
        .filter(|c| {
            c.id.starts_with("permanent_delegate@mint_a/") && c.scenario.name.contains("_drain_")
        })
    {
        let run = runner.run_case(&mut adapter, &case).unwrap();
        assert!(run.harness_error.is_none(), "{:?}", run.harness_error);
        assert_eq!(run.steps[1].transactions.len(), 1);
        assert!(run
            .steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Succeeded));
        assert_eq!(run.status, RunStatus::Fail);
        assert_eq!(rule(&run), Some("permanent_delegate.custody_exclusive"));
        let issuer = run
            .balances
            .iter()
            .find(|b| b.label == "issuer[mint_a]")
            .unwrap();
        assert_eq!(issuer.actual_after, DEPOSIT);
        assert_eq!(
            check(&run, InvariantKind::Refundability),
            CheckStatus::Violated
        );
    }
}
