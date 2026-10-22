//! ExtensionLab adapter for the Anchor escrow in `targets/escrow`.
//!
//! Instructions are built from raw bytes rather than the escrow's
//! Anchor-generated types, so this crate does not pull in `anchor-lang` (see
//! the version-lock risk in PLAN.md section 9). The account lists follow the
//! `Make`, `Take`, `Refund` and `Update` structs in
//! `targets/escrow/programs/escrowq32026/src/instructions/`, and the one-byte
//! discriminators follow the `#[instruction(discriminator = N)]` attributes in
//! its `lib.rs` (Anchor 1.1.2 encodes an integer discriminator as `&[N]`).
//!
//! The program binary is `targets/escrow/target/deploy/escrowq32026.so`,
//! built with `cargo build-sbf` in `targets/escrow`. Set
//! `EXTENSIONLAB_ESCROW_SO` to use another path. The report records the
//! binary's path and sha256.
//!
//! Two variants differ only in which payout accounts exist before `take`:
//!
//! - `escrow`: the taker's token A account and the maker's token B account do
//!   not exist; `take` creates them (`init_if_needed`). Account extensions the
//!   harness puts on actor accounts (required memos) are therefore never on a
//!   payout account, and a frozen-by-default mint gives frozen payout accounts.
//! - `escrow-existing-atas`: the harness creates both payout accounts first
//!   (empty, thawed, with the config's account extensions), as for users who
//!   already hold both tokens. This is what tests required memos on payouts.

use extensionlab_core::{
    adapter::{
        ActorSpec, ActorTokenAccount, Adapter, ExpectedDelta, FlowKind, FlowSpec, FlowTx,
        TrackedAccount, World,
    },
    report::ProgramInfo,
    Error, Result,
};
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use std::{path::PathBuf, str::FromStr};

/// Description of the `escrow` variant, shown by `extensionlab adapters` and
/// in reports.
pub const DESCRIPTION: &str =
    "Turbin3 Q3 2026 Anchor escrow (targets/escrow): make, take, refund; take creates the payout accounts";
/// Description of the `escrow-existing-atas` variant.
pub const DESCRIPTION_EXISTING_ATAS: &str =
    "Turbin3 Q3 2026 Anchor escrow (targets/escrow): make, take, refund; payout accounts exist before take";

/// The escrow's program id (`declare_id!` in its `lib.rs`).
pub const PROGRAM_ID: &str = "9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t";

/// Environment variable that overrides the program binary path.
pub const SO_ENV: &str = "EXTENSIONLAB_ESCROW_SO";

/// Escrow id seed used for every case.
pub const SEED: u64 = 123;
/// Token A the maker deposits, in base units (10 tokens at 6 decimals).
pub const DEPOSIT: u64 = 10_000_000;
/// Token B the maker asks for, in base units.
pub const RECEIVE: u64 = 7_000_000;
/// Far-future expiration, the value the escrow's own tests use.
pub const EXPIRATION: i64 = 17_780_206_209;
/// Initial balance of each funded actor account.
pub const INITIAL_BALANCE: u64 = 100_000_000;

/// Program id as a `Pubkey`.
pub fn program_id() -> Pubkey {
    Pubkey::from_str(PROGRAM_ID).expect("valid program id")
}

/// Default path of the program binary.
pub fn default_so_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../targets/escrow/target/deploy/escrowq32026.so"
    ))
}

/// Program binary path: `EXTENSIONLAB_ESCROW_SO`, or [`default_so_path`].
pub fn so_path() -> PathBuf {
    std::env::var_os(SO_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(default_so_path)
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct EscrowAdapter {
    /// Program binary to load.
    pub so_path: PathBuf,
    /// Create the taker's token A account and the maker's token B account
    /// before the first flow (the `escrow-existing-atas` variant).
    pub existing_payout_accounts: bool,
}

impl Default for EscrowAdapter {
    fn default() -> Self {
        Self {
            so_path: so_path(),
            existing_payout_accounts: false,
        }
    }
}

impl EscrowAdapter {
    /// The `escrow` variant, with the binary from [`so_path`].
    pub fn new() -> Self {
        Self::default()
    }

    /// The `escrow-existing-atas` variant.
    pub fn with_existing_payout_accounts() -> Self {
        Self {
            existing_payout_accounts: true,
            ..Self::default()
        }
    }

    /// The escrow PDA of `maker`.
    pub fn escrow_address(maker: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"escrow", maker.as_ref(), &SEED.to_le_bytes()],
            &program_id(),
        )
        .0
    }
}

/// Factory for `extensionlab_cli::Registry::register` (`escrow`).
pub fn boxed() -> Box<dyn Adapter> {
    Box::new(EscrowAdapter::new())
}

