//! FIXCHECK ONLY (research/fixcheck). Positive checks for candidate fixes A and B,
//! run against research/fixcheck/escrow_patched. Not part of the ExtensionLab target.
//!   fix A only:  cargo build-sbf && cargo test --test fixcheck
//!   fix A + B:   cargo build-sbf --features fix-b && cargo test --features fix-b --test fixcheck
//! The harness below is copied verbatim from targets/escrow/.../tests/token2022_fees.rs.
#![allow(dead_code, unused_imports)]

use {
    anchor_lang::{
        prelude::Clock,
        solana_program::{instruction::Instruction, system_instruction},
        system_program::ID as SYSTEM_PROGRAM_ID,
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{
            get_associated_token_address_with_program_id,
            spl_associated_token_account::instruction::create_associated_token_account,
            ID as ASSOCIATED_TOKEN_PROGRAM_ID,
        },
        token_2022::spl_token_2022::{
            self,
            extension::{
                transfer_fee::{instruction as fee_ix, TransferFeeAmount, TransferFeeConfig},
                BaseStateWithExtensions, ExtensionType, StateWithExtensions,
            },
            state::{Account as TokenAccountState, Mint as MintState},
        },
    },
    litesvm::{
        types::{FailedTransactionMetadata, TransactionMetadata},
        LiteSVM,
    },
    litesvm_token::{
        spl_token::ID as LEGACY_TOKEN_PROGRAM_ID, CreateAssociatedTokenAccount, CreateMint, MintTo,
    },
    solana_keypair::Keypair,
    solana_message::Message,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_transaction::{InstructionError, Transaction, TransactionError},
};

const TOKEN_2022: Pubkey = spl_token_2022::ID;

const DECIMALS: u8 = 6;
const FEE_BPS: u16 = 200; // 2%
const MAX_FEE: u64 = u64::MAX; // generous: the cap never binds in these tests
const START_BALANCE: u64 = 100_000_000; // 100 tokens at 6 decimals
const DEPOSIT: u64 = 10_000_000; // 10 token A
const RECEIVE: u64 = 7_000_000; // 7 token B
const SEED: u64 = 123;
const EXPIRATION: i64 = 17_780_206_209; // same far-future value the existing tests use

// Error codes seen in the logs.
const TOKEN_2022_ACCOUNT_HAS_WITHHELD_TRANSFER_FEES: u32 = 35; // 0x23
const ANCHOR_CONSTRAINT_ASSOCIATED: u32 = 2009;
const ANCHOR_CONSTRAINT_MINT_TOKEN_PROGRAM: u32 = 2022;
const ESCROW_EXPIRED: u32 = 6000;
const RECEIVE_ABOVE_MAX: u32 = 6004;
const ANCHOR_CONSTRAINT_DUPLICATE_MUTABLE_ACCOUNT: u32 = 2040;

// ---------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum MintKind {
    /// Token-2022 mint with no extensions.
    Plain,
    /// Token-2022 mint with TransferFeeConfig.
    Fee { bps: u16, max: u64 },
}

const FEE: MintKind = MintKind::Fee {
    bps: FEE_BPS,
    max: MAX_FEE,
};

/// Independent re-implementation of the Token-2022 fee formula (ceil(amount * bps / 10_000),
/// capped at `max`), used only to cross-check the hard-coded numbers in the asserts.
fn expected_fee(kind: MintKind, amount: u64) -> u64 {
    match kind {
        MintKind::Plain => 0,
        MintKind::Fee { bps, max } => {
            let raw = (amount as u128 * bps as u128).div_ceil(10_000) as u64;
            raw.min(max)
        }
    }
}

struct Env {
    svm: LiteSVM,
    /// Mint authority, transfer-fee-config authority and withdraw-withheld authority.
    /// Deliberately not the maker or the taker: in the real world this is the token issuer.
    issuer: Keypair,
    maker: Keypair,
    taker: Keypair,
    mint_a: Pubkey,
    mint_b: Pubkey,
    maker_ata_a: Pubkey,
    maker_ata_b: Pubkey,
    taker_ata_a: Pubkey,
    taker_ata_b: Pubkey,
    escrow: Pubkey,
    vault: Pubkey,
}

fn new_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    let bytes = include_bytes!(concat!(
        env!("CARGO_TARGET_TMPDIR"),
        "/../deploy/escrowq32026.so"
    ));
    svm.add_program(escrowq32026::id(), bytes).unwrap();
    svm
}

fn send(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    payer: &Keypair,
    extra_signers: &[&Keypair],
) -> Result<TransactionMetadata, FailedTransactionMetadata> {
    // A fresh blockhash per send, so retrying an identical instruction is never
    // rejected as AlreadyProcessed.
    svm.expire_blockhash();
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra_signers);
    let message = Message::new(ixs, Some(&payer.pubkey()));
    let tx = Transaction::new(&signers, message, svm.latest_blockhash());
    svm.send_transaction(tx)
}

