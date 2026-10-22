//! Adapter for `solana-foundation/program-examples`, pinned to [`REVISION`].
//!
//! Run `scripts/fetch_targets.sh program-examples`, then build in
//! `targets/program-examples/tokens/escrow/anchor` with `cargo build-sbf`.
//! Set [`SO_ENV`] to test a different binary. No Anchor dependency or target
//! source modifications are needed: instructions use the upstream account
//! order, standard eight-byte Anchor discriminators and little-endian u64s.
//!
//! Both actors' payout ATAs exist before entry, initially empty and with the
//! matrix's account extensions. This exercises required memos on payouts.
//! Expectations are nominal, so fee-related underpayment remains a failure.

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

pub const DESCRIPTION: &str =
    "Solana Foundation program-examples Anchor escrow: make_offer, take_offer, refund_offer; existing payout ATAs";
pub const REVISION: &str = "de090195e4de698e150572c4f886c333323a64a5";
pub const REPOSITORY: &str = "https://github.com/solana-foundation/program-examples";
pub const PROGRAM_ID: &str = "3o3jk7aj2aGCGsbChkgeNdHcBmeEmGA7BtpnTXdVSgtr";
pub const SO_ENV: &str = "EXTENSIONLAB_PROGRAM_EXAMPLES_ESCROW_SO";
pub const OFFER_ID: u64 = 123;
pub const DEPOSIT: u64 = 10_000_000;
pub const RECEIVE: u64 = 7_000_000;
pub const INITIAL_BALANCE: u64 = 100_000_000;

pub fn program_id() -> Pubkey {
    Pubkey::from_str(PROGRAM_ID).expect("valid upstream program id")
}

pub fn default_so_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../targets/program-examples/tokens/escrow/anchor/target/deploy/escrow.so"
    ))
}

#[derive(Debug, Clone)]
pub struct ProgramExamplesEscrowAdapter {
    pub so_path: PathBuf,
}

