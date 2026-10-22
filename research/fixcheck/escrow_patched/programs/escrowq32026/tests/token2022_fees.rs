//! ExtensionLab finding #1: the escrow's make / take / refund / update flows run against
//! Token-2022 mints, with and without the TransferFeeConfig extension.
//!
//! The program itself is unchanged. Every test that documents a bug asserts the failure
//! explicitly (error code + log line), so the suite passes while the bug exists and the
//! test turns red the moment the program's behaviour changes.
//!
//! Run with logs:  cargo test --test token2022_fees -- --nocapture --test-threads=1
//!
//! Token-2022 program: the one LiteSVM 0.10.0 ships (litesvm/src/programs/elf/spl_token_2022-10.0.0.so).
//! Instruction builders: spl-token-2022-interface 2.1.0, re-exported by anchor-spl 1.1.2 as
//! `anchor_spl::token_2022::spl_token_2022`, so no new dependency was needed.

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

// ---------------------------------------------------------------------------------------
// Control: Token-2022 with no extensions
// ---------------------------------------------------------------------------------------

/// Control. With extension-free Token-2022 mints, make + take behave exactly like the
/// SPL Token tests in tests/mod.rs. Anything that breaks below is caused by the
/// extension (or by refund's ATA derivation), not by Token-2022 itself.
#[test]
fn control_plain_token2022_make_and_take() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);
    assert_eq!(balance(&env.svm, &env.vault), (DEPOSIT, 0));

    let r = env.take(RECEIVE);
    expect_success("take (plain Token-2022)", r);

    assert_eq!(
        balance(&env.svm, &env.maker_ata_a).0,
        START_BALANCE - DEPOSIT
    );
    assert_eq!(balance(&env.svm, &env.taker_ata_a).0, DEPOSIT);
    assert_eq!(balance(&env.svm, &env.maker_ata_b).0, RECEIVE);
    assert_eq!(
        balance(&env.svm, &env.taker_ata_b).0,
        START_BALANCE - RECEIVE
    );
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

/// BUG (independent of fees): `refund` cannot run for ANY Token-2022 mint.
///
/// Refund's `maker_ata_a` and `vault` use `associated_token::mint/authority` without
/// `associated_token::token_program`, so Anchor derives the expected address with the
/// legacy SPL Token program id. A Token-2022 ATA lives at a different address, and the
/// constraint fails before the handler runs.
#[test]
fn repro_refund_rejected_for_any_token2022_mint() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);

    let r = env.refund();
    let logs = expect_custom_failure(
        "refund (plain Token-2022)",
        r,
        0,
        ANCHOR_CONSTRAINT_ASSOCIATED,
    );
    assert_logs_contain(&logs, "AnchorError caused by account: maker_ata_a");
    assert_logs_contain(
        &logs,
        "Error Code: ConstraintAssociated. Error Number: 2009",
    );

    // Nothing moved; the deposit is still in the vault.
    assert_eq!(balance(&env.svm, &env.vault), (DEPOSIT, 0));
    assert_eq!(
        balance(&env.svm, &env.maker_ata_a).0,
        START_BALANCE - DEPOSIT
    );
    assert!(env.svm.get_account(&env.escrow).is_some());
}

/// Consequence of the refund bug: once a Token-2022 escrow expires, nothing can move
/// the deposit. take refuses (expired), update refuses (expired), refund refuses
/// (ConstraintAssociated). The maker's tokens are locked in the vault permanently.
#[test]
fn repro_expired_token2022_escrow_is_locked_forever() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);

    let mut clock: Clock = env.svm.get_sysvar();
    clock.unix_timestamp = EXPIRATION + 1;
    env.svm.set_sysvar(&clock);

    let r = env.take(RECEIVE);
    let logs = expect_custom_failure("take after expiry", r, 0, ESCROW_EXPIRED);
    assert_logs_contain(&logs, "Error Code: EscrowExpired. Error Number: 6000");

    let ix = env.update_ix(None, Some(EXPIRATION + 1_000_000));
    let r = send(&mut env.svm, &[ix], &env.maker, &[]);
    expect_custom_failure("update(expiration) after expiry", r, 0, ESCROW_EXPIRED);

    let r = env.refund();
    expect_custom_failure("refund after expiry", r, 0, ANCHOR_CONSTRAINT_ASSOCIATED);

    assert_eq!(balance(&env.svm, &env.vault), (DEPOSIT, 0));
}

