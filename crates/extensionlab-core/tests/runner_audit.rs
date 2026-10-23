use extensionlab_core::{
    adapter::{
        ActorSpec, ActorTokenAccount, Adapter, ExpectedDelta, FlowKind, FlowSpec, FlowTx,
        SupportPolicy, TrackedAccount, World,
    },
    invariants::{CheckStatus, InvariantKind},
    matrix::{Case, IssuerAction, Matrix, Scenario, SlotStrategy, Step},
    mint::{MintConfig, MintExtension},
    report::{RunStatus, StepOutcome},
    runner::Runner,
    Error, Result,
};
use litesvm::LiteSVM;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use spl_associated_token_account_interface::instruction::create_associated_token_account_idempotent;
use spl_token_2022_interface::instruction::{close_account, transfer_checked};

#[derive(Clone, Copy)]
enum Mode {
    EmptyFlows,
    EmptyInstructions,
    MemoFlows,
    HalfTransfers,
    LaterRejectPolicy,
    LaterSupportPolicy,
    LateHarnessError,
    PartialSigningError,
    WrongMintSlot,
    DuplicateAddress,
    EmptyScenario,
    ClockOnlyScenario,
    DuplicateFlows,
    MissingProgram,
    SplitRoundedFees,
    SplitCappedFees,
    TwoExitCycles,
    ZeroFeeHarvest,
    NeutralCollateralSupport,
}
struct Vault {
    mode: Mode,
    exits: usize,
}

