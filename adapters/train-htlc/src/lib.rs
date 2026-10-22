//! Train user-token lock, redeem and expired refund. Expectations follow this
//! protocol's measured-net accounting; other adapters' nominal policies stay intact.
//! The fixture uses no payout curve, solver rewards, signatures from a cluster or RPC.

use extensionlab_core::{
    adapter::{
        ActorSpec, ActorTokenAccount, Adapter, Credit, ExpectedDelta, FlowKind, FlowSpec, FlowTx,
        SupportPolicy, TrackedAccount, World,
    },
    matrix::{default_scenarios, IssuerAction, Scenario, Step},
    mint::{epoch_transfer_fee, fee_for, MintConfig},
    report::ProgramInfo,
    Error, Result,
};
use litesvm::LiteSVM;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use std::path::PathBuf;

pub const DESCRIPTION: &str = "Train HTLC: measured-net user token lock, redeem and expired refund; no payout curve or solver rewards";
pub const REVISION: &str = "3dbda6949e35e9095493333a6fff7dfe4ccde1aa";
pub const REPOSITORY: &str = "https://github.com/TrainProtocol/contracts";
pub const PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("2cQYFAiud2LBg3r6MxKPJ1oS83yyrRwDsgxQSwhL97LJ");
pub const SO_ENV: &str = "EXTENSIONLAB_TRAIN_HTLC_SO";
pub const DEPOSIT: u64 = 10_000_000;
pub const INITIAL_BALANCE: u64 = 100_000_000;
pub const SLOT: &str = "token";
pub const FIXTURE_PREIMAGE: [u8; 32] = [7; 32];
const RENT: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");