/// The program cannot swap a legacy SPL Token for a Token-2022 token at all: `make`
/// binds both mints to one `token_program`. Not a fund-loss bug, a documented
/// compatibility limit.
#[test]
fn limit_mixed_token_programs_rejected_at_make() {
    let mut svm = new_svm();
    let maker = Keypair::new();
    svm.airdrop(&maker.pubkey(), 10_000_000_000).unwrap();

    let legacy_mint = CreateMint::new(&mut svm, &maker)
        .decimals(DECIMALS)
        .authority(&maker.pubkey())
        .send()
        .unwrap();
    let legacy_ata = CreateAssociatedTokenAccount::new(&mut svm, &maker, &legacy_mint)
        .owner(&maker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut svm, &maker, &legacy_mint, &legacy_ata, START_BALANCE)
        .send()
        .unwrap();
    let t22_mint = create_mint_2022(&mut svm, &maker, MintKind::Plain);

    let escrow = Pubkey::find_program_address(
        &[b"escrow", maker.pubkey().as_ref(), &SEED.to_le_bytes()],
        &escrowq32026::id(),
    )
    .0;

    let make_ix = |token_program: Pubkey| Instruction {
        program_id: escrowq32026::id(),
        accounts: escrowq32026::accounts::Make {
            maker: maker.pubkey(),
            mint_a: legacy_mint,
            mint_b: t22_mint,
            maker_ata_a: legacy_ata,
            escrow,
            vault: get_associated_token_address_with_program_id(
                &escrow,
                &legacy_mint,
                &token_program,
            ),
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            token_program,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: escrowq32026::instruction::Make {
            seed: SEED,
            deposit: DEPOSIT,
            receive: RECEIVE,
            expiration: EXPIRATION,
        }
        .data(),
    };

    // token_program = SPL Token: the vault `init` runs first (and succeeds), then the
    // `mint::token_program` check on mint_b rejects the Token-2022 mint.
    let r = send(&mut svm, &[make_ix(LEGACY_TOKEN_PROGRAM_ID)], &maker, &[]);
    let logs = expect_custom_failure(
        "make legacy A / Token-2022 B, token_program = SPL Token",
        r,
        0,
        ANCHOR_CONSTRAINT_MINT_TOKEN_PROGRAM,
    );
    assert_logs_contain(
        &logs,
        "Error Code: ConstraintMintTokenProgram. Error Number: 2022",
    );

    // token_program = Token-2022: the vault `init` CPI (ATA program -> Token-2022
    // GetAccountDataSize on a legacy mint) fails before Anchor reaches its constraints.
    let r = send(&mut svm, &[make_ix(TOKEN_2022)], &maker, &[]);
    match r {
        Ok(meta) => {
            print_logs(
                "make legacy A / Token-2022 B, token_program = Token-2022",
                &meta.logs,
            );
            panic!("expected IncorrectProgramId, got success");
        }
        Err(failed) => {
            print_logs(
                "make legacy A / Token-2022 B, token_program = Token-2022",
                &failed.meta.logs,
            );
            assert_eq!(
                failed.err,
                TransactionError::InstructionError(0, InstructionError::IncorrectProgramId)
            );
            assert_logs_contain(&failed.meta.logs, "Program log: Error: IncorrectProgramId");
        }
    }
}

// ---------------------------------------------------------------------------------------
// The Token-2022 rule in isolation (no escrow involved)
// ---------------------------------------------------------------------------------------

/// Token-2022 withholds the transfer fee in the DESTINATION account and refuses to close
/// an account whose withheld amount is non-zero, even when its balance is zero. A
/// permissionless HarvestWithheldTokensToMint clears it.
#[test]
fn rule_token2022_refuses_to_close_account_with_withheld_fees() {
    let mut svm = new_svm();
    let issuer = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();
    let stranger = Keypair::new();
    for kp in [&issuer, &alice, &bob, &stranger] {
        svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
    }
    let mint = create_mint_2022(&mut svm, &issuer, FEE);
    let alice_ata = create_ata(&mut svm, &alice, &alice.pubkey(), &mint);
    let bob_ata = create_ata(&mut svm, &bob, &bob.pubkey(), &mint);
    mint_to(&mut svm, &issuer, &mint, &alice_ata, DEPOSIT);

    // alice -> bob, 10 tokens: bob is credited 9.8, 0.2 is withheld in bob's account.
    let ix = spl_token_2022::instruction::transfer_checked(
        &TOKEN_2022,
        &alice_ata,
        &mint,
        &bob_ata,
        &alice.pubkey(),
        &[],
        DEPOSIT,
        DECIMALS,
    )
    .unwrap();
    expect_success("alice -> bob", send(&mut svm, &[ix], &alice, &[]));
    assert_eq!(balance(&svm, &bob_ata), (9_800_000, 200_000));

    // bob moves his whole balance back out. amount = 0, withheld still 200_000.
    let ix = spl_token_2022::instruction::transfer_checked(
        &TOKEN_2022,
        &bob_ata,
        &mint,
        &alice_ata,
        &bob.pubkey(),
        &[],
        9_800_000,
        DECIMALS,
    )
    .unwrap();
    expect_success("bob -> alice", send(&mut svm, &[ix], &bob, &[]));
    assert_eq!(balance(&svm, &bob_ata), (0, 200_000));

    let close = spl_token_2022::instruction::close_account(
        &TOKEN_2022,
        &bob_ata,
        &bob.pubkey(),
        &bob.pubkey(),
        &[],
    )
    .unwrap();
    let logs = expect_custom_failure(
        "close bob (withheld 200_000)",
        send(&mut svm, &[close.clone()], &bob, &[]),
        0,
        TOKEN_2022_ACCOUNT_HAS_WITHHELD_TRANSFER_FEES,
    );
    assert_logs_contain(
        &logs,
        "Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again",
    );

    // Anyone can harvest: the stranger holds no authority over the mint or the account.
    let harvest = fee_ix::harvest_withheld_tokens_to_mint(&TOKEN_2022, &mint, &[&bob_ata]).unwrap();
    expect_success(
        "stranger harvests bob",
        send(&mut svm, &[harvest], &stranger, &[]),
    );
    assert_eq!(balance(&svm, &bob_ata), (0, 0));
    assert_eq!(mint_withheld(&svm, &mint), 200_000);

    expect_success(
        "close bob after harvest",
        send(&mut svm, &[close], &bob, &[]),
    );
    assert!(svm.get_account(&bob_ata).is_none());
}

// ---------------------------------------------------------------------------------------
// Scenario (a): make with a fee-bearing mint_a
// ---------------------------------------------------------------------------------------

#[test]
fn a_fee_mint_a_make_vault_holds_less_than_deposit() {
    let mut env = setup(FEE, MintKind::Plain, true);
    let fee_in = expected_fee(FEE, DEPOSIT);
    assert_eq!(fee_in, 200_000);

    let meta = env.make(DEPOSIT, RECEIVE);
    println!("make CUs: {}", meta.compute_units_consumed);

    // The maker is debited the full nominal deposit...
    assert_eq!(
        balance(&env.svm, &env.maker_ata_a),
        (START_BALANCE - DEPOSIT, 0)
    );
    // ...the vault is credited 2% less, and the 2% sits in the vault as withheld fee.
    assert_eq!(balance(&env.svm, &env.vault), (DEPOSIT - fee_in, fee_in));
    assert_eq!(balance(&env.svm, &env.vault), (9_800_000, 200_000));

    // The escrow account never records the deposit at all. It records the price (receive)
    // and the mints; take/refund later move "whatever vault.amount says".
    let e = escrow_state(&env.svm, &env.escrow);
    assert_eq!(e.receive, RECEIVE);
    assert_eq!(e.mint_a, env.mint_a);
    assert_eq!(e.mint_b, env.mint_b);
    assert_eq!(e.maker, env.maker.pubkey());
    assert_eq!(e.seed, SEED);
    assert_eq!(e.expiration, EXPIRATION);
}

// ---------------------------------------------------------------------------------------
// Scenario (b): take
// ---------------------------------------------------------------------------------------

/// BUG: with a fee-bearing mint_a, `take` always fails. Both transfers succeed, then
/// close_account on the vault fails because the vault still holds the 200_000 fee
/// withheld during `make`. The whole transaction reverts.
#[test]
fn repro_b_fee_mint_a_take_fails_closing_vault_with_withheld_fees() {
    let mut env = setup(FEE, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);

    let r = env.take(RECEIVE);
    let logs = expect_custom_failure(
        "take (fee-bearing mint_a)",
        r,
        0,
        TOKEN_2022_ACCOUNT_HAS_WITHHELD_TRANSFER_FEES,
    );
    assert_logs_contain(&logs, "Program log: Instruction: CloseAccount");
    assert_logs_contain(
        &logs,
        "Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again",
    );
    assert_logs_contain(&logs, "custom program error: 0x23");
    // Both token movements ran before the close was attempted.
    let transfers = logs
        .iter()
        .filter(|l| l.contains("Instruction: TransferChecked"))
        .count();
    assert_eq!(
        transfers, 2,
        "expected both TransferChecked CPIs before CloseAccount"
    );

    // Atomic revert: nothing moved.
    assert_eq!(balance(&env.svm, &env.vault), (9_800_000, 200_000));
    assert_eq!(balance(&env.svm, &env.taker_ata_b), (START_BALANCE, 0));
    assert!(env.svm.get_account(&env.taker_ata_a).is_none());
    assert!(env.svm.get_account(&env.maker_ata_b).is_none());
    assert!(env.svm.get_account(&env.escrow).is_some());
}

/// Workaround that needs no program change: anyone (here an unrelated stranger) harvests
/// the vault's withheld fee to the mint, after which take succeeds. Shows the exact
/// amounts the taker ends up with: the fee is charged twice on the A leg.
#[test]
fn b_fee_mint_a_take_succeeds_after_permissionless_harvest() {
    let mut env = setup(FEE, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);

    let stranger = Keypair::new();
    env.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    let harvest =
        fee_ix::harvest_withheld_tokens_to_mint(&TOKEN_2022, &env.mint_a, &[&env.vault]).unwrap();
    expect_success(
        "stranger harvests vault",
        send(&mut env.svm, &[harvest], &stranger, &[]),
    );
    assert_eq!(balance(&env.svm, &env.vault), (9_800_000, 0));
    assert_eq!(mint_withheld(&env.svm, &env.mint_a), 200_000);

    let r = env.take(RECEIVE);
    let meta = expect_success("take after harvest", r);
    println!("take CUs: {}", meta.compute_units_consumed);

    let fee_out = expected_fee(FEE, 9_800_000);
    assert_eq!(fee_out, 196_000);
    // Taker: nominal deposit 10_000_000, receives 9_604_000 (96.04%).
    assert_eq!(balance(&env.svm, &env.taker_ata_a), (9_604_000, 196_000));
    assert_eq!(DEPOSIT - 9_604_000, 396_000);
    // mint_b is fee-free here, so the maker gets the full price.
    assert_eq!(balance(&env.svm, &env.maker_ata_b), (RECEIVE, 0));
    assert_eq!(
        balance(&env.svm, &env.taker_ata_b),
        (START_BALANCE - RECEIVE, 0)
    );
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

/// Same workaround done by the taker's client: prepend the harvest to the take in one
/// transaction. This is what an integrator can ship today without redeploying.
#[test]
fn b_fee_mint_a_take_succeeds_with_harvest_prepended_in_same_tx() {
    let mut env = setup(FEE, MintKind::Plain, false);
    env.make(DEPOSIT, RECEIVE);

    let harvest =
        fee_ix::harvest_withheld_tokens_to_mint(&TOKEN_2022, &env.mint_a, &[&env.vault]).unwrap();
    let take = env.take_ix(RECEIVE);
    let r = send(&mut env.svm, &[harvest, take], &env.taker, &[]);
    expect_success("harvest + take in one tx", r);

    assert_eq!(balance(&env.svm, &env.taker_ata_a), (9_604_000, 196_000));
    assert_eq!(mint_withheld(&env.svm, &env.mint_a), 200_000);
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

/// Fee-bearing mint_b: take succeeds (the fee is withheld in maker_ata_b, not in the
/// vault, so the vault closes fine), but the maker silently receives less than the
/// `receive` price stored in the escrow.
#[test]
fn b_fee_mint_b_take_maker_receives_less_than_receive() {
    let mut env = setup(MintKind::Plain, FEE, false);
    env.make(DEPOSIT, RECEIVE);

    let r = env.take(RECEIVE);
    expect_success("take (fee-bearing mint_b)", r);

    let fee_b = expected_fee(FEE, RECEIVE);
    assert_eq!(fee_b, 140_000);
    // Taker pays the gross price, maker gets 98%.
    assert_eq!(
        balance(&env.svm, &env.taker_ata_b),
        (START_BALANCE - RECEIVE, 0)
    );
    assert_eq!(balance(&env.svm, &env.maker_ata_b), (6_860_000, 140_000));
    // mint_a is fee-free: taker gets the full deposit, vault and escrow close.
    assert_eq!(balance(&env.svm, &env.taker_ata_a), (DEPOSIT, 0));
    assert!(env.svm.get_account(&env.vault).is_none());
    assert!(env.svm.get_account(&env.escrow).is_none());
}

/// Both mints fee-bearing: take fails for the same vault-close reason as mint_a alone.
#[test]
fn repro_b_fee_both_mints_take_fails_closing_vault() {
    let mut env = setup(FEE, FEE, true);
    env.make(DEPOSIT, RECEIVE);
    let r = env.take(RECEIVE);
    expect_custom_failure(
        "take (both mints fee-bearing)",
        r,
        0,
        TOKEN_2022_ACCOUNT_HAS_WITHHELD_TRANSFER_FEES,
    );
}

// ---------------------------------------------------------------------------------------
// Scenario (c): refund
// ---------------------------------------------------------------------------------------

/// With a fee-bearing mint_a, refund fails too, but NOT at close: it is rejected by the
/// ATA constraint first (same as with plain Token-2022), before or after a harvest.
/// So the withheld-fee close failure is unreachable in refund with this program as
/// written. Combined with the take bug: a fee-bearing mint_a escrow has no exit at all
/// until someone harvests, and after expiry it has no exit ever.
#[test]
fn repro_c_fee_mint_a_refund_rejected_before_close() {
    let mut env = setup(FEE, MintKind::Plain, true);
    env.make(DEPOSIT, RECEIVE);

    let r = env.refund();
    let logs = expect_custom_failure(
        "refund (fee-bearing mint_a)",
        r,
        0,
        ANCHOR_CONSTRAINT_ASSOCIATED,
    );
    assert_logs_contain(&logs, "AnchorError caused by account: maker_ata_a");
    assert!(
        !logs.iter().any(|l| l.contains("Instruction: CloseAccount")),
        "refund should not reach the CloseAccount CPI"
    );

    let harvest =
        fee_ix::harvest_withheld_tokens_to_mint(&TOKEN_2022, &env.mint_a, &[&env.vault]).unwrap();
    expect_success(
        "harvest vault",
        send(&mut env.svm, &[harvest], &env.maker, &[]),
    );
    let r = env.refund();
    expect_custom_failure("refund after harvest", r, 0, ANCHOR_CONSTRAINT_ASSOCIATED);
    assert_eq!(balance(&env.svm, &env.vault), (9_800_000, 0));
}

// ---------------------------------------------------------------------------------------
// Scenario (d): update / max_receive interplay with a fee-bearing mint_b
// ---------------------------------------------------------------------------------------

/// max_receive bounds the GROSS amount leaving the taker. It works as designed, but it
/// says nothing about what the maker nets, and the maker has no matching floor.
#[test]
fn d_fee_mint_b_max_receive_bounds_gross_not_net() {
    let mut env = setup(MintKind::Plain, FEE, true);
    env.make(DEPOSIT, RECEIVE);

    // A taker who budgets "7 B minus the 2% fee" is rejected: the check is on gross.
    let r = env.take(RECEIVE - expected_fee(FEE, RECEIVE));
    expect_custom_failure("take with max_receive = net", r, 0, RECEIVE_ABOVE_MAX);

    // Maker lowers the price to 5 B via update (allowed direction). The maker then nets
    // 4.9 B, not 5 B; neither update nor take has any notion of the fee.
    let ix = env.update_ix(Some(5_000_000), None);
    expect_success(
        "update receive -> 5_000_000",
        send(&mut env.svm, &[ix], &env.maker, &[]),
    );
    expect_success("take at 5_000_000", env.take(RECEIVE));
    assert_eq!(
        balance(&env.svm, &env.taker_ata_b).0,
        START_BALANCE - 5_000_000
    );
    assert_eq!(balance(&env.svm, &env.maker_ata_b), (4_900_000, 100_000));
}

/// The maker can only get an exact net by grossing up `receive` at make time:
/// gross 7_142_858 -> fee ceil(142_857.16) = 142_858 -> net exactly 7_000_000.
#[test]
fn d_fee_mint_b_gross_up_receive_gives_maker_exact_net() {
    let mut env = setup(MintKind::Plain, FEE, true);
    let gross = 7_142_858u64;
    assert_eq!(gross - expected_fee(FEE, gross), RECEIVE);
    env.make(DEPOSIT, gross);
    expect_success("take grossed-up", env.take(gross));
    assert_eq!(balance(&env.svm, &env.maker_ata_b), (RECEIVE, 142_858));
}

/// The fee is not fixed at make time. The mint's fee authority (the issuer, a third
/// party to the deal) raises the fee to 5%; Token-2022 applies it two epochs later.
/// The escrow's price and the taker's max_receive are unchanged, the take succeeds,
/// and the maker nets 6_650_000 instead of 6_860_000 (or the 7_000_000 they asked for).
#[test]
fn d_fee_mint_b_issuer_fee_change_after_make_cuts_maker_net() {
    let mut env = setup(MintKind::Plain, FEE, true);
    env.make(DEPOSIT, RECEIVE);

    let ix = fee_ix::set_transfer_fee(
        &TOKEN_2022,
        &env.mint_b,
        &env.issuer.pubkey(),
        &[],
        500,
        MAX_FEE,
    )
    .unwrap();
    expect_success(
        "issuer sets fee to 500 bps",
        send(&mut env.svm, &[ix], &env.issuer, &[]),
    );

    let mut clock: Clock = env.svm.get_sysvar();
    clock.epoch += 2;
    env.svm.set_sysvar(&clock);

    expect_success("take after fee change", env.take(RECEIVE));
    assert_eq!(
        balance(&env.svm, &env.taker_ata_b).0,
        START_BALANCE - RECEIVE
    );
    assert_eq!(balance(&env.svm, &env.maker_ata_b), (6_650_000, 350_000));
}

/// Can the maker get around the broken refund by taking their own escrow before expiry?
/// With one wallet: no. taker == maker makes taker_ata_b and maker_ata_b the same mutable
/// account, which Anchor 1.1.2 rejects. (A maker with a SECOND wallet that holds `receive`
/// of mint_b can still take it, so before expiry the deposit is recoverable at the cost of
/// funding that wallet; after expiry it is not.)
#[test]
fn limit_maker_cannot_self_take_with_one_wallet() {
    let mut env = setup(MintKind::Plain, MintKind::Plain, true);
    // give the maker mint_b so a self-take is economically possible
    let maker_pk = env.maker.pubkey();
    mint_to(
        &mut env.svm,
        &env.issuer,
        &env.mint_b,
        &env.maker_ata_b,
        RECEIVE,
    );
    env.make(DEPOSIT, RECEIVE);

    let ix = Instruction {
        program_id: escrowq32026::id(),
        accounts: escrowq32026::accounts::Take {
            taker: maker_pk,
            mint_a: env.mint_a,
            mint_b: env.mint_b,
            taker_ata_b: env.maker_ata_b,
            taker_ata_a: env.maker_ata_a,
            maker: maker_pk,
            escrow: env.escrow,
            vault: env.vault,
            maker_ata_b: env.maker_ata_b,
            associated_token_program: ASSOCIATED_TOKEN_PROGRAM_ID,
            token_program: TOKEN_2022,
            system_program: SYSTEM_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: escrowq32026::instruction::Take {
            max_receive: RECEIVE,
        }
        .data(),
    };
    let r = send(&mut env.svm, &[ix], &env.maker, &[]);
    let logs = expect_custom_failure(
        "self-take (taker == maker)",
        r,
        0,
        ANCHOR_CONSTRAINT_DUPLICATE_MUTABLE_ACCOUNT,
    );
    assert_logs_contain(
        &logs,
        "AnchorError caused by account: maker_ata_b. Error Code: ConstraintDuplicateMutableAccount. Error Number: 2040",
    );
    assert_eq!(balance(&env.svm, &env.vault), (DEPOSIT, 0));
}