/// Factory for `extensionlab_cli::Registry::register`
/// (`escrow-existing-atas`).
pub fn boxed_existing_atas() -> Box<dyn Adapter> {
    Box::new(EscrowAdapter::with_existing_payout_accounts())
}

fn delta(label: &str, delta: i128) -> ExpectedDelta {
    ExpectedDelta {
        label: label.into(),
        delta,
    }
}

/// Addresses every flow needs.
struct Accounts {
    maker: Pubkey,
    taker: Pubkey,
    mint_a: Pubkey,
    mint_b: Pubkey,
    token_program: Pubkey,
    escrow: Pubkey,
    vault: Pubkey,
    maker_ata_a: Pubkey,
    maker_ata_b: Pubkey,
    taker_ata_a: Pubkey,
    taker_ata_b: Pubkey,
}

impl Accounts {
    fn of(w: &World) -> Result<Self> {
        let (a, b) = (w.mint("mint_a")?, w.mint("mint_b")?);
        let (maker, taker) = (w.pubkey("maker")?, w.pubkey("taker")?);
        let escrow = EscrowAdapter::escrow_address(&maker);
        Ok(Self {
            maker,
            taker,
            mint_a: a.address,
            mint_b: b.address,
            // The escrow takes one token program for both mints; the matrix
            // fills the other slot with a config of the same program.
            token_program: a.program_id(),
            escrow,
            vault: a.ata(&escrow),
            maker_ata_a: a.ata(&maker),
            maker_ata_b: b.ata(&maker),
            taker_ata_a: a.ata(&taker),
            taker_ata_b: b.ata(&taker),
        })
    }
}

fn ix(data: Vec<u8>, accounts: Vec<AccountMeta>) -> Instruction {
    Instruction {
        program_id: program_id(),
        accounts,
        data,
    }
}

fn tx(payer: &str, instruction: Instruction) -> Vec<FlowTx> {
    vec![FlowTx {
        payer: payer.into(),
        signers: Vec::new(),
        extra_signers: Vec::new(),
        instructions: vec![instruction],
    }]
}

impl Adapter for EscrowAdapter {
    fn name(&self) -> &str {
        if self.existing_payout_accounts {
            "escrow-existing-atas"
        } else {
            "escrow"
        }
    }

    fn description(&self) -> &str {
        if self.existing_payout_accounts {
            DESCRIPTION_EXISTING_ATAS
        } else {
            DESCRIPTION
        }
    }

    fn mint_slots(&self) -> Vec<String> {
        vec!["mint_a".into(), "mint_b".into()]
    }

    fn actors(&self) -> Vec<ActorSpec> {
        let account = |slot: &str, initial_balance| ActorTokenAccount {
            slot: slot.into(),
            initial_balance,
        };
        let accounts = |funded: &str, payout: &str| {
            let mut v = vec![account(funded, INITIAL_BALANCE)];
            if self.existing_payout_accounts {
                v.push(account(payout, 0));
            }
            v
        };
        vec![
            ActorSpec {
                name: "maker".into(),
                lamports: 10_000_000_000,
                token_accounts: accounts("mint_a", "mint_b"),
            },
            ActorSpec {
                name: "taker".into(),
                lamports: 10_000_000_000,
                token_accounts: accounts("mint_b", "mint_a"),
            },
        ]
    }

    fn flows(&self) -> Vec<FlowSpec> {
        let f = |name: &str, kind, description: &str| FlowSpec {
            name: name.into(),
            kind,
            description: description.into(),
        };
        vec![
            f(
                "make",
                FlowKind::Entry,
                "maker deposits token A into the vault and asks for token B",
            ),
            f(
                "take",
                FlowKind::Exit,
                "taker pays token B to the maker and receives the vault",
            ),
            f("refund", FlowKind::Exit, "maker takes the vault back"),
            // Not in the default scenarios (it is neither an entry nor an
            // exit); available to custom scenarios.
            f(
                "update",
                FlowKind::Other,
                "maker re-prices the escrow (same terms)",
            ),
        ]
    }

    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()> {
        if !self.so_path.is_file() {
            return Err(Error::Adapter(format!(
                "escrow program binary not found at {}; run `cargo build-sbf` in targets/escrow or set {SO_ENV}",
                self.so_path.display()
            )));
        }
        svm.add_program_from_file(program_id(), &self.so_path)
            .map_err(|e| Error::Adapter(format!("loading {}: {e}", self.so_path.display())))
    }