impl Vault {
    fn flow_tx(&self, payer: &str, instructions: Vec<Instruction>) -> FlowTx {
        FlowTx {
            payer: payer.into(),
            signers: vec![],
            extra_signers: vec![],
            instructions,
        }
    }
}
impl Adapter for Vault {
    fn name(&self) -> &str {
        "audit-vault"
    }
    fn mint_slots(&self) -> Vec<String> {
        if matches!(
            self.mode,
            Mode::LaterRejectPolicy
                | Mode::LaterSupportPolicy
                | Mode::WrongMintSlot
                | Mode::NeutralCollateralSupport
        ) {
            vec!["a".into(), "z".into()]
        } else {
            vec!["a".into()]
        }
    }
    fn actors(&self) -> Vec<ActorSpec> {
        let balance = if matches!(self.mode, Mode::SplitRoundedFees | Mode::SplitCappedFees) {
            200
        } else {
            100
        };
        [("alice", balance), ("vault", 0)]
            .into_iter()
            .map(|(name, initial_balance)| ActorSpec {
                name: name.into(),
                lamports: 1_000_000_000,
                token_accounts: self
                    .mint_slots()
                    .into_iter()
                    .map(|slot| ActorTokenAccount {
                        slot,
                        initial_balance,
                    })
                    .collect(),
            })
            .collect()
    }
    fn flows(&self) -> Vec<FlowSpec> {
        if matches!(self.mode, Mode::DuplicateFlows) {
            return vec![
                FlowSpec {
                    name: "deposit".into(),
                    kind: FlowKind::Entry,
                    description: String::new(),
                },
                FlowSpec {
                    name: "deposit".into(),
                    kind: FlowKind::Exit,
                    description: String::new(),
                },
            ];
        }
        [("deposit", FlowKind::Entry), ("withdraw", FlowKind::Exit)]
            .into_iter()
            .map(|(name, kind)| FlowSpec {
                name: name.into(),
                kind,
                description: String::new(),
            })
            .collect()
    }
    fn scenarios(&self) -> Vec<Scenario> {
        let steps = match self.mode {
            Mode::EmptyScenario => vec![],
            Mode::ClockOnlyScenario => vec![Step::AdvanceClock { seconds: 10 }],
            Mode::TwoExitCycles => vec![
                Step::flow("deposit"),
                Step::flow("withdraw"),
                Step::flow("deposit"),
                Step::flow("withdraw"),
            ],
            Mode::ZeroFeeHarvest | Mode::SplitRoundedFees => vec![
                Step::flow("deposit"),
                Step::Issuer {
                    action: IssuerAction::HarvestWithheld { slot: "a".into() },
                },
                Step::flow("withdraw"),
            ],
            _ => vec![Step::flow("deposit"), Step::flow("withdraw")],
        };
        vec![Scenario {
            name: "round_trip".into(),
            steps,
            requires_extension: None,
        }]
    }
    fn load_programs(&self, _svm: &mut LiteSVM) -> Result<()> {
        Ok(())
    }
    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        if matches!(self.mode, Mode::EmptyFlows) {
            return Ok(vec![]);
        }
        if matches!(self.mode, Mode::EmptyInstructions) {
            return Ok(vec![self.flow_tx("alice", vec![])]);
        }
        if matches!(self.mode, Mode::MissingProgram) {
            return Ok(vec![self.flow_tx(
                "alice",
                vec![Instruction {
                    program_id: Pubkey::new_from_array([99; 32]),
                    accounts: vec![],
                    data: vec![],
                }],
            )]);
        }
        if matches!(self.mode, Mode::SplitRoundedFees | Mode::SplitCappedFees) {
            let (from, to) = if flow == "deposit" {
                ("alice", "vault")
            } else {
                ("vault", "alice")
            };
            let mint = w.mint("a")?;
            let amount = if flow == "deposit" {
                100
            } else if matches!(self.mode, Mode::SplitRoundedFees) {
                198
            } else {
                194
            };
            let ix = transfer_checked(
                &mint.program_id(),
                &w.ata(from, "a")?,
                &mint.address,
                &w.ata(to, "a")?,
                &w.pubkey(from)?,
                &[],
                amount,
                mint.config.decimals,
            )
            .unwrap();
            let mut transactions = vec![self.flow_tx(from, vec![ix.clone()])];
            if flow == "deposit" {
                transactions.push(self.flow_tx(from, vec![ix]));
            }
            return Ok(transactions);
        }
        if matches!(self.mode, Mode::MemoFlows) {
            return Ok(vec![self.flow_tx(
                "alice",
                vec![Instruction {
                    program_id: Pubkey::from_str_const(
                        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
                    ),
                    accounts: vec![],
                    data: b"successful unrelated transaction".to_vec(),
                }],
            )]);
        }
        if matches!(self.mode, Mode::LaterSupportPolicy) {
            let mint = w.mint("a")?;
            let ix = transfer_checked(
                &mint.program_id(),
                &w.ata("alice", "a")?,
                &mint.address,
                &w.ata("vault", "a")?,
                &w.pubkey("alice")?,
                &[],
                101,
                mint.config.decimals,
            )
            .unwrap();
            return Ok(vec![self.flow_tx("alice", vec![ix])]);
        }
        if matches!(self.mode, Mode::NeutralCollateralSupport) {
            let mint = w.mint("z")?;
            let ix = transfer_checked(
                &mint.program_id(),
                &w.ata("alice", "z")?,
                &mint.address,
                &w.ata("vault", "z")?,
                &w.pubkey("alice")?,
                &[],
                100,
                mint.config.decimals,
            )
            .unwrap();
            return Ok(vec![self.flow_tx("alice", vec![ix])]);
        }
        let (from, to) = if flow == "deposit" {
            ("alice", "vault")
        } else {
            ("vault", "alice")
        };
        let amount = if matches!(self.mode, Mode::HalfTransfers) {
            50
        } else {
            100
        };
        let mint = w.mint("a")?;
        let ix = transfer_checked(
            &mint.program_id(),
            &w.ata(from, "a")?,
            &mint.address,
            &w.ata(to, "a")?,
            &w.pubkey(from)?,
            &[],
            amount,
            mint.config.decimals,
        )
        .unwrap();
        if matches!(self.mode, Mode::PartialSigningError) && flow == "deposit" {
            return Ok(vec![
                self.flow_tx(from, vec![ix.clone()]),
                self.flow_tx("unknown-actor", vec![ix]),
            ]);
        }
        if matches!(self.mode, Mode::TwoExitCycles) {
            let mut instructions = vec![];
            if flow == "deposit" {
                instructions.push(create_associated_token_account_idempotent(
                    &w.pubkey("alice")?,
                    &w.pubkey("vault")?,
                    &mint.address,
                    &mint.program_id(),
                ));
            }
            instructions.push(ix);
            if flow == "withdraw" {
                if self.exits == 0 {
                    instructions.push(
                        close_account(
                            &mint.program_id(),
                            &w.ata("vault", "a")?,
                            &w.pubkey("alice")?,
                            &w.pubkey("vault")?,
                            &[],
                        )
                        .unwrap(),
                    );
                }
                self.exits += 1;
            }
            return Ok(vec![self.flow_tx(from, instructions)]);
        }
        Ok(vec![self.flow_tx(from, vec![ix])])
    }
    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        let slot = if matches!(self.mode, Mode::WrongMintSlot) {
            "z"
        } else {
            "a"
        };
        let mut tracked: Vec<_> = [("alice", false), ("vault", true)]
            .into_iter()
            .map(|(actor, custody)| TrackedAccount {
                label: actor.into(),
                address: w.ata(actor, "a").unwrap(),
                slot: slot.into(),
                custody,
            })
            .collect();
        if matches!(self.mode, Mode::DuplicateAddress) {
            tracked.push(TrackedAccount {
                label: "alias".into(),
                address: w.ata("alice", "a")?,
                slot: "a".into(),
                custody: false,
            });
        }
        Ok(tracked)
    }
    fn expected_deltas(&self, flow: &str, _w: &World) -> Result<Vec<ExpectedDelta>> {
        if matches!(self.mode, Mode::LateHarnessError) && flow == "withdraw" {
            return Err(Error::Adapter("late expected-delta reader failed".into()));
        }
        if matches!(self.mode, Mode::SplitRoundedFees | Mode::SplitCappedFees) {
            let received = if matches!(self.mode, Mode::SplitRoundedFees) {
                198
            } else {
                194
            };
            let returned = if matches!(self.mode, Mode::SplitRoundedFees) {
                197
            } else {
                191
            };
            return Ok(if flow == "deposit" {
                vec![
                    ExpectedDelta {
                        label: "alice".into(),
                        delta: -200,
                    },
                    ExpectedDelta {
                        label: "vault".into(),
                        delta: received,
                    },
                ]
            } else {
                vec![
                    ExpectedDelta {
                        label: "alice".into(),
                        delta: returned,
                    },
                    ExpectedDelta {
                        label: "vault".into(),
                        delta: -received,
                    },
                ]
            });
        }
        let delta = if flow == "deposit" { 100 } else { -100 };
        Ok(vec![
            ExpectedDelta {
                label: "alice".into(),
                delta: -delta,
            },
            ExpectedDelta {
                label: "vault".into(),
                delta,
            },
        ])
    }
    fn support_policy(&self, slot: &str, config: &MintConfig) -> SupportPolicy {
        if matches!(self.mode, Mode::LaterRejectPolicy) && slot == "z" {
            SupportPolicy::MustReject
        } else if (matches!(self.mode, Mode::LaterSupportPolicy) && slot == "z")
            || (matches!(self.mode, Mode::NeutralCollateralSupport)
                && slot == "a"
                && config.is_neutral())
        {
            SupportPolicy::MustSupport
        } else {
            SupportPolicy::Unspecified
        }
    }
    fn closable_accounts(&self, _world: &World) -> Result<Vec<String>> {
        Ok(if matches!(self.mode, Mode::TwoExitCycles) {
            vec!["vault".into()]
        } else {
            vec![]
        })
    }
}

