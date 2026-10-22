//! Runner: executes adapter x matrix and produces a [`Report`].
//!
//! [`Runner::plan`] expands the matrix into cases, [`Runner::run_case`]
//! executes one case in a fresh LiteSVM instance, and [`Runner::run`] runs
//! every case and assembles the report. The lifecycle is documented on
//! [`Runner::run_case`] and in `research/04_architecture.md`, section 7.

use crate::{
    adapter::{Adapter, ExpectedDelta, FlowKind, FlowTx, SupportPolicy, TrackedAccount, World},
    diagnosis,
    invariants::{
        self, AccountSnapshot, CheckStatus, ExitAttempt, InvariantCheck, InvariantKind,
        MintSnapshot, Snapshot,
    },
    matrix::{expand, Case, IssuerAction, Matrix, Step},
    mint::{self, ProvisionedMint},
    report::{
        self, AdapterInfo, Environment, ProgramInfo, Report, RunResult, RunStatus, StepOutcome,
        StepRecord, TxRecord,
    },
    Error, Result, LITESVM_VERSION,
};
use litesvm::LiteSVM;
use solana_clock::Clock;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::instruction::create_associated_token_account;
use spl_token_2022_interface::{
    extension::{
        pausable,
        transfer_fee::{
            instruction::{harvest_withheld_tokens_to_mint, set_transfer_fee},
            MAX_FEE_BASIS_POINTS,
        },
    },
    instruction::{freeze_account, thaw_account, transfer_checked},
};
use std::collections::{BTreeMap, BTreeSet};

/// Lamports airdropped to the issuer, which pays for mints and every account
/// the harness creates.
const ISSUER_LAMPORTS: u64 = 100_000_000_000;

/// A transaction of a step, signed only when it is about to be sent.
enum PendingTx {
    /// From an adapter flow; signers are named actors.
    Flow(FlowTx),
    /// Issuer action instructions; the issuer pays and signs.
    Issuer(Vec<Instruction>),
}

/// Runner settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunnerOptions {
    /// Seed for deterministic keypairs. Issuer, actor and mint keypairs are
    /// derived from `(seed, case id, name)` (see [`derive_keypair`]), so
    /// addresses are the same on every run and reports diff cleanly.
    pub seed: u64,
    /// Keep program logs of successful transactions too (failed transactions
    /// always keep their logs).
    pub keep_success_logs: bool,
}

/// Executes cases.
#[derive(Debug, Clone, Default)]
pub struct Runner {
    /// Settings.
    pub options: RunnerOptions,
}

impl Runner {
    /// Runner with explicit options.
    pub fn new(options: RunnerOptions) -> Self {
        Self { options }
    }

    /// Expand `matrix` against the adapter's slots and scenarios.
    pub fn plan(&self, adapter: &dyn Adapter, matrix: &Matrix) -> Result<Vec<Case>> {
        expand(matrix, &adapter.mint_slots(), &adapter.scenarios())
    }

    /// Run every case and assemble a report. A harness error in one case
    /// becomes an `error` run; it does not abort the other cases.
    pub fn run(&self, adapter: &mut dyn Adapter, matrix: &Matrix) -> Result<Report> {
        let cases = self.plan(adapter, matrix)?;
        let mut runs = Vec::with_capacity(cases.len());
        for case in &cases {
            runs.push(self.run_case(adapter, case)?);
        }
        let mut programs = bundled_token_programs();
        programs.extend(adapter.programs());
        Ok(Report::new(
            Environment {
                litesvm: LITESVM_VERSION.into(),
                programs,
            },
            AdapterInfo {
                name: adapter.name().to_string(),
                description: adapter.description().to_string(),
            },
            &matrix.name,
            matrix.strategy,
            runs,
        ))
    }