#[derive(Debug, Clone)]
pub struct TrainHtlcAdapter {
    pub so_path: PathBuf,
}
impl Default for TrainHtlcAdapter {
    fn default() -> Self {
        Self {
            so_path: std::env::var_os(SO_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../../targets/train/chains/solana/target/deploy/train_htlc.so"
                    ))
                }),
        }
    }
}
pub fn boxed() -> Box<dyn Adapter> {
    Box::new(TrainHtlcAdapter::default())
}
pub fn hashlock() -> [u8; 32] {
    solana_sha256_hasher::hash(&FIXTURE_PREIMAGE).to_bytes()
}
pub fn lock_address() -> Pubkey {
    Pubkey::find_program_address(&[b"user_lock", &hashlock()], &PROGRAM_ID).0
}
pub fn vault_address() -> Pubkey {
    Pubkey::find_program_address(&[b"user_vault", &hashlock()], &PROGRAM_ID).0
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
fn tx(payer: &str, name: &str, accounts: Vec<AccountMeta>, args: Vec<u8>) -> FlowTx {
    let mut data =
        solana_sha256_hasher::hash(format!("global:{name}").as_bytes()).to_bytes()[..8].to_vec();
    data.extend(args);
    FlowTx {
        payer: payer.into(),
        signers: Vec::new(),
        extra_signers: Vec::new(),
        instructions: vec![Instruction {
            program_id: PROGRAM_ID,
            accounts,
            data,
        }],
    }
}
fn delta(label: &str, delta: i128) -> ExpectedDelta {
    ExpectedDelta {
        label: label.into(),
        delta,
    }
}

fn net(world: &World, amount: u64) -> Result<u64> {
    Ok(amount
        - epoch_transfer_fee(&world.svm, &world.mint(SLOT)?.address)?
            .map(|(bps, cap)| fee_for(bps, cap, amount))
            .unwrap_or(0))
}

fn locked_amount(world: &World) -> Result<u64> {
    let Some(account) = world.svm.get_account(&lock_address()) else {
        return Ok(0);
    };
    let discriminator = solana_sha256_hasher::hash(b"account:UserLock").to_bytes();
    if account.owner != PROGRAM_ID || account.data.get(..8) != Some(&discriminator[..8]) {
        return Err(Error::Adapter(
            "unexpected Train lock account layout".into(),
        ));
    }
    let bytes: [u8; 8] = account
        .data
        .get(40..48)
        .ok_or_else(|| Error::Adapter("truncated Train lock account".into()))?
        .try_into()
        .unwrap();
    Ok(u64::from_le_bytes(bytes))
}

impl Adapter for TrainHtlcAdapter {
    fn name(&self) -> &str {
        "train-htlc"
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn mint_slots(&self) -> Vec<String> {
        vec![SLOT.into()]
    }
    fn actors(&self) -> Vec<ActorSpec> {
        [("sender", INITIAL_BALANCE), ("recipient", 0)]
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
            ("lock", FlowKind::Entry),
            ("redeem", FlowKind::Exit),
            ("refund", FlowKind::Exit),
        ]
        .into_iter()
        .map(|(name, kind)| FlowSpec {
            name: name.into(),
            kind,
            description: String::new(),
        })
        .collect()
    }
    fn scenarios(&self) -> Vec<Scenario> {
        let mut scenarios = default_scenarios(&self.flows());
        for exit in ["redeem", "refund"] {
            scenarios.push(Scenario {
                name: format!("lock_harvest_{exit}"),
                requires_extension: Some("transfer_fee".into()),
                steps: vec![
                    Step::flow("lock"),
                    Step::Issuer {
                        action: IssuerAction::HarvestWithheld { slot: SLOT.into() },
                    },
                    Step::flow(exit),
                ],
            });
        }
        for scenario in &mut scenarios {
            scenario.steps = scenario
                .steps
                .iter()
                .flat_map(|step| {
                    let mut steps = Vec::new();
                    if matches!(step, Step::Flow { name } if name == "refund") {
                        steps.push(Step::AdvanceClock { seconds: 61 });
                    }
                    steps.push(step.clone());
                    steps
                })
                .collect();
        }
        scenarios
    }
    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()> {
        if !self.so_path.is_file() {
            return Err(Error::Adapter(format!("missing Train binary {}; run scripts/fetch_targets.sh train and build chains/solana/programs/train-htlc", self.so_path.display())));
        }
        svm.add_program_from_file(PROGRAM_ID, &self.so_path)
            .map_err(|e| Error::Adapter(e.to_string()))
    }
    fn programs(&self) -> Vec<ProgramInfo> {
        vec![ProgramInfo { name: "train_htlc".into(), program_id: PROGRAM_ID,
            source: format!("{}; expected upstream {REPOSITORY}@{REVISION}:chains/solana/programs/train-htlc (MIT)", self.so_path.display()),
            sha256: std::fs::read(&self.so_path).ok().map(|b| solana_sha256_hasher::hash(&b).to_bytes().iter().map(|b| format!("{b:02x}")).collect()),
        }]
    }
    fn build_flow(&mut self, flow: &str, world: &World) -> Result<Vec<FlowTx>> {
        let sender = world.pubkey("sender")?;
        let recipient = world.pubkey("recipient")?;
        let mint = world.mint(SLOT)?;
        let program = mint.program_id();
        let ata = spl_associated_token_account_interface::program::id();
        let system = solana_system_interface::program::id();
        let lock = lock_address();
        let vault = vault_address();
        Ok(vec![match flow {
            "lock" => {
                let now = world
                    .svm
                    .get_sysvar::<solana_clock::Clock>()
                    .unix_timestamp
                    .max(0) as u64;
                let mut args = hashlock().to_vec();
                args.extend(DEPOSIT.to_le_bytes());
                args.extend(60u64.to_le_bytes());
                args.extend(now.saturating_add(3600).to_le_bytes());
                args.extend(recipient.as_ref());
                args.extend(sender.as_ref());
                args.extend(Pubkey::default().as_ref());
                args.extend(0u32.to_le_bytes()); // no payout curve data
                for _ in 0..3 {
                    args.extend(0u32.to_le_bytes());
                } // source/destination chain and address
                args.extend(0u128.to_le_bytes());
                args.extend(0u32.to_le_bytes()); // destination amount/token
                args.extend(0u128.to_le_bytes()); // destination reward amount
                args.extend(0u32.to_le_bytes());
                args.extend(0u32.to_le_bytes()); // reward token/recipient strings
                args.extend(0u64.to_le_bytes()); // destination reward timelock
                args.extend(0u32.to_le_bytes());
                args.extend(0u32.to_le_bytes()); // event-only user/solver data
                tx(
                    "sender",
                    "user_lock_token",
                    vec![
                        signer(sender),
                        ro(sender),
                        rw(lock),
                        ro(mint.address),
                        rw(world.ata("sender", SLOT)?),
                        rw(vault),
                        ro(PROGRAM_ID),
                        ro(program),
                        ro(system),
                        ro(RENT),
                    ],
                    args,
                )
            }
            "redeem" => {
                let mut args = hashlock().to_vec();
                args.extend(FIXTURE_PREIMAGE);
                tx(
                    "recipient",
                    "redeem_user_token",
                    vec![
                        signer(recipient),
                        rw(lock),
                        rw(sender),
                        ro(recipient),
                        ro(sender),
                        ro(mint.address),
                        rw(vault),
                        rw(world.ata("recipient", SLOT)?),
                        ro(PROGRAM_ID),
                        ro(PROGRAM_ID),
                        ro(program),
                        ro(ata),
                        ro(system),
                        ro(RENT),
                    ],
                    args,
                )
            }
            "refund" => tx(
                "sender",
                "refund_user_token",
                vec![
                    signer(sender),
                    rw(lock),
                    rw(sender),
                    ro(sender),
                    ro(mint.address),
                    rw(vault),
                    rw(world.ata("sender", SLOT)?),
                    ro(program),
                    ro(ata),
                    ro(system),
                    ro(RENT),
                ],
                hashlock().to_vec(),
            ),
            _ => return Err(Error::Adapter(format!("unknown flow {flow}"))),
        }])
    }
    fn tracked_accounts(&self, world: &World) -> Result<Vec<TrackedAccount>> {
        Ok([
            ("sender", world.ata("sender", SLOT)?, false),
            ("recipient", world.ata("recipient", SLOT)?, false),
            ("vault", vault_address(), true),
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
        Ok(match flow {
            "lock" => vec![
                delta("sender", -(DEPOSIT as i128)),
                delta("vault", net(world, DEPOSIT)? as i128),
            ],
            "redeem" | "refund" => {
                let amount = locked_amount(world)?;
                vec![
                    delta("vault", -(amount as i128)),
                    delta(
                        if flow == "refund" {
                            "sender"
                        } else {
                            "recipient"
                        },
                        net(world, amount)? as i128,
                    ),
                ]
            }
            _ => return Err(Error::Adapter(format!("unknown flow {flow}"))),
        })
    }
    fn credits(&self, world: &World) -> Result<Vec<Credit>> {
        Ok(vec![Credit {
            label: "user_lock.amount".into(),
            amount: locked_amount(world)?,
            backed_by: "vault".into(),
        }])
    }
    fn closable_accounts(&self, world: &World) -> Result<Vec<String>> {
        // Its intended policy permits retaining fee-bearing vaults for later harvest.
        // A failed close CPI still makes the exit fail; the golden tests check that.
        Ok(if world.mint(SLOT)?.config.transfer_fee().is_none() {
            vec!["vault".into()]
        } else {
            Vec::new()
        })
    }
    fn support_policy(&self, _slot: &str, config: &MintConfig) -> SupportPolicy {
        if config.has("permanent_delegate") || config.has("transfer_hook") {
            SupportPolicy::MustReject
        } else {
            SupportPolicy::Unspecified
        }
    }
}