fn run(mode: Mode) -> extensionlab_core::report::RunResult {
    let config = if matches!(mode, Mode::LaterRejectPolicy | Mode::LaterSupportPolicy) {
        MintConfig::token_2022("pd", 6).with(MintExtension::PermanentDelegate)
    } else if matches!(
        mode,
        Mode::SplitRoundedFees | Mode::SplitCappedFees | Mode::ZeroFeeHarvest
    ) {
        MintConfig::token_2022("fee", 6).with(MintExtension::TransferFee {
            basis_points: if matches!(mode, Mode::ZeroFeeHarvest) {
                0
            } else if matches!(mode, Mode::SplitRoundedFees) {
                1
            } else {
                1_000
            },
            maximum_fee: if matches!(mode, Mode::SplitRoundedFees) {
                u64::MAX
            } else {
                3
            },
        })
    } else {
        MintConfig::token_2022("plain", 6)
    };
    let matrix = Matrix {
        name: "audit".into(),
        description: String::new(),
        strategy: SlotStrategy::AllSlots,
        configs: vec![config],
    };
    Runner::default()
        .run(&mut Vault { mode, exits: 0 }, &matrix)
        .unwrap()
        .runs
        .remove(0)
}

#[test]
fn harvest_requires_actual_withheld_tokens_and_does_not_skip_later_flows() {
    let empty = run(Mode::ZeroFeeHarvest);
    assert_eq!(empty.status, RunStatus::NotApplicable, "{empty:#?}");
    assert_eq!(empty.steps[1].outcome, StepOutcome::NotApplicable);
    assert!(empty.steps[1].transactions.is_empty());
    assert_eq!(empty.steps[2].outcome, StepOutcome::Succeeded);
    let funded = run(Mode::SplitRoundedFees);
    assert_eq!(funded.status, RunStatus::Pass, "{funded:#?}");
    assert_eq!(funded.steps[1].outcome, StepOutcome::Succeeded);
    assert_eq!(funded.steps[1].transactions.len(), 1);
    assert_eq!(funded.steps[2].outcome, StepOutcome::Succeeded);
}