    /// Execute one case in a fresh LiteSVM instance.
    ///
    /// Lifecycle:
    /// 1. `LiteSVM::new()`; `adapter.load_programs(&mut svm)`.
    /// 2. Derive issuer, actor and mint keypairs from the seed; airdrop SOL.
    /// 3. For each slot: `mint::create_mint` with the case's config, and an
    ///    issuer token account (the drain destination, thawed if needed).
    /// 4. For each actor token account: `mint::create_actor_token_account`
    ///    and `mint::mint_tokens` with the initial balance.
    /// 5. `adapter.setup(&world)`; send its transactions.
    /// 6. `adapter.tracked_accounts(&world)` plus the issuer's token
    ///    accounts (labelled `issuer[<slot>]`); take the initial snapshot.
    /// 7. For each step: build (flow) or perform (issuer action, clock), send
    ///    each transaction, record signature, compute units, fee, error and
    ///    logs; snapshot after the step; stop at the first failed transaction
    ///    and mark the remaining steps skipped.
    /// 8. Invariants: conservation over the whole scenario; expected balances
    ///    over the flows that ran (when no transaction was committed the run
    ///    was cleanly rejected and no balance may change);
    ///    credited-equals-received around each successful flow;
    ///    refundability for each entry/exit pair; closeability after the
    ///    first successful exit.
    /// 9. `report::classify`, then `diagnosis::diagnose`.
    ///
    /// A harness problem (setup failure, adapter error, unreadable account)
    /// does not abort: the run is returned with status `error`, the steps
    /// recorded so far, and `harness_error` set.
    pub fn run_case(&self, adapter: &mut dyn Adapter, case: &Case) -> Result<RunResult> {
        let mut run = RunResult {
            id: case.id.clone(),
            slots: case.slots.clone(),
            varied_slot: case.varied_slot.clone(),
            baseline: case.is_baseline(),
            scenario: case.scenario.clone(),
            status: RunStatus::Error,
            steps: Vec::new(),
            balances: Vec::new(),
            invariants: Vec::new(),
            diagnosis: None,
            harness_error: None,
        };
        if let Err(e) = self.execute(adapter, case, &mut run) {
            for (index, step) in case.scenario.steps.iter().enumerate().skip(run.steps.len()) {
                run.steps.push(StepRecord {
                    index,
                    step: step.clone(),
                    outcome: StepOutcome::Skipped,
                    transactions: Vec::new(),
                });
            }
            run.status = RunStatus::Error;
            run.harness_error = Some(e.to_string());
            run.balances.clear();
            run.invariants.clear();
            run.diagnosis = None;
        }
        Ok(run)
    }

