//! Golden test: the runner, driving the escrow adapter, reproduces the two
//! findings of research/01_repro_escrow_fee_token.md.
//!
//! - F1: `refund` fails for every Token-2022 mint with Anchor
//!   `ConstraintAssociated` (2009).
//! - F2: `take` with a fee-bearing `mint_a` fails at `CloseAccount` with
//!   Token-2022 `AccountHasWithheldTransferFees` (35, 0x23); the vault holds
//!   9,800,000 plus 200,000 withheld.
//!
//! Skipped (with a message) when the escrow binary has not been built.

use extensionlab_adapter_escrow::{EscrowAdapter, DEPOSIT};
use extensionlab_core::{
    invariants::{CheckStatus, InvariantKind},
    matrix::{Matrix, SlotStrategy},
    mint::{AccountExtension, MintConfig, MintExtension},
    report::{FailOn, Report, RunResult, RunStatus, EXIT_FAILURES},
    runner::{Runner, RunnerOptions},
};

fn golden_matrix() -> Matrix {
    Matrix {
        name: "golden".into(),
        description: "SPL Token, plain Token-2022, and a 200 bps transfer fee".into(),
        strategy: SlotStrategy::OneSlotAtATime,
        configs: vec![
            MintConfig::spl_token("spl_token", 6),
            MintConfig::token_2022("token_2022", 6),
            MintConfig::token_2022("transfer_fee_200bps", 6).with(MintExtension::TransferFee {
                basis_points: 200,
                maximum_fee: u64::MAX,
            }),
        ],
    }
}

fn binary_available(adapter: &EscrowAdapter) -> bool {
    if adapter.so_path.is_file() {
        return true;
    }
    assert!(
        std::env::var_os("EXTENSIONLAB_REQUIRE_TARGET_BINARIES").is_none(),
        "required escrow binary missing: {}",
        adapter.so_path.display()
    );
    eprintln!(
        "skipping: {} is missing; run `cargo build-sbf` in targets/escrow",
        adapter.so_path.display()
    );
    false
}

fn run_matrix(seed: u64) -> Option<Report> {
    let mut adapter = EscrowAdapter::new();
    if !binary_available(&adapter) {
        return None;
    }
    let runner = Runner::new(RunnerOptions {
        seed,
        keep_success_logs: false,
    });
    Some(runner.run(&mut adapter, &golden_matrix()).expect("runner"))
}

fn find<'a>(report: &'a Report, id: &str) -> &'a RunResult {
    report.runs.iter().find(|r| r.id == id).unwrap_or_else(|| {
        panic!(
            "no run `{id}`; runs: {:?}",
            report.runs.iter().map(|r| &r.id).collect::<Vec<_>>()
        )
    })
}

fn first_error(run: &RunResult) -> String {
    run.first_failed_tx()
        .and_then(|(_, tx)| tx.error.clone())
        .unwrap_or_else(|| panic!("`{}` has no failed transaction", run.id))
}

fn refundability(run: &RunResult) -> CheckStatus {
    run.invariants
        .iter()
        .find(|c| c.invariant == InvariantKind::Refundability)
        .map(|c| c.status)
        .unwrap_or_else(|| panic!("`{}` has no refundability check", run.id))
}

fn rule(run: &RunResult) -> Option<&str> {
    run.diagnosis.as_ref().map(|d| d.rule.as_str())
}