impl Default for ProgramExamplesEscrowAdapter {
    fn default() -> Self {
        Self {
            so_path: std::env::var_os(SO_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(default_so_path),
        }
    }
}

pub fn boxed() -> Box<dyn Adapter> {
    Box::new(ProgramExamplesEscrowAdapter::default())
}

struct Accounts {
    maker: Pubkey,
    taker: Pubkey,
    mint_a: Pubkey,
    mint_b: Pubkey,
    token_program: Pubkey,
    offer: Pubkey,
    vault: Pubkey,
    maker_a: Pubkey,
    maker_b: Pubkey,
    taker_a: Pubkey,
    taker_b: Pubkey,
}

impl Accounts {
    fn of(w: &World) -> Result<Self> {
        let (a, b) = (w.mint("mint_a")?, w.mint("mint_b")?);
        let (maker, taker) = (w.pubkey("maker")?, w.pubkey("taker")?);
        let offer = Pubkey::find_program_address(
            &[b"offer", maker.as_ref(), &OFFER_ID.to_le_bytes()],
            &program_id(),
        )
        .0;
        Ok(Self {
            maker,
            taker,
            mint_a: a.address,
            mint_b: b.address,
            token_program: a.program_id(),
            offer,
            vault: a.ata(&offer),
            maker_a: a.ata(&maker),
            maker_b: b.ata(&maker),
            taker_a: a.ata(&taker),
            taker_b: b.ata(&taker),
        })
    }
}

impl Adapter for ProgramExamplesEscrowAdapter {
    fn name(&self) -> &str {
        "program-examples-escrow"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn mint_slots(&self) -> Vec<String> {
        vec!["mint_a".into(), "mint_b".into()]
    }

    fn actors(&self) -> Vec<ActorSpec> {
        [("maker", INITIAL_BALANCE, 0), ("taker", 0, INITIAL_BALANCE)]
            .into_iter()
            .map(|(name, a, b)| ActorSpec {
                name: name.into(),
                lamports: 10_000_000_000,
                token_accounts: [("mint_a", a), ("mint_b", b)]
                    .into_iter()
                    .map(|(slot, initial_balance)| ActorTokenAccount {
                        slot: slot.into(),
                        initial_balance,
                    })
                    .collect(),
            })
            .collect()
    }

    fn flows(&self) -> Vec<FlowSpec> {
        [
            ("make_offer", FlowKind::Entry, "maker deposits token A"),
            (
                "take_offer",
                FlowKind::Exit,
                "taker pays B and receives the vault",
            ),
            ("refund_offer", FlowKind::Exit, "maker withdraws the vault"),
        ]
        .into_iter()
        .map(|(name, kind, description)| FlowSpec {
            name: name.into(),
            kind,
            description: description.into(),
        })
        .collect()
    }

    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()> {
        if !self.so_path.is_file() {
            return Err(Error::Adapter(format!(
                "program-examples escrow binary missing at {}; run scripts/fetch_targets.sh program-examples, then cargo build-sbf in targets/program-examples/tokens/escrow/anchor, or set {SO_ENV}",
                self.so_path.display()
            )));
        }
        svm.add_program_from_file(program_id(), &self.so_path)
            .map_err(|e| Error::Adapter(format!("loading {}: {e}", self.so_path.display())))
    }

    fn programs(&self) -> Vec<ProgramInfo> {
        vec![ProgramInfo {
            name: "program-examples-escrow".into(),
            program_id: program_id(),
            source: format!(
                "{}; expected upstream {REPOSITORY}@{REVISION}:tokens/escrow/anchor",
                self.so_path.display()
            ),
            sha256: std::fs::read(&self.so_path).ok().map(|bytes| {
                solana_sha256_hasher::hash(&bytes)
                    .to_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            }),
        }]
    }

    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        let k = Accounts::of(w)?;
        let writable = |key| AccountMeta::new(key, false);
        let readonly = |key| AccountMeta::new_readonly(key, false);
        let ata = spl_associated_token_account_interface::program::id();
        let system = solana_system_interface::program::id();
        let (payer, accounts) = match flow {
            "make_offer" => (
                "maker",
                vec![
                    AccountMeta::new(k.maker, true),
                    readonly(k.mint_a),
                    readonly(k.mint_b),
                    writable(k.maker_a),
                    writable(k.offer),
                    writable(k.vault),
                    readonly(ata),
                    readonly(k.token_program),
                    readonly(system),
                ],
            ),
            "take_offer" => (
                "taker",
                vec![
                    AccountMeta::new(k.taker, true),
                    writable(k.maker),
                    readonly(k.mint_a),
                    readonly(k.mint_b),
                    writable(k.taker_a),
                    writable(k.taker_b),
                    writable(k.maker_b),
                    writable(k.offer),
                    writable(k.vault),
                    readonly(ata),
                    readonly(k.token_program),
                    readonly(system),
                ],
            ),
            "refund_offer" => (
                "maker",
                vec![
                    AccountMeta::new(k.maker, true),
                    readonly(k.mint_a),
                    writable(k.maker_a),
                    writable(k.offer),
                    writable(k.vault),
                    readonly(k.token_program),
                ],
            ),
            other => return Err(Error::Adapter(format!("unknown flow `{other}`"))),
        };
        let mut data = solana_sha256_hasher::hash(format!("global:{flow}").as_bytes()).to_bytes()
            [..8]
            .to_vec();
        if flow == "make_offer" {
            for arg in [OFFER_ID, DEPOSIT, RECEIVE] {
                data.extend(arg.to_le_bytes());
            }
        }
        Ok(vec![FlowTx {
            payer: payer.into(),
            signers: Vec::new(),
            extra_signers: Vec::new(),
            instructions: vec![Instruction {
                program_id: program_id(),
                accounts,
                data,
            }],
        }])
    }

    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        let k = Accounts::of(w)?;
        Ok([
            ("maker_ata_a", k.maker_a, "mint_a", false),
            ("maker_ata_b", k.maker_b, "mint_b", false),
            ("taker_ata_a", k.taker_a, "mint_a", false),
            ("taker_ata_b", k.taker_b, "mint_b", false),
            ("vault", k.vault, "mint_a", true),
        ]
        .into_iter()
        .map(|(label, address, slot, custody)| TrackedAccount {
            label: label.into(),
            address,
            slot: slot.into(),
            custody,
        })
        .collect())
    }

    fn expected_deltas(&self, flow: &str, _w: &World) -> Result<Vec<ExpectedDelta>> {
        let (d, r) = (DEPOSIT as i128, RECEIVE as i128);
        let deltas = match flow {
            "make_offer" => vec![("maker_ata_a", -d), ("vault", d)],
            "take_offer" => vec![
                ("vault", -d),
                ("taker_ata_a", d),
                ("taker_ata_b", -r),
                ("maker_ata_b", r),
            ],
            "refund_offer" => vec![("vault", -d), ("maker_ata_a", d)],
            other => return Err(Error::Adapter(format!("unknown flow `{other}`"))),
        };
        Ok(deltas
            .into_iter()
            .map(|(label, delta)| ExpectedDelta {
                label: label.into(),
                delta,
            })
            .collect())
    }

    fn closable_accounts(&self, _w: &World) -> Result<Vec<String>> {
        Ok(vec!["vault".into()])
    }
}