    fn execute(&self, adapter: &mut dyn Adapter, case: &Case, run: &mut RunResult) -> Result<()> {
        let key = |name: &str| derive_keypair(self.options.seed, &case.id, name);

        // 1-2. VM, programs, issuer.
        let mut svm = LiteSVM::new();
        adapter.load_programs(&mut svm)?;
        let issuer = key("issuer");
        let issuer_pk = issuer.pubkey();
        airdrop(&mut svm, &issuer_pk, ISSUER_LAMPORTS)?;
        let mut world = World::new(svm, issuer);
        world.label(issuer_pk, "issuer");

        // 3. Mints and issuer accounts.
        let declared: BTreeSet<String> = adapter.mint_slots().into_iter().collect();
        for (slot, config) in &case.slots {
            if !declared.contains(slot) {
                return Err(Error::Adapter(format!(
                    "case assigns undeclared mint slot `{slot}`"
                )));
            }
            let mint_kp = key(&format!("mint:{slot}"));
            let provisioned =
                mint::create_mint(&mut world.svm, config, &world.issuer, &mint_kp, &issuer_pk)?;
            create_issuer_account(&mut world.svm, &provisioned, &world.issuer)?;
            world.add_mint(slot.clone(), provisioned);
        }

        // 4. Actors and their funded token accounts.
        for spec in adapter.actors() {
            let kp = key(&format!("actor:{}", spec.name));
            airdrop(&mut world.svm, &kp.pubkey(), spec.lamports)?;
            for account in &spec.token_accounts {
                let provisioned = world.mint(&account.slot)?.clone();
                let ata = mint::create_actor_token_account(
                    &mut world.svm,
                    &provisioned,
                    &world.issuer,
                    &kp,
                    &world.issuer,
                )?;
                if account.initial_balance > 0 {
                    mint::mint_tokens(
                        &mut world.svm,
                        &provisioned,
                        &world.issuer,
                        &world.issuer,
                        &ata,
                        account.initial_balance,
                    )?;
                }
                world.label(ata, format!("{}[{}]", spec.name, account.slot));
            }
            world.add_actor(spec.name.clone(), kp);
        }

        // 5. Protocol setup.
        for tx in adapter.setup(&world)? {
            let signed = sign_flow_tx(&world, &tx)?;
            let record = self.submit(&mut world.svm, signed);
            if let Some(err) = record.error {
                return Err(Error::SetupTransaction {
                    what: "adapter setup".into(),
                    err,
                    logs: record.logs.join("\n"),
                });
            }
        }

        // 6. Tracked accounts and the initial snapshot.
        let mut tracked = adapter.tracked_accounts(&world)?;
        for (slot, provisioned) in world.mints() {
            tracked.push(TrackedAccount {
                label: issuer_account_label(slot),
                address: provisioned.ata(&issuer_pk),
                slot: slot.clone(),
                custody: false,
            });
        }
        let mut labels = BTreeSet::new();
        for t in &tracked {
            if !labels.insert(t.label.as_str()) {
                return Err(Error::Adapter(format!(
                    "tracked account label `{}` is used twice",
                    t.label
                )));
            }
            if !case.slots.contains_key(&t.slot) {
                return Err(Error::Adapter(format!(
                    "tracked account `{}` is in unknown mint slot `{}`",
                    t.label, t.slot
                )));
            }
        }
        for t in &tracked {
            world.label(t.address, t.label.clone());
        }
        let flow_kinds: BTreeMap<String, FlowKind> = adapter
            .flows()
            .into_iter()
            .map(|f| (f.name, f.kind))
            .collect();

        // 7. Steps. Steps run in order until the first failure, so the
        // executed steps are a prefix and `snaps[i + 1]` is the snapshot
        // after step `i` for every executed step.
        let mut snaps = vec![snapshot(&world, &tracked)?];
        // Transfer fee in effect per slot at each snapshot, parallel to
        // `snaps`. A step's fee is the one at the snapshot after it: flows do
        // not change the epoch, and an epoch step moves no tokens.
        let mut fees = vec![fee_rates(&world)?];
        let mut credit_checks = Vec::new();
        let mut flows_succeeded = 0usize;
        let mut stopped = false;
        for (index, step) in case.scenario.steps.iter().enumerate() {
            if stopped {
                run.steps.push(StepRecord {
                    index,
                    step: step.clone(),
                    outcome: StepOutcome::Skipped,
                    transactions: Vec::new(),
                });
                continue;
            }
            let (pending, credits_before) = match step {
                Step::Flow { name } => {
                    if !flow_kinds.contains_key(name) {
                        return Err(Error::Adapter(format!(
                            "scenario uses unknown flow `{name}`"
                        )));
                    }
                    let credits = adapter.credits(&world)?;
                    let txs = adapter.build_flow(name, &world)?;
                    (
                        txs.into_iter().map(PendingTx::Flow).collect::<Vec<_>>(),
                        Some(credits),
                    )
                }
                Step::Issuer { action } => {
                    let ix_sets = issuer_instructions(&world, &tracked, action)?;
                    (ix_sets.into_iter().map(PendingTx::Issuer).collect(), None)
                }
                Step::AdvanceClock { seconds } => {
                    let mut clock: Clock = world.svm.get_sysvar();
                    clock.unix_timestamp = clock.unix_timestamp.saturating_add(*seconds);
                    world.svm.set_sysvar(&clock);
                    (Vec::new(), None)
                }
                Step::AdvanceEpoch { epochs } => {
                    let mut clock: Clock = world.svm.get_sysvar();
                    clock.epoch = clock.epoch.saturating_add(*epochs);
                    clock.leader_schedule_epoch =
                        clock.leader_schedule_epoch.saturating_add(*epochs);
                    world.svm.set_sysvar(&clock);
                    (Vec::new(), None)
                }
            };
            let mut record = StepRecord {
                index,
                step: step.clone(),
                outcome: StepOutcome::Succeeded,
                transactions: Vec::new(),
            };
            for pending_tx in pending {
                // Sign each transaction just before sending it: `submit`
                // expires the blockhash after every send, so a transaction
                // signed earlier in the step would be rejected as stale.
                let signed = match &pending_tx {
                    PendingTx::Flow(tx) => sign_flow_tx(&world, tx),
                    PendingTx::Issuer(ixs) => {
                        sign(&world.svm, &world.issuer, ixs, &[&world.issuer])
                    }
                };
                let tx = match signed {
                    Ok(tx) => tx,
                    Err(e) => {
                        if !record.transactions.is_empty() {
                            record.outcome = StepOutcome::Failed;
                            run.steps.push(record);
                        }
                        return Err(e);
                    }
                };
                let tx_record = self.submit(&mut world.svm, tx);
                let failed = tx_record.error.is_some();
                record.transactions.push(tx_record);
                if failed {
                    record.outcome = StepOutcome::Failed;
                    break;
                }
            }
            let succeeded = record.outcome == StepOutcome::Succeeded;
            stopped = !succeeded;
            run.steps.push(record);
            let after = snapshot(&world, &tracked)?;
            if let (Some(before), true) = (credits_before, succeeded) {
                flows_succeeded += 1;
                let credits_after = adapter.credits(&world)?;
                let prev = snaps.last().expect("initial snapshot");
                credit_checks.push(invariants::credited_equals_received(
                    &before,
                    &credits_after,
                    prev,
                    &after,
                ));
            }
            snaps.push(after);
            fees.push(fee_rates(&world)?);
        }

        // 8. Invariants.
        let initial = &snaps[0];
        let last = snaps.last().expect("initial snapshot");
        let mut checks = invariants::conservation(initial, last);

        // Expected deltas of the flows that succeeded, plus the failed flow
        // when some transaction was committed before its failure (its unmet
        // expectation is what stayed stuck). When nothing was committed the
        // run was cleanly rejected, and every account the flows name is
        // expected not to change. Skipped flows never count.
        let mut expected: Vec<ExpectedDelta> = Vec::new();
        let mut declared: BTreeSet<String> = BTreeSet::new();
        let mut committed = false;
        for (step, record) in case.scenario.steps.iter().zip(&run.steps) {
            let step_committed = record.transactions.iter().any(|t| t.error.is_none());
            let counts = match record.outcome {
                StepOutcome::Succeeded => true,
                StepOutcome::Failed => committed || step_committed,
                StepOutcome::Skipped => false,
            };
            committed |= step_committed;
            if let Step::Flow { name } = step {
                let deltas = adapter.expected_deltas(name, &world)?;
                declared.extend(deltas.iter().map(|d| d.label.clone()));
                if counts {
                    expected.extend(deltas);
                }
            }
        }
        let nothing_counted = expected.is_empty() && !declared.is_empty();
        if nothing_counted {
            expected = declared
                .iter()
                .map(|label| ExpectedDelta {
                    label: label.clone(),
                    delta: 0,
                })
                .collect();
        }
        let (rows, mut balance_check) = invariants::expected_balances(initial, last, &expected)?;
        if nothing_counted {
            balance_check.detail = format!(
                "no flow committed a transaction, so no balance should change; {}",
                balance_check.detail
            );
        }
        checks.push(balance_check);

        if flows_succeeded == 0 {
            checks.push(check(
                InvariantKind::CreditedEqualsReceived,
                None,
                CheckStatus::Skipped,
                "no flow succeeded".into(),
            ));
        } else if credit_checks
            .iter()
            .all(|c| c.status == CheckStatus::Skipped)
        {
            checks.push(invariants::credited_equals_received(
                &[],
                &[],
                initial,
                last,
            ));
        } else {
            checks.extend(
                credit_checks
                    .into_iter()
                    .filter(|c| c.status != CheckStatus::Skipped),
            );
        }

        checks.extend(refundability_checks(
            case,
            &run.steps,
            &snaps,
            &fees,
            &tracked,
            &flow_kinds,
        ));

        let closable = adapter.closable_accounts(&world)?;
        let first_exit = case.scenario.steps.iter().enumerate().position(|(i, s)| {
            is_flow_of_kind(s, FlowKind::Exit, &flow_kinds)
                && run.steps[i].outcome == StepOutcome::Succeeded
        });
        checks.push(match first_exit {
            Some(i) => invariants::closeability(&closable, &snaps[i + 1]),
            None if closable.is_empty() => invariants::closeability(&closable, last),
            None => check(
                InvariantKind::Closeability,
                None,
                CheckStatus::Skipped,
                "no exit flow succeeded".into(),
            ),
        });

        // 9. Status and diagnosis.
        let policy = support_policy(adapter, case);
        run.status = report::classify(&run.steps, &checks, policy);
        run.balances = rows;
        run.invariants = checks;
        run.diagnosis = diagnosis::diagnose(run);
        Ok(())
    }