#[test]
fn per_flow_expectations_cannot_cancel_across_a_round_trip() {
    for mode in [Mode::MemoFlows, Mode::HalfTransfers] {
        let run = run(mode);
        assert_eq!(run.status, RunStatus::Fail, "{run:#?}");
        assert!(run
            .steps
            .iter()
            .all(|s| s.outcome == StepOutcome::Succeeded));
        let checks: Vec<_> = run
            .invariants
            .iter()
            .filter(|c| c.invariant == InvariantKind::ExpectedBalances)
            .collect();
        assert!(checks
            .iter()
            .any(|c| c.step_index.is_none() && c.status == CheckStatus::Held));
        for step in [0, 1] {
            assert!(
                checks
                    .iter()
                    .any(|c| c.step_index == Some(step) && c.status == CheckStatus::Violated),
                "{checks:?}"
            );
        }
    }
}

#[test]
fn empty_or_clock_only_work_is_a_harness_error() {
    for mode in [
        Mode::EmptyFlows,
        Mode::EmptyInstructions,
        Mode::EmptyScenario,
        Mode::ClockOnlyScenario,
    ] {
        let run = run(mode);
        assert_eq!(run.status, RunStatus::Error, "{run:#?}");
        assert!(run.steps.iter().all(|s| s.transactions.is_empty()));
    }
}

#[test]
fn every_slot_contributes_to_the_support_policy() {
    let rejected = run(Mode::LaterRejectPolicy);
    assert_eq!(rejected.status, RunStatus::Fail, "{rejected:#?}");
    assert!(rejected
        .steps
        .iter()
        .all(|s| s.outcome == StepOutcome::Succeeded));
    let supported = run(Mode::LaterSupportPolicy);
    assert_eq!(supported.status, RunStatus::Fail, "{supported:#?}");
    assert_eq!(supported.steps[0].outcome, StepOutcome::Failed);
}

#[test]
fn tracked_account_identity_and_flow_declarations_are_validated() {
    for mode in [
        Mode::WrongMintSlot,
        Mode::DuplicateAddress,
        Mode::DuplicateFlows,
    ] {
        let run = run(mode);
        assert_eq!(run.status, RunStatus::Error, "{run:#?}");
        assert!(run.steps.iter().all(|s| s.transactions.is_empty()));
    }
}

#[test]
fn late_harness_errors_preserve_committed_balance_evidence() {
    for mode in [Mode::LateHarnessError, Mode::PartialSigningError] {
        let run = run(mode);
        assert_eq!(run.status, RunStatus::Error, "{run:#?}");
        assert_eq!(run.steps[0].transactions.len(), 1);
        assert!(run.steps[0].transactions[0].error.is_none());
        let vault = run.balances.iter().find(|b| b.label == "vault").unwrap();
        assert_eq!(vault.before, 0);
        assert_eq!(vault.actual_after, 100);
        let alice = run.balances.iter().find(|b| b.label == "alice").unwrap();
        assert_eq!(alice.actual_after, 0);
        assert!(run
            .harness_error
            .as_deref()
            .unwrap()
            .contains("last readable snapshot"));
        assert!(run
            .invariants
            .iter()
            .any(|c| c.invariant == InvariantKind::Conservation && c.status == CheckStatus::Held));
    }
}

