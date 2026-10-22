//! One-leaf Merkle campaign: create and fund atomically, then claim or claw back.
//! The pinned program sets CLAIM_FEE_USD to zero; no oracle state or external
//! service is needed. Its source and license are fetched unchanged, not vendored.

use extensionlab_core::{
    adapter::{
        ActorSpec, ActorTokenAccount, Adapter, ExpectedDelta, FlowKind, FlowSpec, FlowTx,
        TrackedAccount, World,
    },
    mint::{epoch_transfer_fee, fee_for},
    report::ProgramInfo,
    Error, Result,
};
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use std::path::PathBuf;

pub const DESCRIPTION: &str = "Sablier Merkle airdrop: atomic campaign funding, one-leaf claim and creator clawback; zero claim fee at the pinned revision";
pub const FEE_AWARE_DESCRIPTION: &str = "Sablier Merkle airdrop: fee-aware client grosses up campaign funding and expects the recipient's net payout";
pub const REVISION: &str = "e060b26a6c736a3104a0b24ac069fd53633b3ceb";
pub const REPOSITORY: &str = "https://github.com/sablier-labs/solsab";
pub const PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("7XrxoQejBoGouW4V3aozTSwub7xSDjYqB4Go7YLjF9rV");
pub const SO_ENV: &str = "EXTENSIONLAB_SABLIER_MERKLE_SO";
pub const DEPOSIT: u64 = 10_000_000;
pub const INITIAL_BALANCE: u64 = 100_000_000;
const SLOT: &str = "airdrop";
const NAME: &[u8] = b"ExtensionLab";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FundingPolicy {
    #[default]
    Nominal,
    FeeAware,
}

#[derive(Debug, Clone)]
pub struct SablierMerkleAdapter {
    pub so_path: PathBuf,
    pub funding_policy: FundingPolicy,
}

impl Default for SablierMerkleAdapter {
    fn default() -> Self {
        Self {
            so_path: std::env::var_os(SO_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../targets/solsab/target/deploy/sablier_merkle_instant.so"
                    ))
                }),
            funding_policy: FundingPolicy::Nominal,
        }
    }
}

pub fn boxed() -> Box<dyn Adapter> {
    Box::new(SablierMerkleAdapter::default())
}
pub fn boxed_fee_aware() -> Box<dyn Adapter> {
    Box::new(SablierMerkleAdapter {
        funding_policy: FundingPolicy::FeeAware,
        ..Default::default()
    })
}

impl SablierMerkleAdapter {
    fn funding_amount(&self, world: &World) -> Result<u64> {
        if self.funding_policy == FundingPolicy::Nominal {
            return Ok(DEPOSIT);
        }
        let Some((bps, cap)) = epoch_transfer_fee(&world.svm, &world.mint(SLOT)?.address)? else {
            return Ok(DEPOSIT);
        };
        let fee = spl_token_2022_interface::extension::transfer_fee::TransferFee {
            epoch: 0.into(),
            maximum_fee: cap.into(),
            transfer_fee_basis_points: bps.into(),
        };
        fee.calculate_pre_fee_amount(DEPOSIT).ok_or_else(|| {
            Error::Adapter("funding amount overflows while grossing up transfer fees".into())
        })
    }
    fn received(&self, world: &World, amount: u64) -> Result<u64> {
        Ok(amount
            - epoch_transfer_fee(&world.svm, &world.mint(SLOT)?.address)?
                .map(|(bps, cap)| fee_for(bps, cap, amount))
                .unwrap_or(0))
    }
}

pub fn leaf(recipient: &Pubkey, amount: u64) -> [u8; 32] {
    let inner = solana_keccak_hasher::hashv(&[
        &0u32.to_le_bytes(),
        recipient.as_ref(),
        &amount.to_le_bytes(),
    ]);
    solana_keccak_hasher::hash(inner.as_ref()).to_bytes()
}

pub fn campaign(world: &World) -> Result<Pubkey> {
    Ok(Pubkey::find_program_address(
        &[
            b"campaign",
            world.pubkey("creator")?.as_ref(),
            &leaf(&world.pubkey("recipient")?, DEPOSIT),
            &0u64.to_le_bytes(),
            &0u64.to_le_bytes(),
            NAME,
            world.mint(SLOT)?.address.as_ref(),
        ],
        &PROGRAM_ID,
    )
    .0)
}