    /// Send a signed transaction and record it. Expires the blockhash
    /// afterwards so that a later, byte-identical transaction is not rejected
    /// as already processed.
    fn submit(&self, svm: &mut LiteSVM, tx: Transaction) -> TxRecord {
        let signature = tx
            .signatures
            .first()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let result = svm.send_transaction(tx);
        svm.expire_blockhash();
        match result {
            Ok(meta) => TxRecord {
                signature,
                compute_units: meta.compute_units_consumed,
                fee_lamports: meta.fee,
                error: None,
                logs: if self.options.keep_success_logs {
                    meta.logs
                } else {
                    Vec::new()
                },
            },
            Err(failed) => TxRecord {
                signature,
                compute_units: failed.meta.compute_units_consumed,
                fee_lamports: failed.meta.fee,
                error: Some(format!("{:?}", failed.err)),
                logs: failed.meta.logs,
            },
        }
    }
}

/// Deterministic keypair for `name` in case `case_id`: the ed25519 secret is
/// the SHA-256 of a domain tag, the seed, and the length-prefixed case id and
/// name.
pub fn derive_keypair(seed: u64, case_id: &str, name: &str) -> Keypair {
    let hash = solana_sha256_hasher::hashv(&[
        b"extensionlab/keypair/v1",
        &seed.to_le_bytes(),
        &(case_id.len() as u64).to_le_bytes(),
        case_id.as_bytes(),
        &(name.len() as u64).to_le_bytes(),
        name.as_bytes(),
    ]);
    Keypair::new_from_array(hash.to_bytes())
}