#[test]
fn escrow_matrix_reproduces_f1_and_f2() {
    let Some(report) = run_matrix(0) else { return };
    for r in &report.runs {
        assert!(
            r.harness_error.is_none(),
            "{}: harness error {:?}",
            r.id,
            r.harness_error
        );
    }

    // The SPL Token baseline and a plain Token-2022 take work.
    for id in [
        "spl_token/make_take",
        "spl_token/make_refund",
        "token_2022/make_take",
    ] {
        let r = find(&report, id);
        assert_eq!(r.status, RunStatus::Pass, "{id}: {:#?}", r.invariants);
    }

    // Only the SPL Token runs are baseline, so F1 on plain Token-2022 is an
    // ordinary failure (exit 1), not a broken baseline (exit 4).
    for r in &report.runs {
        assert_eq!(r.baseline, r.id.starts_with("spl_token/"), "{}", r.id);
    }
    assert_eq!(report.exit_code(FailOn::Fail), EXIT_FAILURES);

    // F1: refund fails with 2009 for every Token-2022 mint, fee or no fee.
    for id in [
        "token_2022/make_refund",
        "transfer_fee_200bps@mint_a/make_refund",
    ] {
        let r = find(&report, id);
        assert_eq!(r.status, RunStatus::Fail, "{id}");
        assert!(
            first_error(r).contains("Custom(2009)"),
            "{id}: {}",
            first_error(r)
        );
        assert_eq!(refundability(r), CheckStatus::Violated, "{id}");
        let d = r
            .diagnosis
            .as_ref()
            .unwrap_or_else(|| panic!("{id}: no diagnosis"));
        assert_eq!(d.rule, "token_2022.ata_constraint_without_program", "{id}");
    }

    // F2: take with a fee on mint_a fails at CloseAccount with 35 (0x23).
    let r = find(&report, "transfer_fee_200bps@mint_a/make_take");
    assert_eq!(r.status, RunStatus::Fail);
    assert!(first_error(r).contains("Custom(35)"), "{}", first_error(r));
    assert_eq!(refundability(r), CheckStatus::Violated);
    assert_eq!(
        r.diagnosis.as_ref().map(|d| d.rule.as_str()),
        Some("transfer_fee.close_with_withheld")
    );
    let vault = r
        .balances
        .iter()
        .find(|b| b.label == "vault")
        .expect("vault row");
    assert_eq!(vault.actual_after, DEPOSIT - 200_000);
    assert_eq!(vault.withheld_after, 200_000);
    assert!(vault.exists_after);
}

#[test]
fn reports_are_deterministic_for_a_seed() {
    let Some(a) = run_matrix(7) else { return };
    let b = run_matrix(7).expect("second run");
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "same seed, different report"
    );
    let c = run_matrix(8).expect("third run");
    let sig = |r: &Report| r.runs[0].steps[0].transactions[0].signature.clone();
    assert_ne!(
        sig(&a),
        sig(&c),
        "different seeds should give different keypairs"
    );
}

// ---------------------------------------------------------------------------
// Runner behaviour on custom scenarios, using the escrow as the program.

use extensionlab_adapter_escrow::EXPIRATION;
use extensionlab_core::{
    adapter::{ActorSpec, Adapter, ExpectedDelta, FlowSpec, FlowTx, TrackedAccount, World},
    matrix::{IssuerAction, Scenario, Step},
    report::ProgramInfo,
    Result,
};

/// The escrow adapter with its own scenarios, and optionally a `make` flow
/// that sends a second transaction (another flow's) right after it.
struct Custom {
    inner: EscrowAdapter,
    scenarios: Vec<Scenario>,
    make_then: Option<&'static str>,
}

impl Adapter for Custom {
    fn name(&self) -> &str {
        "escrow-custom"
    }
    fn mint_slots(&self) -> Vec<String> {
        self.inner.mint_slots()
    }
    fn actors(&self) -> Vec<ActorSpec> {
        self.inner.actors()
    }
    fn flows(&self) -> Vec<FlowSpec> {
        self.inner.flows()
    }
    fn scenarios(&self) -> Vec<Scenario> {
        self.scenarios.clone()
    }
    fn load_programs(&self, svm: &mut litesvm::LiteSVM) -> Result<()> {
        self.inner.load_programs(svm)
    }
    fn programs(&self) -> Vec<ProgramInfo> {
        self.inner.programs()
    }
    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        let mut txs = self.inner.build_flow(flow, w)?;
        if let (Some(then), "make") = (self.make_then, flow) {
            txs.extend(self.inner.build_flow(then, w)?);
        }
        Ok(txs)
    }
    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        self.inner.tracked_accounts(w)
    }
    fn expected_deltas(&self, flow: &str, w: &World) -> Result<Vec<ExpectedDelta>> {
        let mut d = self.inner.expected_deltas(flow, w)?;
        if let (Some(then), "make") = (self.make_then, flow) {
            d.extend(self.inner.expected_deltas(then, w)?);
        }
        Ok(d)
    }
    fn closable_accounts(&self, w: &World) -> Result<Vec<String>> {
        self.inner.closable_accounts(w)
    }
}

fn scenario(name: &str, steps: Vec<Step>) -> Scenario {
    Scenario {
        name: name.into(),
        steps,
        requires_extension: None,
    }
}