fn treasury() -> Pubkey {
    Pubkey::find_program_address(&[b"treasury"], &PROGRAM_ID).0
}
fn vault(world: &World) -> Result<Pubkey> {
    Ok(world.mint(SLOT)?.ata(&campaign(world)?))
}
fn ro(key: Pubkey) -> AccountMeta {
    AccountMeta::new_readonly(key, false)
}
fn rw(key: Pubkey) -> AccountMeta {
    AccountMeta::new(key, false)
}
fn signer(key: Pubkey) -> AccountMeta {
    AccountMeta::new(key, true)
}
fn string(data: &mut Vec<u8>, value: &[u8]) {
    data.extend((value.len() as u32).to_le_bytes());
    data.extend(value);
}
fn instruction(name: &str, accounts: Vec<AccountMeta>, args: Vec<u8>) -> Instruction {
    let mut data =
        solana_sha256_hasher::hash(format!("global:{name}").as_bytes()).to_bytes()[..8].to_vec();
    data.extend(args);
    Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data,
    }
}
fn tx(payer: &str, instructions: Vec<Instruction>) -> FlowTx {
    FlowTx {
        payer: payer.into(),
        signers: Vec::new(),
        extra_signers: Vec::new(),
        instructions,
    }
}
fn delta(label: &str, delta: i128) -> ExpectedDelta {
    ExpectedDelta {
        label: label.into(),
        delta,
    }
}