fn print_logs(label: &str, logs: &[String]) {
    println!("---- {label} ----");
    for line in logs {
        println!("{line}");
    }
}

fn custom_error(err: &TransactionError) -> Option<(u8, u32)> {
    match err {
        TransactionError::InstructionError(idx, InstructionError::Custom(code)) => {
            Some((*idx, *code))
        }
        _ => None,
    }
}

fn assert_logs_contain(logs: &[String], needle: &str) {
    assert!(
        logs.iter().any(|l| l.contains(needle)),
        "expected a log line containing {needle:?}, got:\n{}",
        logs.join("\n")
    );
}

/// Expects the transaction to fail with `Custom(code)` raised by instruction `ix_index`.
fn expect_custom_failure(
    label: &str,
    result: Result<TransactionMetadata, FailedTransactionMetadata>,
    ix_index: u8,
    code: u32,
) -> Vec<String> {
    match result {
        Ok(meta) => {
            print_logs(label, &meta.logs);
            panic!("{label}: expected failure with Custom({code}), but the transaction succeeded");
        }
        Err(failed) => {
            print_logs(label, &failed.meta.logs);
            assert_eq!(
                custom_error(&failed.err),
                Some((ix_index, code)),
                "{label}: unexpected error {:?}",
                failed.err
            );
            failed.meta.logs
        }
    }
}

fn expect_success(
    label: &str,
    result: Result<TransactionMetadata, FailedTransactionMetadata>,
) -> TransactionMetadata {
    match result {
        Ok(meta) => {
            print_logs(label, &meta.logs);
            meta
        }
        Err(failed) => {
            print_logs(label, &failed.meta.logs);
            panic!("{label}: expected success, got {:?}", failed.err);
        }
    }
}

fn create_mint_2022(svm: &mut LiteSVM, issuer: &Keypair, kind: MintKind) -> Pubkey {
    let mint = Keypair::new();
    let extensions: &[ExtensionType] = match kind {
        MintKind::Plain => &[],
        MintKind::Fee { .. } => &[ExtensionType::TransferFeeConfig],
    };
    let space = ExtensionType::try_calculate_account_len::<MintState>(extensions).unwrap();
    let mut ixs = vec![system_instruction::create_account(
        &issuer.pubkey(),
        &mint.pubkey(),
        svm.minimum_balance_for_rent_exemption(space),
        space as u64,
        &TOKEN_2022,
    )];
    if let MintKind::Fee { bps, max } = kind {
        ixs.push(
            fee_ix::initialize_transfer_fee_config(
                &TOKEN_2022,
                &mint.pubkey(),
                Some(&issuer.pubkey()),
                Some(&issuer.pubkey()),
                bps,
                max,
            )
            .unwrap(),
        );
    }
    ixs.push(
        spl_token_2022::instruction::initialize_mint2(
            &TOKEN_2022,
            &mint.pubkey(),
            &issuer.pubkey(),
            None,
            DECIMALS,
        )
        .unwrap(),
    );
    send(svm, &ixs, issuer, &[&mint]).expect("create Token-2022 mint");
    mint.pubkey()
}

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, &TOKEN_2022)
}

fn create_ata(svm: &mut LiteSVM, payer: &Keypair, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let ix = create_associated_token_account(&payer.pubkey(), owner, mint, &TOKEN_2022);
    send(svm, &[ix], payer, &[]).expect("create Token-2022 ATA");
    ata(owner, mint)
}

fn mint_to(svm: &mut LiteSVM, issuer: &Keypair, mint: &Pubkey, dest: &Pubkey, amount: u64) {
    let ix = spl_token_2022::instruction::mint_to(
        &TOKEN_2022,
        mint,
        dest,
        &issuer.pubkey(),
        &[],
        amount,
    )
    .unwrap();
    send(svm, &[ix], issuer, &[]).expect("mint_to");
}

/// (amount, withheld_amount) of a Token-2022 token account.
fn balance(svm: &LiteSVM, account: &Pubkey) -> (u64, u64) {
    let acc = svm
        .get_account(account)
        .unwrap_or_else(|| panic!("token account {account} does not exist"));
    let state = StateWithExtensions::<TokenAccountState>::unpack(&acc.data).unwrap();
    let withheld = state
        .get_extension::<TransferFeeAmount>()
        .map(|e| u64::from(e.withheld_amount))
        .unwrap_or(0);
    (state.base.amount, withheld)
}