fn run_custom(
    configs: Vec<MintConfig>,
    scenarios: Vec<Scenario>,
    make_then: Option<&'static str>,
) -> Option<Report> {
    let inner = EscrowAdapter::new();
    if !binary_available(&inner) {
        return None;
    }
    let mut adapter = Custom {
        inner,
        scenarios,
        make_then,
    };
    let matrix = Matrix {
        name: "custom".into(),
        description: String::new(),
        strategy: SlotStrategy::OneSlotAtATime,
        configs,
    };
    Some(
        Runner::default()
            .run(&mut adapter, &matrix)
            .expect("runner"),
    )
}

fn fee_200bps() -> MintConfig {
    MintConfig::token_2022("transfer_fee_200bps", 6).with(MintExtension::TransferFee {
        basis_points: 200,
        maximum_fee: u64::MAX,
    })
}

#[test]
fn a_step_with_two_transactions_sends_both() {
    // make followed by update in one step: both must land (each transaction
    // is signed against the blockhash current when it is sent).
    let make_take = scenario("make_take", vec![Step::flow("make"), Step::flow("take")]);
    let Some(r) = run_custom(
        vec![MintConfig::spl_token("spl_token", 6)],
        vec![make_take],
        Some("update"),
    ) else {
        return;
    };
    let run = find(&r, "spl_token/make_take");
    assert_eq!(run.steps[0].transactions.len(), 2);
    assert!(
        run.steps[0].transactions.iter().all(|t| t.error.is_none()),
        "{:#?}",
        run.steps[0]
    );
    assert_eq!(run.status, RunStatus::Pass, "{:#?}", run.invariants);
}

#[test]
fn a_partly_committed_first_step_is_a_failure_not_a_rejection() {
    // make, then refund in the same step: on Token-2022 the refund fails
    // (F1) after make committed, so funds are stuck.
    let make_take = scenario("make_take", vec![Step::flow("make"), Step::flow("take")]);
    let Some(r) = run_custom(
        vec![MintConfig::token_2022("token_2022", 6)],
        vec![make_take],
        Some("refund"),
    ) else {
        return;
    };
    let run = find(&r, "token_2022/make_take");
    assert_eq!(run.status, RunStatus::Fail);
    let eb = run
        .invariants
        .iter()
        .find(|c| c.invariant == InvariantKind::ExpectedBalances)
        .unwrap();
    assert_eq!(eb.status, CheckStatus::Violated, "{}", eb.detail);
}

#[test]
fn a_clock_step_before_a_rejected_entry_keeps_it_a_rejection() {
    let s = scenario(
        "clock_make_take",
        vec![
            Step::AdvanceClock { seconds: 1 },
            Step::flow("make"),
            Step::flow("take"),
        ],
    );
    let non_transferable =
        MintConfig::token_2022("non_transferable", 6).with(MintExtension::NonTransferable);
    let Some(r) = run_custom(vec![non_transferable], vec![s], None) else {
        return;
    };
    let run = find(&r, "non_transferable@mint_a/clock_make_take");
    assert_eq!(run.status, RunStatus::Rejected, "{:#?}", run.invariants);
    let eb = run
        .invariants
        .iter()
        .find(|c| c.invariant == InvariantKind::ExpectedBalances)
        .unwrap();
    assert_eq!(eb.status, CheckStatus::Held, "{}", eb.detail);
}

#[test]
fn harvest_then_take_pays_the_taker_net_of_two_fees() {
    // research/01: after a harvest, take succeeds and the taker receives
    // 9,604,000 of 10,000,000.
    let s = scenario(
        "make_harvest_take",
        vec![
            Step::flow("make"),
            Step::Issuer {
                action: IssuerAction::HarvestWithheld {
                    slot: "mint_a".into(),
                },
            },
            Step::flow("take"),
        ],
    );
    let Some(r) = run_custom(vec![fee_200bps()], vec![s], None) else {
        return;
    };
    let run = find(&r, "transfer_fee_200bps@mint_a/make_harvest_take");
    assert!(run
        .steps
        .iter()
        .all(|s| s.outcome == extensionlab_core::report::StepOutcome::Succeeded));
    let taker = run
        .balances
        .iter()
        .find(|b| b.label == "taker_ata_a")
        .unwrap();
    assert_eq!(taker.actual_after, 9_604_000);
    assert_eq!(refundability(run), CheckStatus::Held);
    assert_eq!(
        run.status,
        RunStatus::Fail,
        "nominal expectations ignore the fee"
    );
    assert_eq!(
        run.diagnosis.as_ref().map(|d| d.rule.as_str()),
        Some("transfer_fee.received_equals_sent")
    );
}