/// Label of the issuer's token account for `slot`.
pub fn issuer_account_label(slot: &str) -> String {
    format!("issuer[{slot}]")
}

fn check(
    invariant: InvariantKind,
    slot: Option<String>,
    status: CheckStatus,
    detail: String,
) -> InvariantCheck {
    InvariantCheck {
        invariant,
        slot,
        status,
        detail,
    }
}

fn airdrop(svm: &mut LiteSVM, to: &Pubkey, lamports: u64) -> Result<()> {
    svm.airdrop(to, lamports)
        .map(|_| ())
        .map_err(|failed| Error::SetupTransaction {
            what: format!("airdrop to {to}"),
            err: format!("{:?}", failed.err),
            logs: failed.meta.logs.join("\n"),
        })
}

/// The issuer's associated token account for a mint, without the actor
/// account extensions (an issuer account that requires memos would make every
/// drain fail for a reason unrelated to the protocol). Thawed when the mint
/// freezes new accounts.
fn create_issuer_account(
    svm: &mut LiteSVM,
    provisioned: &ProvisionedMint,
    issuer: &Keypair,
) -> Result<Pubkey> {
    let program = provisioned.program_id();
    let issuer_pk = issuer.pubkey();
    let ata = provisioned.ata(&issuer_pk);
    let mut ixs = vec![create_associated_token_account(
        &issuer_pk,
        &issuer_pk,
        &provisioned.address,
        &program,
    )];
    if provisioned.config.has("default_account_state_frozen") {
        ixs.push(
            thaw_account(&program, &ata, &provisioned.address, &issuer_pk, &[])
                .map_err(|e| Error::Instruction(format!("thaw_account: {e}")))?,
        );
    }
    mint::send_setup(
        svm,
        &format!("create issuer account for `{}`", provisioned.config.label),
        issuer,
        &ixs,
        &[issuer],
    )?;
    Ok(ata)
}

/// Sign `ixs` with `payer` as fee payer, using those `candidates` the message
/// requires.
fn sign(
    svm: &LiteSVM,
    payer: &Keypair,
    ixs: &[Instruction],
    candidates: &[&Keypair],
) -> Result<Transaction> {
    let message = Message::new(ixs, Some(&payer.pubkey()));
    let required = &message.account_keys[..message.header.num_required_signatures as usize];
    let mut signers: Vec<&Keypair> = Vec::new();
    for kp in std::iter::once(payer).chain(candidates.iter().copied()) {
        let pk = kp.pubkey();
        if required.contains(&pk) && !signers.iter().any(|s| s.pubkey() == pk) {
            signers.push(kp);
        }
    }
    let mut tx = Transaction::new_unsigned(message);
    tx.try_sign(&signers, svm.latest_blockhash())
        .map_err(|e| Error::Adapter(format!("signing failed (missing signer?): {e}")))?;
    Ok(tx)
}

fn sign_flow_tx(world: &World, tx: &FlowTx) -> Result<Transaction> {
    let payer = world.actor(&tx.payer)?;
    let mut candidates: Vec<&Keypair> = Vec::new();
    for name in &tx.signers {
        candidates.push(world.actor(name)?);
    }
    candidates.extend(tx.extra_signers.iter());
    sign(&world.svm, payer, &tx.instructions, &candidates)
}

