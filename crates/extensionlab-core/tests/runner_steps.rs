//! Host tests for the runner's issuer and clock steps, with a program-free
//! adapter: a `vault` wallet whose token account is the custody account, and
//! two flows that are plain `transfer_checked` instructions (`deposit` from
//! `alice` into the vault, `withdraw` of the vault's whole balance back).
//!
//! No protocol binary is involved, so these run everywhere, CI included.

use extensionlab_core::{
    adapter::{
        ActorSpec, ActorTokenAccount, Adapter, ExpectedDelta, FlowKind, FlowSpec, FlowTx,
        TrackedAccount, World,
    },
    invariants::{CheckStatus, InvariantKind},
    matrix::{IssuerAction, Matrix, Scenario, SlotStrategy, Step},
    mint::{read_token_account, MintConfig, MintExtension},
    report::{Report, RunResult, RunStatus},
    runner::{Runner, RunnerOptions},
    Result,
};
use litesvm::LiteSVM;
use spl_token_2022_interface::instruction::transfer_checked;

const SLOT: &str = "token";
const DEPOSIT: u64 = 100_000;

struct Vault {
    scenarios: Vec<Scenario>,
}

impl Vault {
    fn transfer(&self, w: &World, from: &str, to: &str, amount: u64) -> Result<FlowTx> {
        let m = w.mint(SLOT)?;
        let ix = transfer_checked(
            &m.program_id(),
            &w.ata(from, SLOT)?,
            &m.address,
            &w.ata(to, SLOT)?,
            &w.pubkey(from)?,
            &[],
            amount,
            m.config.decimals,
        )
        .map_err(|e| extensionlab_core::Error::Adapter(e.to_string()))?;
        Ok(FlowTx {
            payer: from.into(),
            signers: Vec::new(),
            extra_signers: Vec::new(),
            instructions: vec![ix],
        })
    }
}

impl Adapter for Vault {
    fn name(&self) -> &str {
        "vault"
    }

    fn mint_slots(&self) -> Vec<String> {
        vec![SLOT.into()]
    }

    fn actors(&self) -> Vec<ActorSpec> {
        let account = |initial_balance| {
            vec![ActorTokenAccount {
                slot: SLOT.into(),
                initial_balance,
            }]
        };
        vec![
            ActorSpec {
                name: "alice".into(),
                lamports: 1_000_000_000,
                token_accounts: account(DEPOSIT),
            },
            ActorSpec {
                name: "vault".into(),
                lamports: 1_000_000_000,
                token_accounts: account(0),
            },
        ]
    }

    fn flows(&self) -> Vec<FlowSpec> {
        let flow = |name: &str, kind| FlowSpec {
            name: name.into(),
            kind,
            description: String::new(),
        };
        vec![
            flow("deposit", FlowKind::Entry),
            flow("withdraw", FlowKind::Exit),
        ]
    }

    fn scenarios(&self) -> Vec<Scenario> {
        self.scenarios.clone()
    }

    fn load_programs(&self, _svm: &mut LiteSVM) -> Result<()> {
        Ok(())
    }

    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        match flow {
            "deposit" => Ok(vec![self.transfer(w, "alice", "vault", DEPOSIT)?]),
            _ => {
                let held = read_token_account(&w.svm, &w.ata("vault", SLOT)?)?
                    .map(|v| v.amount)
                    .unwrap_or(0);
                Ok(vec![self.transfer(w, "vault", "alice", held)?])
            }
        }
    }

    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        Ok(vec![
            TrackedAccount {
                label: "alice".into(),
                address: w.ata("alice", SLOT)?,
                slot: SLOT.into(),
                custody: false,
            },
            TrackedAccount {
                label: "vault".into(),
                address: w.ata("vault", SLOT)?,
                slot: SLOT.into(),
                custody: true,
            },
        ])
    }

    fn expected_deltas(&self, _flow: &str, _w: &World) -> Result<Vec<ExpectedDelta>> {
        Ok(Vec::new())
    }
}

fn scenario(name: &str, requires: &str, middle: Vec<Step>) -> Scenario {
    let mut steps = vec![Step::flow("deposit")];
    steps.extend(middle);
    steps.push(Step::flow("withdraw"));
    Scenario {
        name: name.into(),
        steps,
        requires_extension: Some(requires.into()),
    }
}

fn set_fee(basis_points: u16) -> Step {
    Step::Issuer {
        action: IssuerAction::SetTransferFee {
            slot: SLOT.into(),
            basis_points,
            maximum_fee: None,
        },
    }
}

fn issuer(action: IssuerAction) -> Step {
    Step::Issuer { action }
}

