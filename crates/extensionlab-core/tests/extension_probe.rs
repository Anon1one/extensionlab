//! Token-2022 extension probe: the `[probe]` evidence of
//! `research/02_extension_matrix.md`, as host tests against the programs
//! LiteSVM 0.10.0 bundles (Token-2022 10.0.0, ATA 1.1.1, SPL Memo 3.0.0 at
//! `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`).
//!
//! Each test drives one extension (or one group of claims) through real
//! Token-2022 transactions and asserts what happened: success, the failing
//! instruction and its error, or the resulting state. A test fails when
//! Token-2022's behaviour drifts from what is asserted here.
//!
//! Every check prints one Markdown table row: the claim id, the verdict, what
//! was done, what research/02 recorded (observed on 2026-10-21 with LiteSVM
//! 0.16.0, Token-2022 11.0.0), and what this run observed. A `same` row
//! asserts the research/02 outcome. A `different` row asserts what 10.0.0 does
//! instead. A `not probed` row asserts nothing and says why. Each test prints
//! its rows in one block, so parallel tests do not mix rows; one thread also
//! keeps the blocks in a fixed order:
//!
//! ```sh
//! cargo test -p extensionlab-core --test extension_probe -- --nocapture --test-threads=1
//! ```
//!
//! Ported from a probe that was never committed. Its source and output are kept
//! in `research/logs/02_probe_litesvm-0.16.0_main.rs.txt` and
//! `research/logs/02_probe_litesvm-0.16.0_out.txt`. The instruction order of
//! every transaction is the same as there, so `ix N` indices compare directly.
//!
//! Keypairs are derived from fixed names, so runs are deterministic. No
//! program is built, so nothing here goes through a CPI.

use extensionlab_core::{
    mint::{
        create_actor_token_account, create_mint, epoch_transfer_fee, mint_tokens, read_mint,
        read_token_account, send_setup, MintConfig, MintExtension, ProvisionedMint,
        TokenAccountStatus, TokenAccountView,
    },
    runner::derive_keypair,
};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use solana_transaction::{InstructionError, Transaction, TransactionError};
use spl_associated_token_account_interface::{
    address::get_associated_token_address_with_program_id,
    instruction::{create_associated_token_account, create_associated_token_account_idempotent},
};
use spl_token_2022_interface::{
    error::TokenError,
    extension::{
        confidential_transfer, cpi_guard, default_account_state, group_member_pointer,
        group_pointer, interest_bearing_mint, memo_transfer, metadata_pointer, pausable,
        scaled_ui_amount, transfer_fee, transfer_hook, BaseStateWithExtensions, ExtensionType,
        StateWithExtensions,
    },
    instruction as ti,
    state::{Account, AccountState, Mint},
};

// Boxed: `FailedTransactionMetadata` carries the logs and is large.
type TxResult = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

const T22: Pubkey = spl_token_2022_interface::ID;
const PROBE_SEED: u64 = 0;
const YEAR_SECONDS: i64 = 365 * 24 * 3600;

fn memo_program() -> Pubkey {
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
        .parse()
        .unwrap()
}

fn memo(text: &str) -> Instruction {
    Instruction {
        program_id: memo_program(),
        accounts: vec![],
        data: text.as_bytes().to_vec(),
    }
}

fn xfer(
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    ti::transfer_checked(
        &T22,
        source,
        mint,
        destination,
        authority,
        &[],
        amount,
        decimals,
    )
    .unwrap()
}

/// The legacy `Transfer`, without the mint. Deprecated in the interface;
/// research/02 probes it on purpose.
#[allow(deprecated)]
fn legacy_transfer(
    source: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    ti::transfer(&T22, source, destination, authority, &[], amount).unwrap()
}

fn mint_len(types: &[ExtensionType]) -> usize {
    ExtensionType::try_calculate_account_len::<Mint>(types).unwrap()
}

fn account_len(types: &[ExtensionType]) -> usize {
    ExtensionType::try_calculate_account_len::<Account>(types).unwrap()
}

fn list<T: std::fmt::Debug>(items: &[T]) -> String {
    let names: Vec<String> = items.iter().map(|i| format!("{i:?}")).collect();
    format!("[{}]", names.join(", "))
}

/// Extension names as `read_token_account` and `read_mint` return them,
/// without the quotes `{:?}` would add.
fn names(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn status(view: &TokenAccountView) -> &'static str {
    match view.status {
        TokenAccountStatus::Initialized => "initialized",
        TokenAccountStatus::Frozen => "frozen",
    }
}

/// `ok`, or the error as `ix <index>: <error>`. Custom codes are named with
/// `TokenError` from spl-token-2022-interface 2.1.0 (its order matches the
/// 3.1.1 order research/02 cites); codes outside that enum stay numeric.
fn outcome(result: &TxResult) -> String {
    let Err(failed) = result else {
        return "ok".into();
    };
    match &failed.err {
        TransactionError::InstructionError(i, InstructionError::Custom(code)) => {
            match TokenError::try_from(*code) {
                Ok(e) => format!("ix {i}: {e:?} ({code:#x})"),
                Err(_) => format!("ix {i}: Custom({code})"),
            }
        }
        TransactionError::InstructionError(i, e) => format!("ix {i}: {e:?}"),
        other => format!("{other:?}"),
    }
}

/// First 8 bytes of `sha256(input)`: the instruction discriminator of the
/// token-metadata and token-group interfaces. Built by hand so the probe needs
/// no dependency beyond the crate's own.
fn discriminator(input: &str) -> [u8; 8] {
    solana_sha256_hasher::hash(input.as_bytes()).to_bytes()[..8]
        .try_into()
        .unwrap()
}

/// One test's LiteSVM instance, its keys, and the table rows it prints.
struct Probe {
    test: &'static str,
    svm: LiteSVM,
    payer: Keypair,
    /// Mint authority and every extension authority, and freeze authority
    /// where a mint has one. Added as a signer whenever a message needs it.
    issuer: Keypair,
    rows: Vec<String>,
}

impl Probe {
    fn new(test: &'static str, matrix_section: &str) -> Self {
        let mut svm = LiteSVM::new();
        let payer = derive_keypair(PROBE_SEED, test, "payer");
        let issuer = derive_keypair(PROBE_SEED, test, "issuer");
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        svm.airdrop(&issuer.pubkey(), 1_000_000_000).unwrap();
        let header = format!(
            "\n### {test} (research/02: {matrix_section})\n\n\
             | id | verdict | probe | research/02 (11.0.0) | observed (10.0.0) |\n\
             |---|---|---|---|---|\n"
        );
        Probe {
            test,
            svm,
            payer,
            issuer,
            rows: vec![header],
        }
    }

    fn key(&self, name: &str) -> Keypair {
        derive_keypair(PROBE_SEED, self.test, name)
    }

    fn addr(&self, name: &str) -> Pubkey {
        self.key(name).pubkey()
    }

    /// Send `ixs` paid by `payer`. The issuer and `signers` sign when the
    /// message needs them. The blockhash is expired first, so an identical
    /// transaction can be sent again.
    fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> TxResult {
        self.svm.expire_blockhash();
        let message = Message::new(ixs, Some(&self.payer.pubkey()));
        let required = &message.account_keys[..message.header.num_required_signatures as usize];
        let mut keys: Vec<&Keypair> = Vec::new();
        for kp in [&self.payer, &self.issuer]
            .into_iter()
            .chain(signers.iter().copied())
        {
            let pk = kp.pubkey();
            if required.contains(&pk) && !keys.iter().any(|k| k.pubkey() == pk) {
                keys.push(kp);
            }
        }
        let mut tx = Transaction::new_unsigned(message);
        tx.try_sign(&keys, self.svm.latest_blockhash()).unwrap();
        self.svm.send_transaction(tx).map_err(Box::new)
    }