impl Adapter for SablierMerkleAdapter {
    fn name(&self) -> &str {
        if self.funding_policy == FundingPolicy::FeeAware {
            "sablier-merkle-fee-aware"
        } else {
            "sablier-merkle-instant"
        }
    }
    fn description(&self) -> &str {
        if self.funding_policy == FundingPolicy::FeeAware {
            FEE_AWARE_DESCRIPTION
        } else {
            DESCRIPTION
        }
    }
    fn mint_slots(&self) -> Vec<String> {
        vec![SLOT.into()]
    }
    fn actors(&self) -> Vec<ActorSpec> {
        [("creator", INITIAL_BALANCE), ("recipient", 0)]
            .into_iter()
            .map(|(name, initial_balance)| ActorSpec {
                name: name.into(),
                lamports: 10_000_000_000,
                token_accounts: vec![ActorTokenAccount {
                    slot: SLOT.into(),
                    initial_balance,
                }],
            })
            .collect()
    }
    fn flows(&self) -> Vec<FlowSpec> {
        [
            ("fund_campaign", FlowKind::Entry),
            ("claim", FlowKind::Exit),
            ("clawback", FlowKind::Exit),
        ]
        .into_iter()
        .map(|(name, kind)| FlowSpec {
            name: name.into(),
            kind,
            description: String::new(),
        })
        .collect()
    }
    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()> {
        if !self.so_path.is_file() {
            return Err(Error::Adapter(format!("missing Sablier binary {}; run scripts/fetch_targets.sh solsab and build programs/merkle_instant", self.so_path.display())));
        }
        svm.add_program_from_file(PROGRAM_ID, &self.so_path)
            .map_err(|e| Error::Adapter(e.to_string()))
    }
    fn programs(&self) -> Vec<ProgramInfo> {
        vec![ProgramInfo {
            name: "sablier_merkle_instant".into(),
            program_id: PROGRAM_ID,
            source: format!(
                "{}; expected upstream {REPOSITORY}@{REVISION}:programs/merkle_instant (BUSL-1.1)",
                self.so_path.display()
            ),
            sha256: std::fs::read(&self.so_path).ok().map(|b| {
                solana_sha256_hasher::hash(&b)
                    .to_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            }),
        }]
    }
    fn setup(&mut self, world: &World) -> Result<Vec<FlowTx>> {
        let creator = world.pubkey("creator")?;
        // Oracle keys are unused: this exact revision sets CLAIM_FEE_USD = 0.
        let mut args = creator.to_bytes().to_vec();
        args.extend(solana_system_interface::program::id().as_ref());
        args.extend(creator.as_ref());
        Ok(vec![tx(
            "creator",
            vec![instruction(
                "initialize",
                vec![
                    signer(creator),
                    rw(treasury()),
                    ro(solana_system_interface::program::id()),
                ],
                args,
            )],
        )])
    }
    fn build_flow(&mut self, flow: &str, world: &World) -> Result<Vec<FlowTx>> {
        let creator = world.pubkey("creator")?;
        let recipient = world.pubkey("recipient")?;
        let mint = world.mint(SLOT)?;
        let program = mint.program_id();
        let ata = spl_associated_token_account_interface::program::id();
        let system = solana_system_interface::program::id();
        let campaign = campaign(world)?;
        let vault = vault(world)?;
        Ok(match flow {
            "fund_campaign" => {
                let mut args = leaf(&recipient, DEPOSIT).to_vec();
                args.extend(0u64.to_le_bytes()); // starts at epoch timestamp zero
                args.extend(0u64.to_le_bytes()); // never expires
                string(&mut args, NAME);
                string(&mut args, b"");
                args.extend(DEPOSIT.to_le_bytes());
                args.extend(1u32.to_le_bytes());
                let create = instruction(
                    "create_campaign",
                    vec![
                        signer(creator),
                        ro(mint.address),
                        rw(campaign),
                        rw(vault),
                        ro(program),
                        ro(ata),
                        ro(system),
                    ],
                    args,
                );
                let fund = spl_token_2022_interface::instruction::transfer_checked(
                    &program,
                    &world.ata("creator", SLOT)?,
                    &mint.address,
                    &vault,
                    &creator,
                    &[],
                    self.funding_amount(world)?,
                    mint.config.decimals,
                )
                .map_err(|e| Error::Instruction(e.to_string()))?;
                vec![tx("creator", vec![create, fund])]
            }
            "claim" => {
                let receipt = Pubkey::find_program_address(
                    &[b"claim_receipt", campaign.as_ref(), &0u32.to_le_bytes()],
                    &PROGRAM_ID,
                )
                .0;
                let mut args = 0u32.to_le_bytes().to_vec();
                args.extend(DEPOSIT.to_le_bytes());
                args.extend(0u32.to_le_bytes()); // empty proof for a one-leaf tree
                vec![tx(
                    "recipient",
                    vec![instruction(
                        "claim",
                        vec![
                            signer(recipient),
                            ro(recipient),
                            rw(world.ata("recipient", SLOT)?),
                            rw(treasury()),
                            ro(mint.address),
                            rw(campaign),
                            rw(vault),
                            rw(receipt),
                            ro(program),
                            ro(ata),
                            ro(system),
                            ro(creator),
                            ro(system),
                        ],
                        args,
                    )],
                )]
            }
            "clawback" => vec![tx(
                "creator",
                vec![instruction(
                    "clawback",
                    vec![
                        signer(creator),
                        ro(creator),
                        rw(world.ata("creator", SLOT)?),
                        ro(mint.address),
                        ro(campaign),
                        rw(vault),
                        ro(program),
                        ro(ata),
                        ro(system),
                    ],
                    DEPOSIT.to_le_bytes().to_vec(),
                )],
            )],
            _ => return Err(Error::Adapter(format!("unknown flow {flow}"))),
        })
    }
    fn tracked_accounts(&self, world: &World) -> Result<Vec<TrackedAccount>> {
        Ok([
            ("creator", world.ata("creator", SLOT)?, false),
            ("recipient", world.ata("recipient", SLOT)?, false),
            ("campaign_vault", vault(world)?, true),
        ]
        .into_iter()
        .map(|(label, address, custody)| TrackedAccount {
            label: label.into(),
            address,
            slot: SLOT.into(),
            custody,
        })
        .collect())
    }
    fn expected_deltas(&self, flow: &str, world: &World) -> Result<Vec<ExpectedDelta>> {
        let amount = DEPOSIT as i128;
        let payout = if self.funding_policy == FundingPolicy::FeeAware {
            self.received(world, DEPOSIT)? as i128
        } else {
            amount
        };
        Ok(match flow {
            "fund_campaign" => vec![
                delta("creator", -(self.funding_amount(world)? as i128)),
                delta("campaign_vault", amount),
            ],
            "claim" => vec![delta("campaign_vault", -amount), delta("recipient", payout)],
            "clawback" => vec![delta("campaign_vault", -amount), delta("creator", payout)],
            _ => return Err(Error::Adapter(format!("unknown flow {flow}"))),
        })
    }
}