#[test]
fn expiry_and_freeze_scenarios() {
    let expired = scenario(
        "make_expire_take",
        vec![
            Step::flow("make"),
            Step::AdvanceClock {
                seconds: EXPIRATION + 1,
            },
            Step::flow("take"),
        ],
    );
    let frozen = scenario(
        "make_freeze_take",
        vec![
            Step::flow("make"),
            Step::Issuer {
                action: IssuerAction::FreezeCustody {
                    slot: "mint_a".into(),
                },
            },
            Step::flow("take"),
        ],
    );
    let Some(r) = run_custom(
        vec![MintConfig::spl_token("spl_token", 6)],
        vec![expired, frozen],
        None,
    ) else {
        return;
    };
    let run = find(&r, "spl_token/make_expire_take");
    assert!(
        first_error(run).contains("Custom(6000)"),
        "{}",
        first_error(run)
    ); // EscrowExpired
    let run = find(&r, "spl_token/make_freeze_take");
    assert!(
        first_error(run).contains("Custom(17)"),
        "{}",
        first_error(run)
    ); // AccountFrozen
    assert_eq!(
        run.diagnosis.as_ref().map(|d| d.rule.as_str()),
        Some("freeze_authority.custody_frozen")
    );
}

#[test]
fn existing_payout_accounts_expose_required_memos() {
    let mut adapter = EscrowAdapter::with_existing_payout_accounts();
    if !binary_available(&adapter) {
        return;
    }
    let matrix = Matrix {
        name: "memo".into(),
        description: String::new(),
        strategy: SlotStrategy::OneSlotAtATime,
        configs: vec![MintConfig::token_2022("memo_required", 6)
            .with_actor_account(extensionlab_core::mint::AccountExtension::MemoTransferRequired)],
    };
    let r = Runner::default()
        .run(&mut adapter, &matrix)
        .expect("runner");
    for id in [
        "memo_required@mint_a/make_take",
        "memo_required@mint_b/make_take",
    ] {
        let run = find(&r, id);
        assert_eq!(run.status, RunStatus::Fail, "{id}");
        assert!(
            first_error(run).contains("Custom(36)"),
            "{id}: {}",
            first_error(run)
        );
        assert_eq!(
            run.diagnosis.as_ref().map(|d| d.rule.as_str()),
            Some("memo_required.no_memo_needed"),
            "{id}"
        );
    }
    let escrow = r
        .environment
        .programs
        .iter()
        .find(|p| p.name == "escrowq32026")
        .unwrap();
    assert_eq!(escrow.sha256.as_ref().map(String::len), Some(64));
}