    /// Send a probe transaction and return its outcome.
    fn probe(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> String {
        outcome(&self.send(ixs, signers))
    }

    /// Send a setup transaction that must succeed.
    fn setup(
        &mut self,
        what: &str,
        ixs: &[Instruction],
        signers: &[&Keypair],
    ) -> TransactionMetadata {
        self.svm.expire_blockhash();
        let mut candidates = vec![&self.issuer];
        candidates.extend_from_slice(signers);
        send_setup(&mut self.svm, what, &self.payer, ixs, &candidates)
            .unwrap_or_else(|e| panic!("{}: {e}", self.test))
    }

    fn row(&mut self, id: &str, verdict: &str, what: &str, research_02: &str, observed: &str) {
        self.rows.push(format!(
            "| {id} | {verdict} | {what} | {research_02} | {observed} |\n"
        ));
    }

    /// A check on which this run and research/02 agree. Asserts `expected`.
    fn same(&mut self, id: &str, what: &str, expected: &str, observed: impl Into<String>) {
        let observed = observed.into();
        self.row(id, "same", what, expected, &observed);
        assert_eq!(observed, expected, "{id}: {what}");
    }

    /// A check where Token-2022 10.0.0 (or this stack) differs from what
    /// research/02 recorded on 11.0.0. Asserts the 10.0.0 outcome `on_10`.
    fn differs(
        &mut self,
        id: &str,
        what: &str,
        research_02: &str,
        on_10: &str,
        observed: impl Into<String>,
    ) {
        let observed = observed.into();
        self.row(id, "different", what, research_02, &observed);
        assert_eq!(observed, on_10, "{id}: {what}");
    }

    /// A research/02 claim that this stack cannot run. Asserts nothing.
    fn not_probed(&mut self, id: &str, what: &str, research_02: &str, reason: &str) {
        self.row(id, "not probed", what, research_02, reason);
    }

    /// Create a mint with the factory. The issuer holds every authority.
    fn factory_mint(&mut self, name: &str, config: MintConfig) -> ProvisionedMint {
        let mint = self.key(name);
        let issuer = self.issuer.pubkey();
        self.svm.expire_blockhash();
        create_mint(&mut self.svm, &config, &self.payer, &mint, &issuer)
            .unwrap_or_else(|e| panic!("{}: {e}", self.test))
    }

    fn mint_factory_tokens(&mut self, mint: &ProvisionedMint, account: &Pubkey, amount: u64) {
        self.svm.expire_blockhash();
        mint_tokens(
            &mut self.svm,
            mint,
            &self.payer,
            &self.issuer,
            account,
            amount,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", self.test));
    }

    /// Instructions for a mint the factory cannot build: `create_account` with
    /// `space` bytes, the extension initializers `init`, then
    /// `InitializeMint2` with the issuer as mint authority (and as freeze
    /// authority when `freeze`). `extra_rent` funds that many more bytes, for
    /// extensions Token-2022 reallocates into later.
    fn mint_ixs(
        &self,
        mint: &Pubkey,
        space: usize,
        init: Vec<Instruction>,
        decimals: u8,
        freeze: bool,
        extra_rent: usize,
    ) -> Vec<Instruction> {
        let lamports = self
            .svm
            .minimum_balance_for_rent_exemption(space + extra_rent);
        let issuer = self.issuer.pubkey();
        let mut ixs = vec![create_account(
            &self.payer.pubkey(),
            mint,
            lamports,
            space as u64,
            &T22,
        )];
        ixs.extend(init);
        ixs.push(
            ti::initialize_mint2(&T22, mint, &issuer, freeze.then_some(&issuer), decimals).unwrap(),
        );
        ixs
    }

    fn raw_mint(
        &mut self,
        name: &str,
        types: &[ExtensionType],
        init: Vec<Instruction>,
        decimals: u8,
        extra_rent: usize,
    ) -> Pubkey {
        let mint = self.key(name);
        let ixs = self.mint_ixs(
            &mint.pubkey(),
            mint_len(types),
            init,
            decimals,
            false,
            extra_rent,
        );
        self.setup(&format!("create mint `{name}`"), &ixs, &[&mint]);
        mint.pubkey()
    }

    fn mint_types(&self, mint: &Pubkey) -> Vec<ExtensionType> {
        let data = self.svm.get_account(mint).unwrap().data;
        StateWithExtensions::<Mint>::unpack(&data)
            .unwrap()
            .get_extension_types()
            .unwrap()
    }

    /// Instructions for a token account at `account`, a keypair address, the
    /// way a program creates a vault: sized for the mint's required account
    /// extensions plus `extra` (or `space` bytes), with `ImmutableOwner` only
    /// when `extra` asks for it. ATAs always carry `ImmutableOwner`, which
    /// would change the sizes research/02 records.
    fn account_ixs(
        &self,
        account: &Pubkey,
        mint: &Pubkey,
        owner: &Pubkey,
        extra: &[ExtensionType],
        space: Option<usize>,
    ) -> Vec<Instruction> {
        let mut types = ExtensionType::get_required_init_account_extensions(&self.mint_types(mint));
        for t in extra {
            if !types.contains(t) {
                types.push(*t);
            }
        }
        let space = space.unwrap_or_else(|| account_len(&types));
        let lamports = self.svm.minimum_balance_for_rent_exemption(space);
        let mut ixs = vec![create_account(
            &self.payer.pubkey(),
            account,
            lamports,
            space as u64,
            &T22,
        )];
        if extra.contains(&ExtensionType::ImmutableOwner) {
            ixs.push(ti::initialize_immutable_owner(&T22, account).unwrap());
        }
        ixs.push(ti::initialize_account3(&T22, account, mint, owner).unwrap());
        ixs
    }

    fn token_account(
        &mut self,
        name: &str,
        mint: &Pubkey,
        owner: &Pubkey,
        extra: &[ExtensionType],
    ) -> Pubkey {
        let account = self.key(name);
        let ixs = self.account_ixs(&account.pubkey(), mint, owner, extra, None);
        self.setup(&format!("create token account `{name}`"), &ixs, &[&account]);
        account.pubkey()
    }

    fn mint_to(&mut self, mint: &Pubkey, account: &Pubkey, amount: u64) {
        let ix = ti::mint_to(&T22, mint, account, &self.issuer.pubkey(), &[], amount).unwrap();
        self.setup("mint_to", &[ix], &[]);
    }

    fn view(&self, account: &Pubkey) -> TokenAccountView {
        read_token_account(&self.svm, account)
            .unwrap()
            .expect("token account exists")
    }

    fn len(&self, address: &Pubkey) -> usize {
        self.svm.get_account(address).map_or(0, |a| a.data.len())
    }

    fn exists(&self, address: &Pubkey) -> bool {
        self.svm
            .get_account(address)
            .is_some_and(|a| a.lamports > 0)
    }

    fn ui_amount(&mut self, mint: &Pubkey, amount: u64) -> String {
        let ix = ti::amount_to_ui_amount(&T22, mint, amount).unwrap();
        String::from_utf8(
            self.setup("amount_to_ui_amount", &[ix], &[])
                .return_data
                .data,
        )
        .unwrap()
    }

    fn raw_amount(&mut self, mint: &Pubkey, ui: &str) -> u64 {
        let ix = ti::ui_amount_to_amount(&T22, mint, ui).unwrap();
        let data = self
            .setup("ui_amount_to_amount", &[ix], &[])
            .return_data
            .data;
        u64::from_le_bytes(data[..8].try_into().unwrap())
    }

    fn account_data_size(&mut self, mint: &Pubkey, types: &[ExtensionType]) -> u64 {
        let ix = ti::get_account_data_size(&T22, mint, types).unwrap();
        let data = self
            .setup("get_account_data_size", &[ix], &[])
            .return_data
            .data;
        u64::from_le_bytes(data[..8].try_into().unwrap())
    }

    fn warp(&mut self, seconds: i64, epochs: u64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp += seconds;
        clock.epoch += epochs;
        clock.slot += 1;
        self.svm.set_sysvar(&clock);
    }
}

impl Drop for Probe {
    // Printed on drop so that a failed check still shows the rows before it.
    fn drop(&mut self) {
        print!("{}", self.rows.concat());
    }
}

#[test]
fn account_sizes() {
    let mut p = Probe::new("account_sizes", "Account size reference; finding 2");
    p.same(
        "SIZE.1",
        "base mint, base token account",
        "82, 165",
        format!("{}, {}", mint_len(&[]), account_len(&[])),
    );
    // Mint with a single extension: size; account extensions it forces at
    // InitializeAccount; the size of such an account.
    use ExtensionType as E;
    let mints = [
        (
            E::TransferFeeConfig,
            "278; forces [TransferFeeAmount]; account 178",
        ),
        (E::MintCloseAuthority, "202; forces []; account 165"),
        (E::ConfidentialTransferMint, "235; forces []; account 165"),
        (E::DefaultAccountState, "171; forces []; account 165"),
        (
            E::NonTransferable,
            "170; forces [NonTransferableAccount, ImmutableOwner]; account 174",
        ),
        (E::InterestBearingConfig, "222; forces []; account 165"),
        (E::PermanentDelegate, "202; forces []; account 165"),
        (
            E::TransferHook,
            "234; forces [TransferHookAccount]; account 171",
        ),
        (
            E::ConfidentialTransferFeeConfig,
            "299; forces []; account 165",
        ),
        (E::MetadataPointer, "234; forces []; account 165"),
        (E::GroupPointer, "234; forces []; account 165"),
        (E::TokenGroup, "250; forces []; account 165"),
        (E::GroupMemberPointer, "234; forces []; account 165"),
        (E::TokenGroupMember, "242; forces []; account 165"),
        (E::ScaledUiAmount, "226; forces []; account 165"),
        (E::Pausable, "203; forces [PausableAccount]; account 170"),
    ];
    for (i, (ty, want)) in mints.iter().enumerate() {
        let forced = ExtensionType::get_required_init_account_extensions(&[*ty]);
        let observed = format!(
            "{}; forces {}; account {}",
            mint_len(&[*ty]),
            list(&forced),
            account_len(&forced)
        );
        p.same(
            &format!("SIZE.m{:02}", i + 1),
            &format!("mint + {ty:?}"),
            want,
            observed,
        );
    }
    p.not_probed(
        "SIZE.m17",
        "mint + PermissionedBurn",
        "202; forces []; account 165",
        "spl-token-2022-interface 2.1.0 has no PermissionedBurn extension type",
    );
    p.same(
        "SIZE.m18",
        "mint + TokenMetadata (variable length)",
        "Err(InvalidArgument)",
        format!(
            "{:?}",
            ExtensionType::try_calculate_account_len::<Mint>(&[E::TokenMetadata])
        ),
    );
    let accounts = [
        (E::TransferFeeAmount, "178"),
        (E::ImmutableOwner, "170"),
        (E::MemoTransfer, "171"),
        (E::CpiGuard, "171"),
        (E::NonTransferableAccount, "170"),
        (E::TransferHookAccount, "171"),
        (E::PausableAccount, "170"),
        (E::ConfidentialTransferAccount, "465"),
        (E::ConfidentialTransferFeeAmount, "234"),
    ];
    for (i, (ty, want)) in accounts.iter().enumerate() {
        p.same(
            &format!("SIZE.a{:02}", i + 1),
            &format!("account + {ty:?}"),
            want,
            account_len(&[*ty]).to_string(),
        );
    }
}

#[test]
fn transfer_fee_config() {
    let mut p = Probe::new("transfer_fee_config", "TransferFeeConfig");
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("fee_100bps", 6).with(MintExtension::TransferFee {
            basis_points: 100,
            maximum_fee: 5_000,
        }),
    );
    let mint = m.address;
    let alice = p.key("alice");
    let vault_owner = p.key("vault_owner");
    let bob = p.addr("bob");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let v = p.token_account("vault", &mint, &vault_owner.pubkey(), &[]);
    let b = p.token_account("bob_account", &mint, &bob, &[]);
    let vault = p.view(&v);
    p.same(
        "TF.1",
        "vault account: extensions, size",
        "[TransferFeeAmount], 178",
        format!("{}, {}", names(&vault.extensions), p.len(&v)),
    );

    p.mint_factory_tokens(&m, &a, 10_000);
    let r = p.probe(
        &[xfer(&a, &mint, &v, &alice.pubkey(), 10_000, 6)],
        &[&alice],
    );
    let (av, vv) = (p.view(&a), p.view(&v));
    p.same(
        "TF.2",
        "deposit 10,000 at 100 bps; alice, vault",
        "ok; alice 0, vault 9900 (+100 withheld)",
        format!(
            "{r}; alice {}, vault {} (+{} withheld)",
            av.amount, vv.amount, vv.withheld
        ),
    );
    let r = p.probe(
        &[xfer(&v, &mint, &a, &vault_owner.pubkey(), 10_000, 6)],
        &[&vault_owner],
    );
    p.same(
        "TF.3",
        "withdraw the credited 10,000",
        "ix 0: InsufficientFunds (0x1)",
        r,
    );
    let r = p.probe(
        &[xfer(&v, &mint, &a, &vault_owner.pubkey(), 9_900, 6)],
        &[&vault_owner],
    );
    let (av, vv) = (p.view(&a), p.view(&v));
    p.same(
        "TF.4",
        "withdraw the 9,900 held; alice, vault",
        "ok; alice 9801 (+99 withheld), vault 0 (+100 withheld)",
        format!(
            "{r}; alice {} (+{} withheld), vault {} (+{} withheld)",
            av.amount, av.withheld, vv.amount, vv.withheld
        ),
    );
    let legacy = legacy_transfer(&a, &b, &alice.pubkey(), 10);
    let r = p.probe(&[legacy], &[&alice]);
    p.same(
        "TF.5",
        "legacy Transfer (no mint)",
        "ix 0: MintRequiredForTransfer (0x1f)",
        r,
    );
    let with_fee = transfer_fee::instruction::transfer_checked_with_fee(
        &T22,
        &a,
        &mint,
        &b,
        &alice.pubkey(),
        &[],
        1_000,
        6,
        0,
    )
    .unwrap();
    let r = p.probe(&[with_fee], &[&alice]);
    p.same(
        "TF.6",
        "TransferCheckedWithFee 1,000 with expected fee 0",
        "ix 0: FeeMismatch (0x20)",
        r,
    );
    let r = p.probe(&[xfer(&a, &mint, &b, &alice.pubkey(), 1, 6)], &[&alice]);
    let bv = p.view(&b);
    p.same(
        "TF.7",
        "transfer 1 unit; bob",
        "ok; bob 0 (+1 withheld)",
        format!("{r}; bob {} (+{} withheld)", bv.amount, bv.withheld),
    );

    let close = ti::close_account(&T22, &v, &p.payer.pubkey(), &vault_owner.pubkey(), &[]).unwrap();
    let r = p.probe(std::slice::from_ref(&close), &[&vault_owner]);
    p.same(
        "TF.8",
        "close vault at balance 0 with 100 withheld",
        "ix 0: AccountHasWithheldTransferFees (0x23)",
        r,
    );
    let harvest =
        transfer_fee::instruction::harvest_withheld_tokens_to_mint(&T22, &mint, &[&v]).unwrap();
    let signers = harvest.accounts.iter().filter(|a| a.is_signer).count();
    let r = p.probe(&[harvest], &[]);
    p.same(
        "TF.9",
        "HarvestWithheldTokensToMint from the vault",
        "ok; signed by the fee payer only",
        format!(
            "{r}; {}",
            if signers == 0 {
                "signed by the fee payer only"
            } else {
                "needs an authority"
            }
        ),
    );
    let r = p.probe(&[close], &[&vault_owner]);
    p.same("TF.10", "close vault after the harvest", "ok", r);

    let set = transfer_fee::instruction::set_transfer_fee(
        &T22,
        &mint,
        &p.issuer.pubkey(),
        &[],
        500,
        5_000,
    )
    .unwrap();
    let r = p.probe(&[set], &[]);
    let mut credited = Vec::new();
    for (step, want_bps) in [(0, 100), (1, 100), (1, 500)] {
        p.warp(0, step);
        // The harness's own reading of the fee in effect agrees with Token-2022.
        assert_eq!(
            epoch_transfer_fee(&p.svm, &mint).unwrap(),
            Some((want_bps, 5_000))
        );
        let before = p.view(&b).amount;
        p.setup(
            "transfer 1,000",
            &[xfer(&a, &mint, &b, &alice.pubkey(), 1_000, 6)],
            &[&alice],
        );
        credited.push((p.view(&b).amount - before).to_string());
    }
    p.same(
        "TF.11",
        "SetTransferFee 100 -> 500 bps; 1,000 credited at epoch +0, +1, +2",
        "ok; 990, 990, 950",
        format!("{r}; {}", credited.join(", ")),
    );

    let k = p.key("account_165");
    let ixs = p.account_ixs(&k.pubkey(), &mint, &bob, &[], Some(165));
    let r = p.probe(&ixs, &[&k]);
    p.same(
        "TF.12",
        "InitializeAccount3 on a 165-byte account",
        "ix 1: InvalidAccountData",
        r,
    );
}

#[test]
fn transfer_hook() {
    let mut p = Probe::new("transfer_hook", "TransferHook");
    // The hook program id points at an address with no program.
    let missing_hook = p.addr("missing_hook_program");
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("hook", 6).with(MintExtension::TransferHook {
            program_id: missing_hook,
        }),
    );
    let mint = m.address;
    let alice = p.key("alice");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let vault_owner = p.addr("vault_owner");
    let v = p.token_account("vault", &mint, &vault_owner, &[]);
    let av = p.view(&a);
    p.same(
        "HOOK.1",
        "token account: extensions, size",
        "[TransferHookAccount], 171",
        format!("{}, {}", names(&av.extensions), p.len(&a)),
    );
    p.mint_factory_tokens(&m, &a, 1_000);
    let ix = xfer(&a, &mint, &v, &alice.pubkey(), 100, 6);
    let r = p.probe(std::slice::from_ref(&ix), &[&alice]);
    p.same(
        "HOOK.2",
        "transfer_checked, no extra accounts, hook program missing",
        "ix 0: MissingAccount",
        r,
    );
    let mut with_hook = ix.clone();
    with_hook
        .accounts
        .push(AccountMeta::new_readonly(missing_hook, false));
    let r = p.probe(&[with_hook], &[&alice]);
    p.same(
        "HOOK.3",
        "transfer_checked + the (missing) hook program account",
        "ix 0: UnsupportedProgramId",
        r,
    );
    let legacy = legacy_transfer(&a, &v, &alice.pubkey(), 1);
    let r = p.probe(&[legacy], &[&alice]);
    p.same(
        "HOOK.4",
        "legacy Transfer (no mint)",
        "ix 0: MintRequiredForTransfer (0x1f)",
        r,
    );