/// Instructions for an issuer action, one inner vector per transaction.
/// Accounts that do not exist (or hold nothing to move) are left out; an
/// action with nothing to do produces no transaction and succeeds.
fn issuer_instructions(
    world: &World,
    tracked: &[TrackedAccount],
    action: &IssuerAction,
) -> Result<Vec<Vec<Instruction>>> {
    let issuer_pk = world.issuer.pubkey();
    let ix_err = |what: &str, e: &dyn std::fmt::Display| Error::Instruction(format!("{what}: {e}"));
    let mut out = Vec::new();
    match action {
        IssuerAction::DrainCustody => {
            for t in tracked.iter().filter(|t| t.custody) {
                let provisioned = world.mint(&t.slot)?;
                if !provisioned.config.has("permanent_delegate") {
                    continue;
                }
                let Some(view) = mint::read_token_account(&world.svm, &t.address)? else {
                    continue;
                };
                if view.amount == 0 {
                    continue;
                }
                let ix = transfer_checked(
                    &provisioned.program_id(),
                    &t.address,
                    &provisioned.address,
                    &provisioned.ata(&issuer_pk),
                    &issuer_pk,
                    &[],
                    view.amount,
                    provisioned.config.decimals,
                )
                .map_err(|e| ix_err("transfer_checked (permanent delegate)", &e))?;
                out.push(vec![ix]);
            }
        }
        IssuerAction::FreezeCustody { slot } => {
            let provisioned = world.mint(slot)?;
            let mut ixs = Vec::new();
            for t in tracked.iter().filter(|t| t.custody && &t.slot == slot) {
                if account_exists(&world.svm, &t.address) {
                    ixs.push(
                        freeze_account(
                            &provisioned.program_id(),
                            &t.address,
                            &provisioned.address,
                            &issuer_pk,
                            &[],
                        )
                        .map_err(|e| ix_err("freeze_account", &e))?,
                    );
                }
            }
            if !ixs.is_empty() {
                out.push(ixs);
            }
        }
        IssuerAction::HarvestWithheld { slot } => {
            let provisioned = world.mint(slot)?;
            if provisioned.config.transfer_fee().is_some() {
                let sources: Vec<&Pubkey> = tracked
                    .iter()
                    .filter(|t| &t.slot == slot && account_exists(&world.svm, &t.address))
                    .map(|t| &t.address)
                    .collect();
                if !sources.is_empty() {
                    out.push(vec![harvest_withheld_tokens_to_mint(
                        &provisioned.program_id(),
                        &provisioned.address,
                        &sources,
                    )
                    .map_err(|e| ix_err("harvest_withheld_tokens_to_mint", &e))?]);
                }
            }
        }
        IssuerAction::SetTransferFee {
            slot,
            basis_points,
            maximum_fee,
        } => {
            if *basis_points > MAX_FEE_BASIS_POINTS {
                return Err(Error::Adapter(format!(
                    "issuer:set_transfer_fee({slot}) asks for {basis_points} basis points; the maximum is {MAX_FEE_BASIS_POINTS}"
                )));
            }
            let provisioned = world.mint(slot)?;
            if let Some((_, current_max)) =
                mint::epoch_transfer_fee(&world.svm, &provisioned.address)?
            {
                out.push(vec![set_transfer_fee(
                    &provisioned.program_id(),
                    &provisioned.address,
                    &issuer_pk,
                    &[],
                    *basis_points,
                    maximum_fee.unwrap_or(current_max),
                )
                .map_err(|e| ix_err("set_transfer_fee", &e))?]);
            }
        }
        IssuerAction::Pause { slot } | IssuerAction::Resume { slot } => {
            let provisioned = world.mint(slot)?;
            if provisioned.config.has("pausable") {
                let program = provisioned.program_id();
                let ix = if matches!(action, IssuerAction::Pause { .. }) {
                    pausable::instruction::pause(&program, &provisioned.address, &issuer_pk, &[])
                        .map_err(|e| ix_err("pausable::pause", &e))?
                } else {
                    pausable::instruction::resume(&program, &provisioned.address, &issuer_pk, &[])
                        .map_err(|e| ix_err("pausable::resume", &e))?
                };
                out.push(vec![ix]);
            }
        }
        IssuerAction::PauseAll => {
            for (_, provisioned) in world.mints().filter(|(_, m)| m.config.has("pausable")) {
                out.push(vec![pausable::instruction::pause(
                    &provisioned.program_id(),
                    &provisioned.address,
                    &issuer_pk,
                    &[],
                )
                .map_err(|e| ix_err("pausable::pause", &e))?]);
            }
        }
    }
    Ok(out)
}