#[test]
fn cpi_guard_and_pause_scenarios() {
    let make_take = scenario("make_take", vec![Step::flow("make"), Step::flow("take")]);
    let make_pause_take = Scenario {
        name: "make_pause_take".into(),
        steps: vec![
            Step::flow("make"),
            Step::Issuer {
                action: IssuerAction::Pause {
                    slot: "mint_a".into(),
                },
            },
            Step::flow("take"),
        ],
        requires_extension: Some("pausable".into()),
    };
    let cpi_guard =
        MintConfig::token_2022("cpi_guard", 6).with_actor_account(AccountExtension::CpiGuard);
    let pausable = MintConfig::token_2022("pausable", 6).with(MintExtension::Pausable);
    let Some(r) = run_custom(
        vec![cpi_guard, pausable],
        vec![make_take, make_pause_take],
        None,
    ) else {
        return;
    };
    // Token-2022 10.0.0 logs this text with code 0x2a (42); the error string in
    // spl-token-2022-interface 2.1.0 adds "via CPI", so match the program's.
    let guard_text = "CPI Guard is enabled, and a program attempted to transfer user funds";
    let logs = |run: &RunResult| {
        run.first_failed_tx()
            .map(|(_, t)| t.logs.join("\n"))
            .unwrap_or_default()
    };

    // The maker's mint_a account has CPI Guard: make pulls the deposit by CPI
    // with the maker as signer, so Token-2022 refuses it and nothing commits.
    let run = find(&r, "cpi_guard@mint_a/make_take");
    assert_eq!(run.status, RunStatus::Rejected, "{:#?}", run.steps);
    assert!(logs(run).contains(guard_text), "{}", logs(run));
    assert!(
        first_error(run).contains("Custom(42)"),
        "{}",
        first_error(run)
    );
    assert_eq!(rule(run), Some("cpi_guard.user_signed_cpi"));
    // The taker's mint_b account has it: make commits, take is refused, and
    // the deposit stays in the vault.
    let run = find(&r, "cpi_guard@mint_b/make_take");
    assert_eq!(run.status, RunStatus::Fail, "{:#?}", run.steps);
    assert_eq!(
        run.steps[1].outcome,
        extensionlab_core::report::StepOutcome::Failed
    );
    assert!(logs(run).contains(guard_text), "{}", logs(run));
    assert_eq!(refundability(run), CheckStatus::Violated);
    assert_eq!(rule(run), Some("cpi_guard.user_signed_cpi"));

    // A paused mint_a blocks take after make; without the pause it works.
    let run = find(&r, "pausable@mint_a/make_take");
    assert_eq!(run.status, RunStatus::Pass, "{:#?}", run.invariants);
    assert_eq!(rule(run), None);
    let run = find(&r, "pausable@mint_a/make_pause_take");
    assert_eq!(run.status, RunStatus::Fail, "{:#?}", run.steps);
    assert!(logs(run).contains("paused on this mint"), "{}", logs(run));
    assert_eq!(refundability(run), CheckStatus::Violated);
    assert_eq!(rule(run), Some("pausable.paused_mint"));
}

#[test]
fn all_preset_runs_pause_and_cpi_guard_cases() {
    let mut adapter = EscrowAdapter::new();
    if !binary_available(&adapter) {
        return;
    }
    let matrix = extensionlab_core::matrix::preset("all").expect("preset");
    let r = Runner::default()
        .run(&mut adapter, &matrix)
        .expect("runner");
    for run in &r.runs {
        assert!(
            run.harness_error.is_none(),
            "{}: harness error {:?}",
            run.id,
            run.harness_error
        );
    }

    // The default pause scenarios exist only for the Pausable config.
    let paused: Vec<&str> = r
        .runs
        .iter()
        .map(|run| run.id.as_str())
        .filter(|id| id.contains("_pause_"))
        .collect();
    assert_eq!(
        paused,
        [
            "pausable@mint_a/make_pause_take",
            "pausable@mint_a/make_pause_refund",
            "pausable@mint_b/make_pause_take",
            "pausable@mint_b/make_pause_refund",
        ]
    );
    for slot in ["mint_a", "mint_b"] {
        // take moves both mints, so a pause of either blocks it after make.
        let id = format!("pausable@{slot}/make_pause_take");
        let run = find(&r, &id);
        assert_eq!(run.status, RunStatus::Fail, "{id}");
        assert!(
            run.first_failed_tx()
                .is_some_and(|(_, t)| t.logs.iter().any(|l| l.contains("paused on this mint"))),
            "{id}: {:#?}",
            run.steps
        );
        assert_eq!(refundability(run), CheckStatus::Violated, "{id}");
        assert_eq!(rule(run), Some("pausable.paused_mint"), "{id}");
        // refund fails on F1 before it moves any token, so the pause is never
        // reached.
        let id = format!("pausable@{slot}/make_pause_refund");
        let run = find(&r, &id);
        assert_eq!(run.status, RunStatus::Fail, "{id}");
        assert!(
            first_error(run).contains("Custom(2009)"),
            "{id}: {}",
            first_error(run)
        );
        assert_eq!(
            rule(run),
            Some("token_2022.ata_constraint_without_program"),
            "{id}"
        );
    }

    let run = find(&r, "cpi_guard@mint_a/make_take");
    assert_eq!(run.status, RunStatus::Rejected, "{:#?}", run.steps);
    assert_eq!(rule(run), Some("cpi_guard.user_signed_cpi"));
    let run = find(&r, "cpi_guard@mint_b/make_take");
    assert_eq!(run.status, RunStatus::Fail, "{:#?}", run.steps);
    assert_eq!(rule(run), Some("cpi_guard.user_signed_cpi"));
}