    // A second mint whose hook program is None, switched on later.
    let m2 = p.addr("mint_without_hook");
    let init =
        transfer_hook::instruction::initialize(&T22, &m2, Some(p.issuer.pubkey()), None).unwrap();
    p.raw_mint(
        "mint_without_hook",
        &[ExtensionType::TransferHook],
        vec![init],
        6,
        0,
    );
    let a2 = p.token_account("alice_account_2", &m2, &alice.pubkey(), &[]);
    let v2 = p.token_account("vault_2", &m2, &vault_owner, &[]);
    p.mint_to(&m2, &a2, 1_000);
    let ix = xfer(&a2, &m2, &v2, &alice.pubkey(), 100, 6);
    let r = p.probe(std::slice::from_ref(&ix), &[&alice]);
    p.same("HOOK.5", "hook None: transfer_checked", "ok", r);
    let update = transfer_hook::instruction::update(
        &T22,
        &m2,
        &p.issuer.pubkey(),
        &[],
        Some(memo_program()),
    )
    .unwrap();
    let r = p.probe(&[update], &[]);
    p.same(
        "HOOK.6",
        "hook authority sets the hook to the memo program",
        "ok",
        r,
    );
    let r = p.probe(std::slice::from_ref(&ix), &[&alice]);
    p.same(
        "HOOK.7",
        "after the switch: same transfer_checked, no extra accounts",
        "ix 0: MissingAccount",
        r,
    );
    let mut with_hook = ix.clone();
    with_hook
        .accounts
        .push(AccountMeta::new_readonly(memo_program(), false));
    let result = p.send(&[with_hook], &[&alice]);
    // Token-2022 calls the hook without a validation account, and the memo
    // program rejects the non-signer accounts it is given.
    let memo_failed = format!("Program {} failed", memo_program());
    let in_memo = result
        .as_ref()
        .err()
        .is_some_and(|f| f.meta.logs.iter().any(|l| l.starts_with(&memo_failed)));
    p.same(
        "HOOK.8",
        "after the switch: + hook program account",
        "ix 0: MissingRequiredSignature, raised in the memo program",
        format!(
            "{}{}",
            outcome(&result),
            if in_memo {
                ", raised in the memo program"
            } else {
                ""
            }
        ),
    );
    let (validation, _) =
        Pubkey::find_program_address(&[b"extra-account-metas", m2.as_ref()], &memo_program());
    let mut with_validation = ix.clone();
    with_validation
        .accounts
        .push(AccountMeta::new_readonly(validation, false));
    with_validation
        .accounts
        .push(AccountMeta::new_readonly(memo_program(), false));
    let r = p.probe(&[with_validation], &[&alice]);
    p.same(
        "HOOK.9",
        "after the switch: + nonexistent validation PDA + hook program",
        "ix 0: InvalidAccountData",
        r,
    );
    let r = p.probe(&[xfer(&a2, &m2, &a2, &alice.pubkey(), 10, 6)], &[&alice]);
    p.same(
        "HOOK.10",
        "after the switch: self-transfer, no extra accounts",
        "ok",
        r,
    );
}