/// withheld_amount sitting on the mint (after a harvest).
fn mint_withheld(svm: &LiteSVM, mint: &Pubkey) -> u64 {
    let acc = svm.get_account(mint).unwrap();
    let state = StateWithExtensions::<MintState>::unpack(&acc.data).unwrap();
    u64::from(
        state
            .get_extension::<TransferFeeConfig>()
            .unwrap()
            .withheld_amount,
    )
}

fn escrow_state(svm: &LiteSVM, escrow: &Pubkey) -> escrowq32026::state::Escrow {
    let acc = svm.get_account(escrow).expect("escrow account exists");
    escrowq32026::state::Escrow::try_deserialize(&mut acc.data.as_ref()).unwrap()
}

/// Two Token-2022 mints, both owned by the issuer. The maker holds START_BALANCE of
/// mint_a, the taker holds START_BALANCE of mint_b. `precreate` decides whether the
/// taker's mint_a ATA and the maker's mint_b ATA exist before `take` (take creates them
/// with init_if_needed otherwise).
fn setup(kind_a: MintKind, kind_b: MintKind, precreate: bool) -> Env {
    let mut svm = new_svm();
    let issuer = Keypair::new();
    let maker = Keypair::new();
    let taker = Keypair::new();
    for kp in [&issuer, &maker, &taker] {
        svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    }

    let mint_a = create_mint_2022(&mut svm, &issuer, kind_a);
    let mint_b = create_mint_2022(&mut svm, &issuer, kind_b);

    let maker_ata_a = create_ata(&mut svm, &maker, &maker.pubkey(), &mint_a);
    let taker_ata_b = create_ata(&mut svm, &taker, &taker.pubkey(), &mint_b);
    let (maker_ata_b, taker_ata_a) = if precreate {
        (
            create_ata(&mut svm, &maker, &maker.pubkey(), &mint_b),
            create_ata(&mut svm, &taker, &taker.pubkey(), &mint_a),
        )
    } else {
        (ata(&maker.pubkey(), &mint_b), ata(&taker.pubkey(), &mint_a))
    };

    mint_to(&mut svm, &issuer, &mint_a, &maker_ata_a, START_BALANCE);
    mint_to(&mut svm, &issuer, &mint_b, &taker_ata_b, START_BALANCE);

    let escrow = Pubkey::find_program_address(
        &[b"escrow", maker.pubkey().as_ref(), &SEED.to_le_bytes()],
        &escrowq32026::id(),
    )
    .0;
    let vault = ata(&escrow, &mint_a);

    Env {
        svm,
        issuer,
        maker,
        taker,
        mint_a,
        mint_b,
        maker_ata_a,
        maker_ata_b,
        taker_ata_a,
        taker_ata_b,
        escrow,
        vault,
    }
}

impl Env {
    fn make_ix(&self, deposit: u64, receive: u64) -> Instruction {
        Instruction {
            program_id: escrowq32026::id(),
            accounts: escrowq32026::accounts::Make {
                maker: self.maker.pubkey(),
                mint_a: self.mint_a,
                mint_b: self.mint_b,
                maker_ata_a: self.maker_ata_a,
                escrow: self.escrow,
                vault: self.vault,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                token_program: TOKEN_2022,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: escrowq32026::instruction::Make {
                seed: SEED,
                deposit,
                receive,
                expiration: EXPIRATION,
            }
            .data(),
        }
    }

    fn take_ix(&self, max_receive: u64) -> Instruction {
        Instruction {
            program_id: escrowq32026::id(),
            accounts: escrowq32026::accounts::Take {
                taker: self.taker.pubkey(),
                mint_a: self.mint_a,
                mint_b: self.mint_b,
                taker_ata_b: self.taker_ata_b,
                taker_ata_a: self.taker_ata_a,
                maker: self.maker.pubkey(),
                escrow: self.escrow,
                vault: self.vault,
                maker_ata_b: self.maker_ata_b,
                associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
                token_program: TOKEN_2022,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: escrowq32026::instruction::Take { max_receive }.data(),
        }
    }

    fn refund_ix(&self) -> Instruction {
        Instruction {
            program_id: escrowq32026::id(),
            accounts: escrowq32026::accounts::Refund {
                maker: self.maker.pubkey(),
                mint_a: self.mint_a,
                maker_ata_a: self.maker_ata_a,
                escrow: self.escrow,
                vault: self.vault,
                token_program: TOKEN_2022,
                system_program: SYSTEM_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: escrowq32026::instruction::Refund {}.data(),
        }
    }

    fn update_ix(&self, receive: Option<u64>, expiration: Option<i64>) -> Instruction {
        Instruction {
            program_id: escrowq32026::id(),
            accounts: escrowq32026::accounts::Update {
                maker: self.maker.pubkey(),
                escrow: self.escrow,
            }
            .to_account_metas(None),
            data: escrowq32026::instruction::Update {
                receive,
                expiration,
            }
            .data(),
        }
    }

    fn make(&mut self, deposit: u64, receive: u64) -> TransactionMetadata {
        let ix = self.make_ix(deposit, receive);
        let r = send(&mut self.svm, &[ix], &self.maker, &[]);
        expect_success("make", r)
    }

    fn take(&mut self, max_receive: u64) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.take_ix(max_receive);
        send(&mut self.svm, &[ix], &self.taker, &[])
    }

    fn refund(&mut self) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let ix = self.refund_ix();
        send(&mut self.svm, &[ix], &self.maker, &[])
    }
}

// ---------------------------- fix A only (feature fix-b OFF) ----------------------------

#[cfg(not(feature = "fix-b"))]
#[test]
fn fix_a_refund_plain_token2022_succeeds() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);
    expect_success("refund (plain, fix A)", env.refund());
    assert_eq!(balance(&env.svm, &env.maker_ata_a), (START_BALANCE, 0));
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