#[test]
fn direct_cases_must_assign_every_declared_slot() {
    let mut vault = Vault {
        mode: Mode::LaterRejectPolicy,
        exits: 0,
    };
    let case = Case {
        id: "incomplete".into(),
        slots: [("a".into(), MintConfig::token_2022("plain", 6))].into(),
        varied_slot: None,
        scenario: vault.scenarios().remove(0),
    };
    let run = Runner::default().run_case(&mut vault, &case).unwrap();
    assert_eq!(run.status, RunStatus::Error);
    assert!(run.harness_error.as_deref().unwrap().contains("exactly"));
}

#[test]
fn neutral_collateral_support_does_not_promise_the_varied_extension() {
    let mut vault = Vault {
        mode: Mode::NeutralCollateralSupport,
        exits: 0,
    };
    let mut case = Case {
        id: "non_transferable@z/round_trip".into(),
        slots: [
            ("a".into(), MintConfig::token_2022("plain", 6)),
            (
                "z".into(),
                MintConfig::token_2022("non_transferable", 6).with(MintExtension::NonTransferable),
            ),
        ]
        .into(),
        varied_slot: Some("z".into()),
        scenario: vault.scenarios().remove(0),
    };
    let run = Runner::default().run_case(&mut vault, &case).unwrap();
    assert_eq!(run.status, RunStatus::Rejected, "{run:#?}");
    assert_eq!(run.steps[0].outcome, StepOutcome::Failed);
    case.varied_slot = Some("undeclared".into());
    let invalid = Runner::default().run_case(&mut vault, &case).unwrap();
    assert_eq!(invalid.status, RunStatus::Error);
    assert!(invalid
        .harness_error
        .as_deref()
        .unwrap()
        .contains("varied slot"));
}

#[test]
fn missing_program_is_a_harness_error_with_the_failed_transaction_retained() {
    let run = run(Mode::MissingProgram);
    assert_eq!(run.status, RunStatus::Error, "{run:#?}");
    assert_eq!(run.steps[0].outcome, StepOutcome::Failed);
    assert_eq!(run.steps[0].transactions.len(), 1);
    let error = run.steps[0].transactions[0].error.as_deref().unwrap();
    assert!(
        error.contains("ProgramAccountNotFound") || error.contains("InvalidProgramForExecution"),
        "{error}"
    );
    assert!(run
        .harness_error
        .as_deref()
        .unwrap()
        .contains("could not execute"));
    assert_eq!(run.steps[1].outcome, StepOutcome::Skipped);
}

#[test]
fn refundability_counts_fee_rounding_and_caps_for_each_transfer() {
    for (mode, returned) in [(Mode::SplitRoundedFees, 197), (Mode::SplitCappedFees, 191)] {
        let run = run(mode);
        assert_eq!(run.status, RunStatus::Pass, "{run:#?}");
        assert_eq!(run.steps[0].transactions.len(), 2);
        assert_eq!(
            run.balances
                .iter()
                .find(|b| b.label == "alice")
                .unwrap()
                .actual_after,
            returned
        );
        let refund = run
            .invariants
            .iter()
            .find(|c| c.invariant == InvariantKind::Refundability)
            .unwrap();
        assert_eq!(refund.status, CheckStatus::Held, "{refund:?}");
    }
}

#[test]
fn every_exit_checks_closeability_at_its_own_snapshot() {
    let run = run(Mode::TwoExitCycles);
    assert_eq!(run.status, RunStatus::Fail, "{run:#?}");
    assert!(run
        .steps
        .iter()
        .all(|step| step.outcome == StepOutcome::Succeeded));
    let closed = run
        .invariants
        .iter()
        .filter(|c| c.invariant == InvariantKind::Closeability)
        .collect::<Vec<_>>();
    assert_eq!(closed.len(), 2);
    assert!(closed
        .iter()
        .any(|c| c.step_index == Some(1) && c.status == CheckStatus::Held));
    assert!(closed
        .iter()
        .any(|c| c.step_index == Some(3) && c.status == CheckStatus::Violated));
}