/// The transfer fee in effect for every slot's mint at the current epoch.
fn fee_rates(world: &World) -> Result<BTreeMap<String, Option<(u16, u64)>>> {
    world
        .mints()
        .map(|(slot, m)| {
            Ok((
                slot.clone(),
                mint::epoch_transfer_fee(&world.svm, &m.address)?,
            ))
        })
        .collect()
}

fn account_exists(svm: &LiteSVM, address: &Pubkey) -> bool {
    svm.get_account(address)
        .map(|a| a.lamports > 0)
        .unwrap_or(false)
}

/// Balances of every tracked account and every mint. An account that does
/// not exist (never created, or closed) is recorded with `exists: false` and
/// zero balances.
fn snapshot(world: &World, tracked: &[TrackedAccount]) -> Result<Snapshot> {
    let mut accounts = Vec::with_capacity(tracked.len());
    for t in tracked {
        let view = if account_exists(&world.svm, &t.address) {
            mint::read_token_account(&world.svm, &t.address)?
        } else {
            None
        };
        accounts.push(match view {
            Some(v) => AccountSnapshot {
                label: t.label.clone(),
                address: t.address,
                slot: t.slot.clone(),
                exists: true,
                amount: v.amount,
                withheld: v.withheld,
                lamports: v.lamports,
            },
            None => AccountSnapshot {
                label: t.label.clone(),
                address: t.address,
                slot: t.slot.clone(),
                exists: false,
                amount: 0,
                withheld: 0,
                lamports: 0,
            },
        });
    }
    let mut mints = Vec::new();
    for (slot, provisioned) in world.mints() {
        let view = mint::read_mint(&world.svm, &provisioned.address)?;
        mints.push(MintSnapshot {
            slot: slot.clone(),
            supply: view.supply,
            withheld: view.withheld,
        });
    }
    Ok(Snapshot { accounts, mints })
}

fn is_flow_of_kind(step: &Step, kind: FlowKind, kinds: &BTreeMap<String, FlowKind>) -> bool {
    matches!(step, Step::Flow { name } if kinds.get(name) == Some(&kind))
}