    fn programs(&self) -> Vec<ProgramInfo> {
        let sha256 = std::fs::read(&self.so_path).ok().map(|bytes| {
            solana_sha256_hasher::hash(&bytes)
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        });
        vec![ProgramInfo {
            name: "escrowq32026".into(),
            program_id: program_id(),
            source: self.so_path.display().to_string(),
            sha256,
        }]
    }

    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        let k = Accounts::of(w)?;
        let ata_program = spl_associated_token_account_interface::program::id();
        let system_program = solana_system_interface::program::id();
        Ok(match flow {
            "make" => {
                let mut data = vec![0u8];
                data.extend(SEED.to_le_bytes());
                data.extend(DEPOSIT.to_le_bytes());
                data.extend(RECEIVE.to_le_bytes());
                data.extend(EXPIRATION.to_le_bytes());
                tx(
                    "maker",
                    ix(
                        data,
                        vec![
                            AccountMeta::new(k.maker, true),
                            AccountMeta::new_readonly(k.mint_a, false),
                            AccountMeta::new_readonly(k.mint_b, false),
                            AccountMeta::new(k.maker_ata_a, false),
                            AccountMeta::new(k.escrow, false),
                            AccountMeta::new(k.vault, false),
                            AccountMeta::new_readonly(ata_program, false),
                            AccountMeta::new_readonly(k.token_program, false),
                            AccountMeta::new_readonly(system_program, false),
                        ],
                    ),
                )
            }
            "take" => {
                let mut data = vec![1u8];
                data.extend(RECEIVE.to_le_bytes()); // max_receive
                tx(
                    "taker",
                    ix(
                        data,
                        vec![
                            AccountMeta::new(k.taker, true),
                            AccountMeta::new_readonly(k.mint_a, false),
                            AccountMeta::new_readonly(k.mint_b, false),
                            AccountMeta::new(k.taker_ata_b, false),
                            AccountMeta::new(k.taker_ata_a, false),
                            AccountMeta::new(k.maker, false),
                            AccountMeta::new(k.escrow, false),
                            AccountMeta::new(k.vault, false),
                            AccountMeta::new(k.maker_ata_b, false),
                            AccountMeta::new_readonly(ata_program, false),
                            AccountMeta::new_readonly(k.token_program, false),
                            AccountMeta::new_readonly(system_program, false),
                        ],
                    ),
                )
            }
            "refund" => tx(
                "maker",
                ix(
                    vec![2u8],
                    vec![
                        AccountMeta::new(k.maker, true),
                        AccountMeta::new_readonly(k.mint_a, false),
                        AccountMeta::new(k.maker_ata_a, false),
                        AccountMeta::new(k.escrow, false),
                        AccountMeta::new(k.vault, false),
                        AccountMeta::new_readonly(k.token_program, false),
                        AccountMeta::new_readonly(system_program, false),
                    ],
                ),
            ),
            "update" => {
                // receive: Some(RECEIVE) (unchanged, which the program allows),
                // expiration: None.
                let mut data = vec![3u8, 1];
                data.extend(RECEIVE.to_le_bytes());
                data.push(0);
                tx(
                    "maker",
                    ix(
                        data,
                        vec![
                            AccountMeta::new_readonly(k.maker, true),
                            AccountMeta::new(k.escrow, false),
                        ],
                    ),
                )
            }
            other => return Err(Error::Adapter(format!("unknown flow `{other}`"))),
        })
    }

    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        let k = Accounts::of(w)?;
        let t = |label: &str, address: Pubkey, slot: &str, custody: bool| TrackedAccount {
            label: label.into(),
            address,
            slot: slot.into(),
            custody,
        };
        Ok(vec![
            t("maker_ata_a", k.maker_ata_a, "mint_a", false),
            t("maker_ata_b", k.maker_ata_b, "mint_b", false),
            t("taker_ata_a", k.taker_ata_a, "mint_a", false),
            t("taker_ata_b", k.taker_ata_b, "mint_b", false),
            t("vault", k.vault, "mint_a", true),
        ])
    }

    fn expected_deltas(&self, flow: &str, _w: &World) -> Result<Vec<ExpectedDelta>> {
        let (d, r) = (DEPOSIT as i128, RECEIVE as i128);
        Ok(match flow {
            "make" => vec![delta("maker_ata_a", -d), delta("vault", d)],
            "take" => vec![
                delta("vault", -d),
                delta("taker_ata_a", d),
                delta("taker_ata_b", -r),
                delta("maker_ata_b", r),
            ],
            "refund" => vec![delta("vault", -d), delta("maker_ata_a", d)],
            _ => Vec::new(),
        })
    }

    fn closable_accounts(&self, _w: &World) -> Result<Vec<String>> {
        Ok(vec!["vault".into()])
    }
}