#[cfg(not(feature = "fix-b"))]
#[test]
fn fix_a_refund_after_expiry_succeeds() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);
    let mut clock: Clock = env.svm.get_sysvar();
    clock.unix_timestamp = EXPIRATION + 1;
    env.svm.set_sysvar(&clock);
    expect_success("refund after expiry (fix A)", env.refund());
    assert_eq!(balance(&env.svm, &env.maker_ata_a), (START_BALANCE, 0));
}

/// Scenario (c) observed directly: once refund gets past the ATA constraint, it hits the
/// same withheld-fee close failure as take.
#[cfg(not(feature = "fix-b"))]
#[test]
fn fix_a_refund_fee_mint_a_fails_at_close() {
    let mut env = setup(FEE, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);
    let logs = expect_custom_failure(
        "refund (fee mint_a, fix A only)",
        env.refund(),
        0,
        TOKEN_2022_ACCOUNT_HAS_WITHHELD_TRANSFER_FEES,
    );
    assert_logs_contain(&logs, "Program log: Instruction: CloseAccount");
    assert_logs_contain(
        &logs,
        "Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again",
    );
    assert_eq!(balance(&env.svm, &env.vault), (9_800_000, 200_000));
}

/// With a client-side harvest first, refund works, and the maker pays the fee twice on a
/// deal that never happened: 10_000_000 out, 9_604_000 back.
#[cfg(not(feature = "fix-b"))]
#[test]
fn fix_a_refund_fee_mint_a_after_harvest_costs_maker_two_fees() {
    let mut env = setup(FEE, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);
    let harvest =
        fee_ix::harvest_withheld_tokens_to_mint(&TOKEN_2022, &env.mint_a, &[&env.vault]).unwrap();
    let refund = env.refund_ix();
    expect_success(
        "harvest + refund (fix A)",
        send(&mut env.svm, &[harvest, refund], &env.maker, &[]),
    );
    assert_eq!(
        balance(&env.svm, &env.maker_ata_a),
        (START_BALANCE - DEPOSIT + 9_604_000, 196_000)
    );
    assert_eq!(
        START_BALANCE - balance(&env.svm, &env.maker_ata_a).0,
        396_000
    );
    assert!(env.svm.get_account(&env.vault).is_none());
}

// ---------------------------- fix A + B (feature fix-b ON) ------------------------------

#[cfg(feature = "fix-b")]
#[test]
fn fix_b_take_fee_mint_a_succeeds_without_client_harvest() {
    let mut env = setup(FEE, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);
    let meta = expect_success("take (fee mint_a, fix B)", env.take(RECEIVE));
    println!("take CUs (fix B): {}", meta.compute_units_consumed);
    assert_logs_contain(&meta.logs, "Instruction: HarvestWithheldTokensToMint");
    assert_eq!(balance(&env.svm, &env.taker_ata_a), (9_604_000, 196_000));
    assert_eq!(mint_withheld(&env.svm, &env.mint_a), 200_000);
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

#[cfg(feature = "fix-b")]
#[test]
fn fix_b_refund_fee_mint_a_succeeds_without_client_harvest() {
    let mut env = setup(FEE, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);
    expect_success("refund (fee mint_a, fix B)", env.refund());
    assert_eq!(
        balance(&env.svm, &env.maker_ata_a),
        (START_BALANCE - DEPOSIT + 9_604_000, 196_000)
    );
    assert!(env.svm.get_account(&env.vault).is_none());
}

#[cfg(feature = "fix-b")]
#[test]
fn fix_b_take_plain_token2022_does_not_harvest() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);
    let meta = expect_success("take (plain, fix B)", env.take(RECEIVE));
    assert!(!meta
        .logs
        .iter()
        .any(|l| l.contains("HarvestWithheldTokensToMint")));
    assert_eq!(balance(&env.svm, &env.taker_ata_a), (DEPOSIT, 0));
}