/// Refundability for every entry flow and the next exit flow after it (an
/// entry with another entry before any exit is not paired).
///
/// Per mint slot whose custody accounts received tokens in the entry flow:
/// `deposited` is what left the non-custody, non-issuer tracked accounts in
/// the entry flow, and `returned` is what those accounts received in the exit
/// flow. The allowed loss is the transfer fee in effect during the entry flow
/// on the deposit, plus the fee in effect during the exit flow on the way back
/// out (they differ after an `issuer:set_transfer_fee` and enough epochs).
fn refundability_checks(
    case: &Case,
    steps: &[StepRecord],
    snaps: &[Snapshot],
    fees: &[BTreeMap<String, Option<(u16, u64)>>],
    tracked: &[TrackedAccount],
    kinds: &BTreeMap<String, FlowKind>,
) -> Vec<InvariantCheck> {
    let scenario = &case.scenario.steps;
    let is_holder = |label: &str| {
        tracked
            .iter()
            .any(|t| t.label == label && !t.custody && !t.label.starts_with("issuer["))
    };
    let is_custody = |label: &str| tracked.iter().any(|t| t.label == label && t.custody);
    let mut out = Vec::new();
    for (i, step) in scenario.iter().enumerate() {
        if !is_flow_of_kind(step, FlowKind::Entry, kinds) {
            continue;
        }
        let exit = scenario.iter().enumerate().skip(i + 1).find_map(|(j, s)| {
            if is_flow_of_kind(s, FlowKind::Entry, kinds) {
                Some(None)
            } else if is_flow_of_kind(s, FlowKind::Exit, kinds) {
                Some(Some(j))
            } else {
                None
            }
        });
        let Some(Some(j)) = exit else {
            continue;
        };
        let Step::Flow { name: exit_flow } = &scenario[j] else {
            continue;
        };
        let Step::Flow { name: entry_flow } = step else {
            continue;
        };
        if steps[i].outcome != StepOutcome::Succeeded {
            let none = ExitAttempt {
                flow: exit_flow.clone(),
                succeeded: false,
                error: None,
            };
            out.push(invariants::refundability(false, &none, 0, 0, 0));
            continue;
        }
        let (entry_before, entry_after) = (&snaps[i], &snaps[i + 1]);
        let mut found = false;
        for slot in case.slots.keys() {
            let custody_in: i128 = entry_after
                .accounts
                .iter()
                .filter(|a| &a.slot == slot && is_custody(&a.label))
                .map(|a| {
                    let before = entry_before
                        .account(&a.label)
                        .map(|b| b.amount as i128 + b.withheld as i128)
                        .unwrap_or(0);
                    a.amount as i128 + a.withheld as i128 - before
                })
                .sum();
            if custody_in <= 0 {
                continue;
            }
            found = true;
            let moved = |from: &Snapshot, to: &Snapshot, outflow: bool| -> u64 {
                to.accounts
                    .iter()
                    .filter(|a| &a.slot == slot && is_holder(&a.label))
                    .map(|a| {
                        let before = from.account(&a.label).map(|b| b.amount).unwrap_or(0);
                        if outflow {
                            before.saturating_sub(a.amount)
                        } else {
                            a.amount.saturating_sub(before)
                        }
                    })
                    .sum()
            };
            let deposited = moved(entry_before, entry_after, true);
            // The exit's fee: the snapshot after the exit step when it ran,
            // else the latest one (a failed or skipped exit never moved
            // tokens, so this only affects the detail text).
            let fee_at = |k: usize, amount: u64| match fees.get(k).and_then(|f| f.get(slot)) {
                Some(Some((bps, max))) => mint::fee_for(*bps, *max, amount),
                _ => 0,
            };
            let exit_k = (j + 1).min(fees.len() - 1);
            let fee_in = fee_at(i + 1, deposited);
            let allowed = fee_in.saturating_add(fee_at(exit_k, deposited.saturating_sub(fee_in)));
            let mut c = match steps[j].outcome {
                StepOutcome::Skipped => check(
                    InvariantKind::Refundability,
                    None,
                    CheckStatus::Skipped,
                    format!("exit flow `{exit_flow}` did not run because an earlier step failed"),
                ),
                StepOutcome::Failed => {
                    let attempt = ExitAttempt {
                        flow: exit_flow.clone(),
                        succeeded: false,
                        error: steps[j].transactions.iter().find_map(|t| t.error.clone()),
                    };
                    invariants::refundability(true, &attempt, deposited, 0, allowed)
                }
                StepOutcome::Succeeded => {
                    let returned = moved(&snaps[j], &snaps[j + 1], false);
                    let attempt = ExitAttempt {
                        flow: exit_flow.clone(),
                        succeeded: true,
                        error: None,
                    };
                    invariants::refundability(true, &attempt, deposited, returned, allowed)
                }
            };
            c.slot = Some(slot.clone());
            out.push(c);
        }
        if !found {
            out.push(check(
                InvariantKind::Refundability,
                None,
                CheckStatus::Skipped,
                format!("no custody account received tokens in entry flow `{entry_flow}`"),
            ));
        }
    }
    out
}

/// Support policy that applies to a case: the varied slot's, or for a case
/// without a varied slot, the first slot whose config has extensions (none
/// for a baseline case).
fn support_policy(adapter: &dyn Adapter, case: &Case) -> SupportPolicy {
    if let Some(slot) = &case.varied_slot {
        if let Some(config) = case.slots.get(slot) {
            return adapter.support_policy(slot, config);
        }
    }
    case.slots
        .iter()
        .find(|(_, c)| !c.is_neutral())
        .map(|(slot, c)| adapter.support_policy(slot, c))
        .unwrap_or_default()
}

/// The token programs LiteSVM 0.10.0 loads by default, for the report's
/// environment section.
pub fn bundled_token_programs() -> Vec<ProgramInfo> {
    vec![
        ProgramInfo {
            name: "spl_token".into(),
            program_id: spl_token_2022_interface::inline_spl_token::id(),
            source: "bundled with litesvm 0.10.0 (spl_token-3.5.0.so)".into(),
            sha256: None,
        },
        ProgramInfo {
            name: "spl_token_2022".into(),
            program_id: spl_token_2022_interface::id(),
            source: "bundled with litesvm 0.10.0 (spl_token_2022-10.0.0.so)".into(),
            sha256: None,
        },
        ProgramInfo {
            name: "spl_associated_token_account".into(),
            program_id: spl_associated_token_account_interface::program::id(),
            source: "bundled with litesvm 0.10.0 (spl_associated_token_account-1.1.1.so)".into(),
            sha256: None,
        },
    ]
}