#[test]
fn permanent_delegate() {
    let mut p = Probe::new("permanent_delegate", "PermanentDelegate");
    // The issuer is the permanent delegate.
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("permanent_delegate", 6).with(MintExtension::PermanentDelegate),
    );
    let mint = m.address;
    let vault_owner = p.addr("vault_owner");
    let k = p.key("vault");
    let ixs = p.account_ixs(&k.pubkey(), &mint, &vault_owner, &[], None);
    let created = p.send(&ixs, &[&k]);
    let warning: Vec<String> = created
        .as_ref()
        .map(|meta| {
            meta.logs
                .iter()
                .filter(|l| l.contains("Warning"))
                .map(|l| l.trim_start_matches("Program log: ").to_string())
                .collect()
        })
        .unwrap_or_default();
    p.same(
        "PD.1",
        "create a vault account; warning logged",
        "ok; Warning: Mint has a permanent delegate, so tokens in this account may be seized at any time",
        format!("{}; {}", outcome(&created), warning.join(" / ")),
    );
    let v = k.pubkey();
    let sink = create_actor_token_account(&mut p.svm, &m, &p.payer, &p.issuer, &p.issuer).unwrap();
    p.mint_factory_tokens(&m, &v, 1_000);
    let issuer = p.issuer.pubkey();
    let r = p.probe(&[xfer(&v, &mint, &sink, &issuer, 600, 6)], &[]);
    p.same(
        "PD.2",
        "delegate transfers 600 out of the vault, owner not signing",
        "ok",
        r,
    );
    let burn = ti::burn_checked(&T22, &v, &mint, &issuer, &[], 400, 6).unwrap();
    let r = p.probe(&[burn], &[]);
    p.same(
        "PD.3",
        "delegate burns 400 from the vault, owner not signing; vault",
        "ok; vault 0",
        format!("{r}; vault {}", p.view(&v).amount),
    );
}