fn run() -> Report {
    let scenarios = vec![
        scenario("set_fee_now", "transfer_fee", vec![set_fee(1_000)]),
        scenario(
            "set_fee_two_epochs",
            "transfer_fee",
            vec![set_fee(1_000), Step::AdvanceEpoch { epochs: 2 }],
        ),
        scenario("set_fee_too_high", "transfer_fee", vec![set_fee(10_001)]),
        scenario(
            "pause_without_pausable",
            "transfer_fee",
            vec![issuer(IssuerAction::Pause { slot: SLOT.into() })],
        ),
        scenario(
            "pause",
            "pausable",
            vec![issuer(IssuerAction::Pause { slot: SLOT.into() })],
        ),
        scenario(
            "pause_resume",
            "pausable",
            vec![
                issuer(IssuerAction::Pause { slot: SLOT.into() }),
                issuer(IssuerAction::Resume { slot: SLOT.into() }),
            ],
        ),
        scenario(
            "pause_all_without_pausable",
            "transfer_fee",
            vec![issuer(IssuerAction::PauseAll)],
        ),
        scenario(
            "pause_all",
            "pausable",
            vec![issuer(IssuerAction::PauseAll)],
        ),
    ];
    let matrix = Matrix {
        name: "issuer_steps".into(),
        description: String::new(),
        strategy: SlotStrategy::AllSlots,
        configs: vec![
            MintConfig::token_2022("fee_200bps", 6).with(MintExtension::TransferFee {
                basis_points: 200,
                maximum_fee: u64::MAX,
            }),
            MintConfig::token_2022("pausable", 6).with(MintExtension::Pausable),
        ],
    };
    let runner = Runner::new(RunnerOptions {
        seed: 7,
        keep_success_logs: false,
    });
    runner
        .run(&mut Vault { scenarios }, &matrix)
        .expect("runner")
}

fn find<'a>(report: &'a Report, id: &str) -> &'a RunResult {
    report.runs.iter().find(|r| r.id == id).unwrap_or_else(|| {
        panic!(
            "no run `{id}`; runs: {:?}",
            report.runs.iter().map(|r| &r.id).collect::<Vec<_>>()
        )
    })
}

fn refundability(r: &RunResult) -> (CheckStatus, &str) {
    let c = r
        .invariants
        .iter()
        .find(|c| c.invariant == InvariantKind::Refundability)
        .unwrap_or_else(|| panic!("{}: no refundability check", r.id));
    (c.status, c.detail.as_str())
}

fn alice_balance(r: &RunResult) -> u64 {
    r.balances
        .iter()
        .find(|b| b.label == "alice")
        .map(|b| b.actual_after)
        .unwrap_or_else(|| panic!("{}: no balance row for alice", r.id))
}

fn rule(r: &RunResult) -> Option<&str> {
    r.diagnosis.as_ref().map(|d| d.rule.as_str())
}

#[test]
fn issuer_and_epoch_steps() {
    let report = run();
    assert_eq!(report.runs.len(), 8, "{:?}", report.runs);

    // A new fee only applies two epochs later: without the epoch step both
    // transfers pay 2%, with it the withdrawal pays 10%. Refundability allows
    // the fee in effect at each transfer, so both hold.
    let now = find(&report, "fee_200bps/set_fee_now");
    assert_eq!(now.status, RunStatus::Pass, "{:#?}", now.invariants);
    assert_eq!(alice_balance(now), 100_000 - 2_000 - 1_960);
    assert_eq!(
        refundability(now).0,
        CheckStatus::Held,
        "{:?}",
        refundability(now)
    );

    let later = find(&report, "fee_200bps/set_fee_two_epochs");
    assert_eq!(later.status, RunStatus::Pass, "{:#?}", later.invariants);
    assert_eq!(alice_balance(later), 100_000 - 2_000 - 9_800);
    assert_eq!(
        refundability(later).0,
        CheckStatus::Held,
        "{:?}",
        refundability(later)
    );
    assert_eq!(
        later
            .steps
            .iter()
            .map(|s| s.step.describe())
            .collect::<Vec<_>>(),
        [
            "flow:deposit",
            "issuer:set_transfer_fee(token, 1000bps)",
            "clock:+2epoch",
            "flow:withdraw"
        ]
    );

    // An impossible fee is a harness error, not a protocol failure.
    let bad = find(&report, "fee_200bps/set_fee_too_high");
    assert_eq!(bad.status, RunStatus::Error);
    assert!(
        bad.harness_error.as_deref().unwrap_or("").contains("10001"),
        "{:?}",
        bad.harness_error
    );

    // Pause on a mint without Pausable sends nothing and succeeds.
    let noop = find(&report, "fee_200bps/pause_without_pausable");
    assert_eq!(noop.status, RunStatus::Pass, "{:#?}", noop.invariants);
    assert!(noop.steps[1].transactions.is_empty());

    // While paused the withdrawal fails and the deposit is stuck.
    let paused = find(&report, "pausable/pause");
    assert_eq!(paused.status, RunStatus::Fail);
    assert_eq!(refundability(paused).0, CheckStatus::Violated);
    let (_, tx) = paused.first_failed_tx().expect("failed tx");
    assert!(
        tx.logs.iter().any(|l| l.contains("paused on this mint")),
        "{:?}",
        tx.logs
    );
    assert_eq!(rule(paused), Some("pausable.paused_mint"));

    // After resume it works again.
    let resumed = find(&report, "pausable/pause_resume");
    assert_eq!(resumed.status, RunStatus::Pass, "{:#?}", resumed.invariants);
    assert_eq!(alice_balance(resumed), DEPOSIT);
    assert_eq!(rule(resumed), None);

    // PauseAll needs no slot: it pauses every Pausable mint and sends nothing
    // when there is none.
    let noop = find(&report, "fee_200bps/pause_all_without_pausable");
    assert_eq!(noop.status, RunStatus::Pass, "{:#?}", noop.invariants);
    assert!(noop.steps[1].transactions.is_empty());
    let all = find(&report, "pausable/pause_all");
    assert_eq!(all.steps[1].step.describe(), "issuer:pause_all");
    assert_eq!(all.steps[1].transactions.len(), 1);
    assert_eq!(all.status, RunStatus::Fail);
    assert_eq!(refundability(all).0, CheckStatus::Violated);
    assert_eq!(rule(all), Some("pausable.paused_mint"));
}