#[test]
fn pausable() {
    let mut p = Probe::new("pausable", "Pausable");
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("pausable", 6).with(MintExtension::Pausable),
    );
    let mint = m.address;
    let alice = p.key("alice");
    let vault_owner = p.key("vault_owner");
    let empty_owner = p.key("empty_owner");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let v = p.token_account("vault", &mint, &vault_owner.pubkey(), &[]);
    let e = p.token_account("empty_account", &mint, &empty_owner.pubkey(), &[]);
    let av = p.view(&a);
    p.same(
        "PAUSE.1",
        "token account: extensions, size",
        "[PausableAccount], 170",
        format!("{}, {}", names(&av.extensions), p.len(&a)),
    );
    p.mint_factory_tokens(&m, &a, 1_000);
    p.setup(
        "deposit before the pause",
        &[xfer(&a, &mint, &v, &alice.pubkey(), 500, 6)],
        &[&alice],
    );
    let issuer = p.issuer.pubkey();
    p.setup(
        "pause",
        &[pausable::instruction::pause(&T22, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    assert!(read_mint(&p.svm, &mint).unwrap().paused);

    let r = p.probe(
        &[xfer(&v, &mint, &a, &vault_owner.pubkey(), 500, 6)],
        &[&vault_owner],
    );
    p.same(
        "PAUSE.2",
        "paused: withdraw with transfer_checked",
        "ix 0: MintPaused (0x43)",
        r,
    );
    let r = p.probe(
        &[ti::mint_to(&T22, &mint, &a, &issuer, &[], 1).unwrap()],
        &[],
    );
    p.same("PAUSE.3", "paused: mint_to", "ix 0: MintPaused (0x43)", r);
    let burn = ti::burn_checked(&T22, &a, &mint, &alice.pubkey(), &[], 1, 6).unwrap();
    let r = p.probe(&[burn], &[&alice]);
    p.same(
        "PAUSE.4",
        "paused: burn_checked",
        "ix 0: MintPaused (0x43)",
        r,
    );
    let delegate = p.addr("delegate");
    let approve = ti::approve(&T22, &a, &delegate, &alice.pubkey(), &[], 1).unwrap();
    let r = p.probe(&[approve], &[&alice]);
    p.same("PAUSE.5", "paused: Approve", "ok", r);
    let close = ti::close_account(&T22, &e, &p.payer.pubkey(), &empty_owner.pubkey(), &[]).unwrap();
    let r = p.probe(&[close], &[&empty_owner]);
    p.same("PAUSE.6", "paused: close an empty account", "ok", r);
    let legacy = legacy_transfer(&v, &a, &vault_owner.pubkey(), 1);
    let r = p.probe(&[legacy], &[&vault_owner]);
    p.same(
        "PAUSE.7",
        "paused: legacy Transfer",
        "ix 0: MintRequiredForTransfer (0x1f)",
        r,
    );
    p.setup(
        "resume",
        &[pausable::instruction::resume(&T22, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    let r = p.probe(
        &[xfer(&v, &mint, &a, &vault_owner.pubkey(), 500, 6)],
        &[&vault_owner],
    );
    p.same("PAUSE.8", "resumed: withdraw", "ok", r);
}

#[test]
fn default_account_state_frozen() {
    let mut p = Probe::new(
        "default_account_state_frozen",
        "DefaultAccountState (Frozen)",
    );
    let no_freeze = p.key("mint_without_freeze_authority");
    let init = default_account_state::instruction::initialize_default_account_state(
        &T22,
        &no_freeze.pubkey(),
        &AccountState::Frozen,
    )
    .unwrap();
    let ixs = p.mint_ixs(
        &no_freeze.pubkey(),
        mint_len(&[ExtensionType::DefaultAccountState]),
        vec![init],
        6,
        false,
        0,
    );
    let r = p.probe(&ixs, &[&no_freeze]);
    p.same(
        "DAS.1",
        "frozen default without a freeze authority",
        "ix 2: MintCannotFreeze (0x10)",
        r,
    );

    // The factory sets the issuer as freeze authority.
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("frozen", 6).with(MintExtension::DefaultAccountStateFrozen),
    );
    let mint = m.address;
    let alice = p.key("alice");
    let vault_owner = p.key("vault_owner");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let v = p.token_account("vault", &mint, &vault_owner.pubkey(), &[]);
    let (av, vv) = (p.view(&a), p.view(&v));
    p.same(
        "DAS.2",
        "new accounts: state",
        "alice frozen, vault frozen",
        format!("alice {}, vault {}", status(&av), status(&vv)),
    );
    let issuer = p.issuer.pubkey();
    let r = p.probe(
        &[ti::mint_to(&T22, &mint, &a, &issuer, &[], 1_000).unwrap()],
        &[],
    );
    p.same(
        "DAS.3",
        "mint_to a frozen account",
        "ix 0: AccountFrozen (0x11)",
        r,
    );
    p.setup(
        "thaw alice",
        &[ti::thaw_account(&T22, &a, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    p.mint_factory_tokens(&m, &a, 1_000);
    let deposit = xfer(&a, &mint, &v, &alice.pubkey(), 500, 6);
    let r = p.probe(std::slice::from_ref(&deposit), &[&alice]);
    p.same(
        "DAS.4",
        "deposit into the frozen vault",
        "ix 0: AccountFrozen (0x11)",
        r,
    );
    p.setup(
        "thaw vault",
        &[ti::thaw_account(&T22, &v, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    let r = p.probe(&[deposit], &[&alice]);
    p.same("DAS.5", "deposit after the issuer thaws the vault", "ok", r);
    p.setup(
        "freeze vault",
        &[ti::freeze_account(&T22, &v, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    let r = p.probe(
        &[xfer(&v, &mint, &a, &vault_owner.pubkey(), 500, 6)],
        &[&vault_owner],
    );
    p.same(
        "DAS.6",
        "withdraw after the freeze authority freezes the vault",
        "ix 0: AccountFrozen (0x11)",
        r,
    );
    let update = default_account_state::instruction::update_default_account_state(
        &T22,
        &mint,
        &issuer,
        &[],
        &AccountState::Initialized,
    )
    .unwrap();
    let r = p.probe(&[update], &[]);
    let later_owner = p.addr("later_owner");
    let later = p.token_account("later_account", &mint, &later_owner, &[]);
    let lv = p.view(&later);
    p.same(
        "DAS.7",
        "freeze authority sets the default to Initialized; a new account",
        "ok; new account initialized",
        format!("{r}; new account {}", status(&lv)),
    );
}

#[test]
fn memo_transfer() {
    let mut p = Probe::new("memo_transfer", "MemoTransfer (required memo)");
    let m = p.factory_mint("mint", MintConfig::token_2022("plain", 6));
    let mint = m.address;
    let alice = p.key("alice");
    let vault_owner = p.key("vault_owner");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let v = p.token_account(
        "vault",
        &mint,
        &vault_owner.pubkey(),
        &[ExtensionType::MemoTransfer],
    );
    let enable = memo_transfer::instruction::enable_required_transfer_memos(
        &T22,
        &v,
        &vault_owner.pubkey(),
        &[],
    )
    .unwrap();
    p.setup(
        "enable required memos on the vault",
        &[enable],
        &[&vault_owner],
    );
    p.mint_factory_tokens(&m, &a, 10_000);

    let deposit = xfer(&a, &mint, &v, &alice.pubkey(), 1_000, 6);
    let r = p.probe(std::slice::from_ref(&deposit), &[&alice]);
    p.same(
        "MEMO.1",
        "deposit into a memo vault, no memo",
        "ix 0: NoMemo (0x24)",
        r,
    );
    let r = p.probe(&[memo("deposit"), deposit.clone()], &[&alice]);
    p.same("MEMO.2", "deposit with the memo directly before", "ok", r);
    let r = p.probe(&[deposit, memo("deposit")], &[&alice]);
    p.same(
        "MEMO.3",
        "deposit with the memo after",
        "ix 0: NoMemo (0x24)",
        r,
    );
    let withdraw = xfer(&v, &mint, &a, &vault_owner.pubkey(), 100, 6);
    let r = p.probe(std::slice::from_ref(&withdraw), &[&vault_owner]);
    p.same("MEMO.4", "transfer out of the memo vault, no memo", "ok", r);

    let before = p.len(&a);
    let realloc = ti::reallocate(
        &T22,
        &a,
        &p.payer.pubkey(),
        &alice.pubkey(),
        &[],
        &[ExtensionType::MemoTransfer],
    )
    .unwrap();
    let r = p.probe(&[realloc], &[&alice]);
    p.same(
        "MEMO.5",
        "user reallocates own account for MemoTransfer; size",
        "ok; 165 -> 171",
        format!("{r}; {before} -> {}", p.len(&a)),
    );
    let enable =
        memo_transfer::instruction::enable_required_transfer_memos(&T22, &a, &alice.pubkey(), &[])
            .unwrap();
    p.setup("user enables required memos", &[enable], &[&alice]);
    let r = p.probe(std::slice::from_ref(&withdraw), &[&vault_owner]);
    p.same(
        "MEMO.6",
        "protocol withdraws to that user, no memo",
        "ix 0: NoMemo (0x24)",
        r,
    );
    let r = p.probe(&[memo("withdraw"), withdraw], &[&vault_owner]);
    p.same("MEMO.7", "protocol withdraws with the memo first", "ok", r);
}

#[test]
fn cpi_guard_top_level() {
    let mut p = Probe::new("cpi_guard_top_level", "CpiGuard");
    let m = p.factory_mint("mint", MintConfig::token_2022("plain", 6));
    let mint = m.address;
    let owner = p.key("owner");
    let bob = p.key("bob");
    let a = p.token_account(
        "owner_account",
        &mint,
        &owner.pubkey(),
        &[ExtensionType::CpiGuard],
    );
    let b = create_actor_token_account(&mut p.svm, &m, &p.payer, &bob, &p.issuer).unwrap();
    let before = p.view(&a).extensions;
    let enable = cpi_guard::instruction::enable_cpi_guard(&T22, &a, &owner.pubkey(), &[]).unwrap();
    p.setup("enable CPI Guard", &[enable], &[&owner]);
    let after = p.view(&a).extensions;
    p.same(
        "CPIG.1",
        "account extensions before and after enable_cpi_guard; size",
        "[] -> [CpiGuard], 171",
        format!("{} -> {}, {}", names(&before), names(&after), p.len(&a)),
    );
    p.mint_factory_tokens(&m, &a, 1_000);
    let r = p.probe(&[xfer(&a, &mint, &b, &owner.pubkey(), 100, 6)], &[&owner]);
    p.same("CPIG.2", "top level: owner transfer", "ok", r);
    let delegate = p.addr("delegate");
    let approve = ti::approve(&T22, &a, &delegate, &owner.pubkey(), &[], 100).unwrap();
    let r = p.probe(&[approve], &[&owner]);
    p.same("CPIG.3", "top level: Approve", "ok", r);
    let new_owner = p.addr("new_owner");
    let set = ti::set_authority(
        &T22,
        &a,
        Some(&new_owner),
        ti::AuthorityType::AccountOwner,
        &owner.pubkey(),
        &[],
    )
    .unwrap();
    let r = p.probe(&[set], &[&owner]);
    p.same(
        "CPIG.4",
        "top level: SetAuthority(AccountOwner)",
        "ix 0: CpiGuardOwnerChangeBlocked (0x2f)",
        r,
    );
}

#[test]
fn mint_close_authority() {
    let mut p = Probe::new("mint_close_authority", "MintCloseAuthority");
    let issuer = p.issuer.pubkey();
    let payer = p.payer.pubkey();
    let mint_key = p.key("mint");
    let mint = mint_key.pubkey();
    let init = ti::initialize_mint_close_authority(&T22, &mint, Some(&issuer)).unwrap();
    p.raw_mint(
        "mint",
        &[ExtensionType::MintCloseAuthority],
        vec![init],
        6,
        0,
    );
    let alice = p.key("alice");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    p.mint_to(&mint, &a, 100);
    let close = ti::close_account(&T22, &mint, &payer, &issuer, &[]).unwrap();
    let r = p.probe(std::slice::from_ref(&close), &[]);
    p.same(
        "MCA.1",
        "close the mint at supply 100",
        "ix 0: MintHasSupply (0x1c)",
        r,
    );
    let burn = ti::burn_checked(&T22, &a, &mint, &alice.pubkey(), &[], 100, 6).unwrap();
    p.setup("burn all", &[burn], &[&alice]);
    let r = p.probe(&[close], &[]);
    let gone = |kept: bool| if kept { "kept" } else { "gone" };
    p.same(
        "MCA.2",
        "close the mint at supply 0; mint account, token account",
        "ok; mint gone, token account kept",
        format!(
            "{r}; mint {}, token account {}",
            gone(p.exists(&mint)),
            gone(p.exists(&a))
        ),
    );

    // Re-create a different mint at the same address: decimals 6 -> 0, plus a
    // 10% fee.
    let i1 = ti::initialize_mint_close_authority(&T22, &mint, Some(&issuer)).unwrap();
    let i2 = transfer_fee::instruction::initialize_transfer_fee_config(
        &T22,
        &mint,
        Some(&issuer),
        Some(&issuer),
        1_000,
        u64::MAX,
    )
    .unwrap();
    let types = [
        ExtensionType::MintCloseAuthority,
        ExtensionType::TransferFeeConfig,
    ];
    let ixs = p.mint_ixs(&mint, mint_len(&types), vec![i1, i2], 0, false, 0);
    let r = p.probe(&ixs, &[&mint_key]);
    let old = p.view(&a);
    p.same(
        "MCA.3",
        "re-create at the same address with decimals 0 and a 10% fee; old account extensions",
        "ok; []",
        format!("{r}; {}", names(&old.extensions)),
    );
    let r = p.probe(
        &[ti::mint_to(&T22, &mint, &a, &issuer, &[], 1_000).unwrap()],
        &[],
    );
    p.same(
        "MCA.4",
        "mint_to the old account under the new mint",
        "ok",
        r,
    );
    let owner2 = p.key("owner2");
    let fresh = p.token_account("fresh_account", &mint, &owner2.pubkey(), &[]);
    p.mint_to(&mint, &fresh, 1_000);
    let r = p.probe(
        &[xfer(&fresh, &mint, &a, &owner2.pubkey(), 1_000, 0)],
        &[&owner2],
    );
    p.same(
        "MCA.5",
        "fee-bearing transfer into the old account (no TransferFeeAmount); old balance",
        "ix 0: InvalidState (0xd); 1000",
        format!("{r}; {}", p.view(&a).amount),
    );
    let r = p.probe(&[xfer(&a, &mint, &fresh, &alice.pubkey(), 1, 0)], &[&alice]);
    p.same(
        "MCA.6",
        "transfer out of the old account under the new mint",
        "ok",
        r,
    );
}

/// Create a mint with only `MintCloseAuthority` (decimals 0) and one
/// zero-balance account, close the mint, and re-create it at the same address
/// with `MintCloseAuthority` plus `types`, initialized by `init(mint, issuer)`.
/// Returns the mint, the old account and its owner.
fn resurrect(
    p: &mut Probe,
    case: &str,
    types: &[ExtensionType],
    init: impl FnOnce(&Pubkey, &Pubkey) -> Vec<Instruction>,
    freeze: bool,
) -> (Pubkey, Pubkey, Keypair) {
    let mint_key = p.key(&format!("{case}/mint"));
    let mint = mint_key.pubkey();
    let issuer = p.issuer.pubkey();
    let payer = p.payer.pubkey();
    let close_authority = ti::initialize_mint_close_authority(&T22, &mint, Some(&issuer)).unwrap();
    let ixs = p.mint_ixs(
        &mint,
        mint_len(&[ExtensionType::MintCloseAuthority]),
        vec![close_authority.clone()],
        0,
        false,
        0,
    );
    p.setup("mint v1", &ixs, &[&mint_key]);
    let old_owner = p.key(&format!("{case}/old_owner"));
    let old = p.token_account(
        &format!("{case}/old_account"),
        &mint,
        &old_owner.pubkey(),
        &[],
    );
    let close = ti::close_account(&T22, &mint, &payer, &issuer, &[]).unwrap();
    p.setup("close mint v1", &[close], &[]);
    let mut all = vec![ExtensionType::MintCloseAuthority];
    all.extend_from_slice(types);
    let mut pre = vec![close_authority];
    pre.extend(init(&mint, &issuer));
    let ixs = p.mint_ixs(&mint, mint_len(&all), pre, 0, freeze, 0);
    p.setup("mint v2 at the same address", &ixs, &[&mint_key]);
    (mint, old, old_owner)
}

#[test]
fn mint_resurrection() {
    let mut p = Probe::new(
        "mint_resurrection",
        "MintCloseAuthority, resurrection table",
    );
    let issuer = p.issuer.pubkey();

    let (mint, old, old_owner) = resurrect(
        &mut p,
        "frozen_default",
        &[ExtensionType::DefaultAccountState],
        |m, _| {
            vec![
                default_account_state::instruction::initialize_default_account_state(
                    &T22,
                    m,
                    &AccountState::Frozen,
                )
                .unwrap(),
            ]
        },
        true,
    );
    let fresh_owner = p.addr("frozen_default/fresh_owner");
    let fresh = p.token_account("frozen_default/fresh_account", &mint, &fresh_owner, &[]);
    let (ov, fv) = (p.view(&old), p.view(&fresh));
    p.same(
        "MCA.7",
        "re-created with DefaultAccountState(Frozen): old and new account state",
        "old initialized, new frozen",
        format!("old {}, new {}", status(&ov), status(&fv)),
    );
    let r = p.probe(
        &[ti::mint_to(&T22, &mint, &old, &issuer, &[], 100).unwrap()],
        &[],
    );
    p.same("MCA.8", "frozen default: mint_to the old account", "ok", r);
    let sink_owner = p.addr("frozen_default/sink_owner");
    let sink = p.token_account("frozen_default/sink", &mint, &sink_owner, &[]);
    p.setup(
        "thaw sink",
        &[ti::thaw_account(&T22, &sink, &mint, &issuer, &[]).unwrap()],
        &[],
    );
    let r = p.probe(
        &[xfer(&old, &mint, &sink, &old_owner.pubkey(), 100, 0)],
        &[&old_owner],
    );
    p.same(
        "MCA.9",
        "frozen default: the never-thawed old account transfers out",
        "ok",
        r,
    );

    let (mint, old, _) = resurrect(
        &mut p,
        "non_transferable",
        &[ExtensionType::NonTransferable],
        |m, _| vec![ti::initialize_non_transferable_mint(&T22, m).unwrap()],
        false,
    );
    let r = p.probe(
        &[ti::mint_to(&T22, &mint, &old, &issuer, &[], 100).unwrap()],
        &[],
    );
    p.same(
        "MCA.10",
        "re-created NonTransferable: mint_to the old account",
        "ix 0: NonTransferableNeedsImmutableOwnership (0x26)",
        r,
    );

    let (mint, old, old_owner) = resurrect(
        &mut p,
        "transfer_fee",
        &[ExtensionType::TransferFeeConfig],
        |m, i| {
            vec![transfer_fee::instruction::initialize_transfer_fee_config(
                &T22,
                m,
                Some(i),
                Some(i),
                1_000,
                u64::MAX,
            )
            .unwrap()]
        },
        false,
    );
    p.mint_to(&mint, &old, 1_000);
    let dst_owner = p.addr("transfer_fee/dst_owner");
    let dst = p.token_account("transfer_fee/dst", &mint, &dst_owner, &[]);
    let r = p.probe(
        &[xfer(&old, &mint, &dst, &old_owner.pubkey(), 1_000, 0)],
        &[&old_owner],
    );
    let dv = p.view(&dst);
    p.same(
        "MCA.11",
        "re-created with a 10% fee: transfer 1,000 out of the old account; destination",
        "ok; 900 (+100 withheld)",
        format!("{r}; {} (+{} withheld)", dv.amount, dv.withheld),
    );

    let missing_hook = p.addr("transfer_hook/missing_hook_program");
    let (mint, old, old_owner) = resurrect(
        &mut p,
        "transfer_hook",
        &[ExtensionType::TransferHook],
        move |m, i| {
            vec![
                transfer_hook::instruction::initialize(&T22, m, Some(*i), Some(missing_hook))
                    .unwrap(),
            ]
        },
        false,
    );
    p.mint_to(&mint, &old, 1_000);
    let dst_owner = p.addr("transfer_hook/dst_owner");
    let dst = p.token_account("transfer_hook/dst", &mint, &dst_owner, &[]);
    let r = p.probe(
        &[xfer(&old, &mint, &dst, &old_owner.pubkey(), 10, 0)],
        &[&old_owner],
    );
    p.same(
        "MCA.12",
        "re-created with a transfer hook: transfer out of the old account",
        "ix 0: InvalidAccountData",
        r,
    );
}

#[test]
fn interest_bearing_config() {
    let mut p = Probe::new("interest_bearing_config", "InterestBearingConfig");
    let issuer = p.issuer.pubkey();
    let mint = p.addr("mint");
    let init =
        interest_bearing_mint::instruction::initialize(&T22, &mint, Some(issuer), 500).unwrap();
    p.raw_mint(
        "mint",
        &[ExtensionType::InterestBearingConfig],
        vec![init],
        6,
        0,
    );
    let holder = p.addr("holder");
    let a = p.token_account("holder_account", &mint, &holder, &[]);
    p.mint_to(&mint, &a, 1_000_000_000);
    let ui = p.ui_amount(&mint, 1_000_000_000);
    p.same(
        "IB.1",
        "5%, at start: raw balance, UI amount",
        "raw 1000000000, ui 1000",
        format!("raw {}, ui {ui}", p.view(&a).amount),
    );
    p.warp(YEAR_SECONDS, 0);
    let ui = p.ui_amount(&mint, 1_000_000_000);
    p.same(
        "IB.2",
        "after one simulated year: raw balance, UI amount",
        "raw 1000000000, ui 1051.236557",
        format!("raw {}, ui {ui}", p.view(&a).amount),
    );
    let raw = p.raw_amount(&mint, "1000");
    p.same(
        "IB.3",
        "after one year: UiAmountToAmount(\"1000\")",
        "951260678",
        raw.to_string(),
    );
    let update =
        interest_bearing_mint::instruction::update_rate(&T22, &mint, &issuer, &[], -1_000).unwrap();
    let r = p.probe(&[update], &[]);
    p.warp(YEAR_SECONDS, 0);
    let ui = p.ui_amount(&mint, 1_000_000_000);
    p.same(
        "IB.4",
        "rate authority sets -1000 bps; UI amount a year later",
        "ok; ui 951.260678",
        format!("{r}; ui {ui}"),
    );
}

#[test]
fn scaled_ui_amount() {
    let mut p = Probe::new("scaled_ui_amount", "ScaledUiAmount; InterestBearingConfig");
    let issuer = p.issuer.pubkey();
    let mint = p.addr("mint");
    let init = scaled_ui_amount::instruction::initialize(&T22, &mint, Some(issuer), 1.0).unwrap();
    p.raw_mint("mint", &[ExtensionType::ScaledUiAmount], vec![init], 6, 0);
    let holder = p.addr("holder");
    let a = p.token_account("holder_account", &mint, &holder, &[]);
    p.mint_to(&mint, &a, 1_000_000);
    let ui = p.ui_amount(&mint, 1_000_000);
    p.same(
        "SUA.1",
        "multiplier 1.0: raw balance, UI amount",
        "raw 1000000, ui 1",
        format!("raw {}, ui {ui}", p.view(&a).amount),
    );
    let now = p.svm.get_sysvar::<Clock>().unix_timestamp;
    let update = scaled_ui_amount::instruction::update_multiplier(
        &T22,
        &mint,
        &issuer,
        &[],
        2.0,
        now + 1_000,
    )
    .unwrap();
    let r = p.probe(&[update], &[]);
    let ui = p.ui_amount(&mint, 1_000_000);
    p.same(
        "SUA.2",
        "schedule multiplier 2.0 at now + 1000 s; UI amount before then",
        "ok; ui 1",
        format!("{r}; ui {ui}"),
    );
    p.warp(1_000, 0);
    let ui = p.ui_amount(&mint, 1_000_000);
    let raw = p.raw_amount(&mint, "1");
    p.same(
        "SUA.3",
        "after the effective time: raw, UI amount, UiAmountToAmount(\"1\")",
        "raw 1000000, ui 2, 500000",
        format!("raw {}, ui {ui}, {raw}", p.view(&a).amount),
    );

    let both = p.key("mint_with_both");
    let i1 =
        scaled_ui_amount::instruction::initialize(&T22, &both.pubkey(), Some(issuer), 1.0).unwrap();
    let i2 =
        interest_bearing_mint::instruction::initialize(&T22, &both.pubkey(), Some(issuer), 100)
            .unwrap();
    let types = [
        ExtensionType::ScaledUiAmount,
        ExtensionType::InterestBearingConfig,
    ];
    let ixs = p.mint_ixs(&both.pubkey(), mint_len(&types), vec![i1, i2], 6, false, 0);
    let r = p.probe(&ixs, &[&both]);
    p.same(
        "SUA.4",
        "ScaledUiAmount + InterestBearingConfig on one mint",
        "ix 3: InvalidExtensionCombination (0x33)",
        r,
    );
}

#[test]
fn non_transferable() {
    let mut p = Probe::new("non_transferable", "NonTransferable");
    let m = p.factory_mint(
        "mint",
        MintConfig::token_2022("non_transferable", 6).with(MintExtension::NonTransferable),
    );
    let mint = m.address;
    let alice = p.key("alice");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let vault_owner = p.addr("vault_owner");
    let v = p.token_account("vault", &mint, &vault_owner, &[]);
    let av = p.view(&a);
    p.same(
        "NT.1",
        "token account: extensions, size",
        "[NonTransferableAccount, ImmutableOwner], 174",
        format!("{}, {}", names(&av.extensions), p.len(&a)),
    );
    p.mint_factory_tokens(&m, &a, 1_000);
    let r = p.probe(&[xfer(&a, &mint, &v, &alice.pubkey(), 1_000, 6)], &[&alice]);
    p.same(
        "NT.2",
        "deposit with transfer_checked",
        "ix 0: NonTransferable (0x25)",
        r,
    );
    let burn = ti::burn_checked(&T22, &a, &mint, &alice.pubkey(), &[], 1_000, 6).unwrap();
    let r = p.probe(&[burn], &[&alice]);
    p.same("NT.3", "owner burns", "ok", r);
    let close = ti::close_account(&T22, &a, &p.payer.pubkey(), &alice.pubkey(), &[]).unwrap();
    let r = p.probe(&[close], &[&alice]);
    p.same("NT.4", "owner closes the account after the burn", "ok", r);
    let k = p.key("account_165");
    let ixs = p.account_ixs(&k.pubkey(), &mint, &vault_owner, &[], Some(165));
    let r = p.probe(&ixs, &[&k]);
    p.same(
        "NT.5",
        "InitializeAccount3 on a 165-byte account",
        "ix 1: InvalidAccountData",
        r,
    );
}

#[test]
fn immutable_owner() {
    let mut p = Probe::new("immutable_owner", "ImmutableOwner; finding 2");
    let m = p.factory_mint("mint", MintConfig::token_2022("plain", 6));
    let mint = m.address;
    let owner = p.key("owner");
    let plain = p.token_account("plain_account", &mint, &owner.pubkey(), &[]);
    let immutable = p.token_account(
        "immutable_account",
        &mint,
        &owner.pubkey(),
        &[ExtensionType::ImmutableOwner],
    );
    // Stands in for the PDA an escrow would hand the account to.
    let pda = p.addr("escrow_pda");
    let set_owner = |account: &Pubkey, current: &Pubkey| {
        ti::set_authority(
            &T22,
            account,
            Some(&pda),
            ti::AuthorityType::AccountOwner,
            current,
            &[],
        )
        .unwrap()
    };
    let r = p.probe(&[set_owner(&plain, &owner.pubkey())], &[&owner]);
    p.same(
        "IO.1",
        "SetAuthority(AccountOwner), account without ImmutableOwner",
        "ok",
        r,
    );
    let r = p.probe(&[set_owner(&immutable, &owner.pubkey())], &[&owner]);
    p.same(
        "IO.2",
        "SetAuthority(AccountOwner), ImmutableOwner account",
        "ix 0: ImmutableOwner (0x22)",
        r,
    );

    let wallet = p.key("wallet");
    let payer = p.payer.pubkey();
    let create = create_associated_token_account(&payer, &wallet.pubkey(), &mint, &T22);
    let r = p.probe(&[create], &[]);
    let ata = m.ata(&wallet.pubkey());
    let view = p.view(&ata);
    p.same(
        "IO.3",
        "create a Token-2022 ATA; its extensions",
        "ok; [ImmutableOwner]",
        format!("{r}; {}", names(&view.extensions)),
    );
    let r = p.probe(&[set_owner(&ata, &wallet.pubkey())], &[&wallet]);
    p.same(
        "IO.4",
        "SetAuthority(AccountOwner) on the Token-2022 ATA",
        "ix 0: ImmutableOwner (0x22)",
        r,
    );

    // The hook program id does not affect sizing; research/02's probe used
    // None, the factory needs an id.
    let hook = p.addr("hook_program");
    let multi = p.factory_mint(
        "multi_mint",
        MintConfig::token_2022("fee_pausable_hook", 6)
            .with(MintExtension::TransferFee {
                basis_points: 100,
                maximum_fee: 1_000,
            })
            .with(MintExtension::Pausable)
            .with(MintExtension::TransferHook { program_id: hook }),
    );
    let n = p.account_data_size(&multi.address, &[]);
    let n_io = p.account_data_size(&multi.address, &[ExtensionType::ImmutableOwner]);
    let host = account_len(&ExtensionType::get_required_init_account_extensions(
        &p.mint_types(&multi.address),
    ));
    p.same(
        "IO.5",
        "GetAccountDataSize, TransferFee + Pausable + TransferHook mint: plain, + ImmutableOwner, host calculation",
        "187, 191, 187",
        format!("{n}, {n_io}, {host}"),
    );
    let create =
        create_associated_token_account_idempotent(&payer, &wallet.pubkey(), &multi.address, &T22);
    let r = p.probe(&[create], &[]);
    let ata = get_associated_token_address_with_program_id(&wallet.pubkey(), &multi.address, &T22);
    let view = p.view(&ata);
    p.same(
        "IO.6",
        "create that mint's ATA (idempotent); extensions, size",
        "ok; [ImmutableOwner, TransferFeeAmount, PausableAccount, TransferHookAccount], 191",
        format!("{r}; {}, {}", names(&view.extensions), p.len(&ata)),
    );
}

#[test]
fn confidential_transfer() {
    let mut p = Probe::new(
        "confidential_transfer",
        "ConfidentialTransfer; NonTransferable",
    );
    let issuer = p.issuer.pubkey();
    let mint = p.addr("mint");
    let init =
        confidential_transfer::instruction::initialize_mint(&T22, &mint, Some(issuer), true, None)
            .unwrap();
    p.raw_mint(
        "mint",
        &[ExtensionType::ConfidentialTransferMint],
        vec![init],
        6,
        0,
    );
    let alice = p.key("alice");
    let a = p.token_account("alice_account", &mint, &alice.pubkey(), &[]);
    let vault_owner = p.addr("vault_owner");
    let v = p.token_account("vault", &mint, &vault_owner, &[]);
    let av = p.view(&a);
    p.same(
        "CT.1",
        "unconfigured token account: extensions, size",
        "[], 165",
        format!("{}, {}", names(&av.extensions), p.len(&a)),
    );
    p.mint_to(&mint, &a, 1_000);
    let r = p.probe(&[xfer(&a, &mint, &v, &alice.pubkey(), 1_000, 6)], &[&alice]);
    p.same(
        "CT.2",
        "public transfer_checked between unconfigured accounts",
        "ok",
        r,
    );

    let with_fee = p.key("mint_with_fee");
    let i1 = confidential_transfer::instruction::initialize_mint(
        &T22,
        &with_fee.pubkey(),
        Some(issuer),
        true,
        None,
    )
    .unwrap();
    let i2 = transfer_fee::instruction::initialize_transfer_fee_config(
        &T22,
        &with_fee.pubkey(),
        Some(&issuer),
        Some(&issuer),
        100,
        1_000,
    )
    .unwrap();
    let types = [
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::TransferFeeConfig,
    ];
    let ixs = p.mint_ixs(
        &with_fee.pubkey(),
        mint_len(&types),
        vec![i1, i2],
        6,
        false,
        0,
    );
    let r = p.probe(&ixs, &[&with_fee]);
    p.same(
        "CT.3",
        "ConfidentialTransferMint + TransferFeeConfig, no ConfidentialTransferFeeConfig",
        "ix 3: InvalidExtensionCombination (0x33)",
        r,
    );
    let soulbound = p.key("mint_non_transferable");
    let i1 = confidential_transfer::instruction::initialize_mint(
        &T22,
        &soulbound.pubkey(),
        Some(issuer),
        true,
        None,
    )
    .unwrap();
    let i2 = ti::initialize_non_transferable_mint(&T22, &soulbound.pubkey()).unwrap();
    let types = [
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::NonTransferable,
    ];
    let ixs = p.mint_ixs(
        &soulbound.pubkey(),
        mint_len(&types),
        vec![i1, i2],
        6,
        false,
        0,
    );
    let r = p.probe(&ixs, &[&soulbound]);
    // 10.0.0 has no rule for this pair, and neither has
    // `check_for_invalid_mint_extension_combinations` in interface 2.1.0. The
    // rule is in 3.1.1.
    assert!(ExtensionType::check_for_invalid_mint_extension_combinations(&types).is_ok());
    p.differs(
        "CT.4",
        "ConfidentialTransferMint + NonTransferable, no ConfidentialMintBurn",
        "ix 3: InvalidExtensionCombination (0x33)",
        "ok",
        r,
    );
}

/// spl-token-metadata-interface `Initialize`: discriminator, then the Borsh
/// encoding of name, symbol and uri (u32 length + bytes each).
fn initialize_token_metadata(mint: &Pubkey, authority: &Pubkey, fields: [&str; 3]) -> Instruction {
    let mut data = discriminator("spl_token_metadata_interface:initialize_account").to_vec();
    for field in fields {
        data.extend_from_slice(&(field.len() as u32).to_le_bytes());
        data.extend_from_slice(field.as_bytes());
    }
    Instruction {
        program_id: T22,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new_readonly(*authority, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

#[test]
fn metadata_pointer() {
    let mut p = Probe::new("metadata_pointer", "MetadataPointer / TokenMetadata");
    let issuer = p.issuer.pubkey();
    let fields = ["ExtensionLab Test", "EXT", "https://example.com/m.json"];
    let tlv = 4 + 32 + 32 + fields.iter().map(|f| 4 + f.len()).sum::<usize>() + 4;
    for (id, label, extra_rent, want) in [
        (
            "MD.1",
            "funded",
            tlv,
            "ok; 234 -> 364, [MetadataPointer, TokenMetadata]",
        ),
        (
            // research/02's probe reported `InsufficientFundsForRent {
            // account_index: 1 }`; index 1 was the mint in its transaction.
            "MD.2",
            "underfunded",
            0,
            "InsufficientFundsForRent (the mint); 234 -> 234, [MetadataPointer]",
        ),
    ] {
        let mint = p.addr(&format!("mint_{label}"));
        let pointer =
            metadata_pointer::instruction::initialize(&T22, &mint, Some(issuer), Some(mint))
                .unwrap();
        p.raw_mint(
            &format!("mint_{label}"),
            &[ExtensionType::MetadataPointer],
            vec![pointer],
            6,
            extra_rent,
        );
        let before = p.len(&mint);
        let ix = initialize_token_metadata(&mint, &issuer, fields);
        let keys = Message::new(std::slice::from_ref(&ix), Some(&p.payer.pubkey())).account_keys;
        let result = p.send(&[ix], &[]);
        let r = match &result {
            Err(f) => match f.err {
                TransactionError::InsufficientFundsForRent { account_index } => format!(
                    "InsufficientFundsForRent ({})",
                    if keys[account_index as usize] == mint {
                        "the mint"
                    } else {
                        "another account"
                    }
                ),
                _ => outcome(&result),
            },
            Ok(_) => outcome(&result),
        };
        let view = read_mint(&p.svm, &mint).unwrap();
        p.same(
            id,
            &format!("initialize TokenMetadata in the mint, {label} rent (TLV {tlv} bytes); size, extensions"),
            want,
            format!("{r}; {before} -> {}, {}", p.len(&mint), names(&view.extensions)),
        );
    }

    let mint = p.key("mint_foreign_pointer");
    let foreign = p.addr("foreign_metadata");
    let pointer = metadata_pointer::instruction::initialize(
        &T22,
        &mint.pubkey(),
        Some(issuer),
        Some(foreign),
    )
    .unwrap();
    let ixs = p.mint_ixs(
        &mint.pubkey(),
        mint_len(&[ExtensionType::MetadataPointer]),
        vec![pointer],
        6,
        false,
        0,
    );
    let r = p.probe(&ixs, &[&mint]);
    p.same("MD.3", "pointer to an arbitrary foreign address", "ok", r);
    let holder = p.key("holder");
    let receiver = p.addr("receiver");
    let a = p.token_account("holder_account", &mint.pubkey(), &holder.pubkey(), &[]);
    let b = p.token_account("receiver_account", &mint.pubkey(), &receiver, &[]);
    p.mint_to(&mint.pubkey(), &a, 10);
    let r = p.probe(
        &[xfer(&a, &mint.pubkey(), &b, &holder.pubkey(), 10, 6)],
        &[&holder],
    );
    p.same("MD.4", "transfer_checked on the pointer mint", "ok", r);
}

#[test]
fn token_group() {
    let mut p = Probe::new(
        "token_group",
        "GroupPointer / TokenGroup / GroupMemberPointer / TokenGroupMember",
    );
    let issuer = p.issuer.pubkey();
    let group = p.addr("group_mint");
    let pointer =
        group_pointer::instruction::initialize(&T22, &group, Some(issuer), Some(group)).unwrap();
    p.raw_mint(
        "group_mint",
        &[ExtensionType::GroupPointer],
        vec![pointer],
        0,
        200,
    );
    // spl-token-group-interface `InitializeGroup`: discriminator, update
    // authority (32 bytes), max_size (u64 LE).
    let mut data = discriminator("spl_token_group_interface:initialize_token_group").to_vec();
    data.extend_from_slice(issuer.as_ref());
    data.extend_from_slice(&2u64.to_le_bytes());
    let init_group = Instruction {
        program_id: T22,
        accounts: vec![
            AccountMeta::new(group, false),
            AccountMeta::new_readonly(group, false),
            AccountMeta::new_readonly(issuer, true),
        ],
        data,
    };
    let r = p.probe(&[init_group], &[]);
    let view = read_mint(&p.svm, &group).unwrap();
    p.same(
        "GRP.1",
        "initialize_group in the mint, max_size 2; size, extensions",
        "ok; 318, [GroupPointer, TokenGroup]",
        format!("{r}; {}, {}", p.len(&group), names(&view.extensions)),
    );
    let mut results = Vec::new();
    for i in 0..3 {
        let name = format!("member_mint_{i}");
        let member = p.addr(&name);
        let pointer = group_member_pointer::instruction::initialize(
            &T22,
            &member,
            Some(issuer),
            Some(member),
        )
        .unwrap();
        p.raw_mint(
            &name,
            &[ExtensionType::GroupMemberPointer],
            vec![pointer],
            0,
            200,
        );
        // `InitializeMember` carries only its discriminator.
        let init_member = Instruction {
            program_id: T22,
            accounts: vec![
                AccountMeta::new(member, false),
                AccountMeta::new_readonly(member, false),
                AccountMeta::new_readonly(issuer, true),
                AccountMeta::new(group, false),
                AccountMeta::new_readonly(issuer, true),
            ],
            data: discriminator("spl_token_group_interface:initialize_member").to_vec(),
        };
        results.push(p.probe(&[init_member], &[]));
    }
    p.same(
        "GRP.2",
        "initialize_member #0 and #1",
        "ok, ok",
        format!("{}, {}", results[0], results[1]),
    );
    p.same(
        "GRP.3",
        "initialize_member #2 (SizeExceedsMaxSize)",
        "ix 0: Custom(3406457177)",
        results[2].clone(),
    );
}

#[test]
fn initialization_order() {
    let mut p = Probe::new(
        "initialization_order",
        "Account size reference, ordering rules",
    );
    let issuer = p.issuer.pubkey();
    let payer = p.payer.pubkey();
    let fee_init = |m: &Pubkey| {
        transfer_fee::instruction::initialize_transfer_fee_config(
            &T22,
            m,
            Some(&issuer),
            Some(&issuer),
            100,
            1_000,
        )
        .unwrap()
    };
    let space = mint_len(&[ExtensionType::TransferFeeConfig]);

    let mint = p.key("mint_init_after");
    let m = mint.pubkey();
    let lamports = p.svm.minimum_balance_for_rent_exemption(space);
    let ixs = [
        create_account(&payer, &m, lamports, space as u64, &T22),
        ti::initialize_mint2(&T22, &m, &issuer, None, 6).unwrap(),
        fee_init(&m),
    ];
    let r = p.probe(&ixs, &[&mint]);
    p.same(
        "ORDER.1",
        "InitializeMint2 before the transfer fee initializer (account sized for it)",
        "ix 1: InvalidAccountData",
        r,
    );

    let mint = p.key("mint_82_bytes");
    let m = mint.pubkey();
    let lamports = p.svm.minimum_balance_for_rent_exemption(82);
    let ixs = [
        create_account(&payer, &m, lamports, 82, &T22),
        fee_init(&m),
        ti::initialize_mint2(&T22, &m, &issuer, None, 6).unwrap(),
    ];
    let r = p.probe(&ixs, &[&mint]);
    p.same(
        "ORDER.2",
        "transfer fee initializer on an 82-byte account",
        "ix 1: InvalidAccountData",
        r,
    );

    let mint = p.key("mint_space_unused");
    let ixs = p.mint_ixs(&mint.pubkey(), space, vec![], 6, false, 0);
    let r = p.probe(&ixs, &[&mint]);
    p.same(
        "ORDER.3",
        "space reserved for TransferFeeConfig, never initialized",
        "ix 1: InvalidAccountData",
        r,
    );
}

#[test]
fn permissioned_burn() {
    let mut p = Probe::new("permissioned_burn", "PermissionedBurn");
    // spl-token-2022-interface 2.1.0 has no PermissionedBurn module, so the
    // initializer is built as 3.1.1 encodes it: TokenInstruction 46
    // (PermissionedBurnExtension), sub-instruction 0 (Initialize), then the
    // burn authority. Size as research/02 records it: 165 + 1 + 4 + 32.
    let mint = p.key("mint");
    let burn_authority = p.addr("burn_authority");
    let mut data = vec![46, 0];
    data.extend_from_slice(burn_authority.as_ref());
    let init = Instruction {
        program_id: T22,
        accounts: vec![AccountMeta::new(mint.pubkey(), false)],
        data,
    };
    let ixs = p.mint_ixs(&mint.pubkey(), 202, vec![init], 6, false, 0);
    let r = p.probe(&ixs, &[&mint]);
    p.differs(
        "PB.1",
        "create a 202-byte mint with PermissionedBurn",
        "ok",
        "ix 1: InvalidInstruction (0xc)",
        r,
    );
    let reason = "Token-2022 10.0.0 has no PermissionedBurn (PB.1), so no such mint exists";
    p.not_probed(
        "PB.2",
        "owner's standard BurnChecked",
        "ix 0: InvalidInstruction (0xc)",
        reason,
    );
    p.not_probed(
        "PB.3",
        "permissioned BurnChecked, burn authority co-signing",
        "ok",
        reason,
    );
}
