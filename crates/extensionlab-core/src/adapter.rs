//! The adapter trait a protocol author implements.
//!
//! An adapter tells ExtensionLab four things about a protocol:
//!
//! 1. **What to load**: program binaries ([`Adapter::load_programs`]).
//! 2. **Who and what exists**: the token slots the protocol uses
//!    ([`Adapter::mint_slots`]) and the wallets that act on it
//!    ([`Adapter::actors`]).
//! 3. **What to run**: named flows such as `deposit`, `withdraw`, `refund`
//!    ([`Adapter::flows`], [`Adapter::build_flow`]). A flow returns the real
//!    instructions the protocol's client would send.
//! 4. **What should be true**: which accounts to watch, what each flow should
//!    do to their balances, what the protocol's own state says it owes, and
//!    which accounts must be closable at the end ([`Adapter::tracked_accounts`],
//!    [`Adapter::expected_deltas`], [`Adapter::credits`],
//!    [`Adapter::closable_accounts`]).
//!
//! The harness owns the LiteSVM instance, the keypairs and the mints, and sends
//! every transaction itself, so that logs, compute units and balances are
//! captured the same way for every protocol.
//!
//! The trait is object safe: the CLI keeps adapters as `Box<dyn Adapter>`.

use crate::{
    matrix::{default_scenarios, Scenario},
    mint::{MintConfig, ProvisionedMint},
    report::ProgramInfo,
    Error, Result,
};
use litesvm::LiteSVM;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::collections::BTreeMap;

/// A wallet the harness creates and funds before the first flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorSpec {
    /// Name used by flows to refer to this actor, e.g. `maker`.
    pub name: String,
    /// SOL balance in lamports.
    pub lamports: u64,
    /// Token accounts the harness creates for this actor, with the initial
    /// balance it mints into each. Slots not listed get no account; the
    /// protocol is expected to create it if it needs one.
    pub token_accounts: Vec<ActorTokenAccount>,
}

/// One actor token account (always the associated token account).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorTokenAccount {
    /// Mint slot name.
    pub slot: String,
    /// Base units minted into the account before the first flow.
    pub initial_balance: u64,
}

/// Role of a flow in the protocol's lifecycle. Scenario generation pairs
/// every entry flow with every exit flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    /// Moves user funds into protocol custody (deposit, make, open).
    Entry,
    /// Moves funds out of custody and ends the position (withdraw, refund,
    /// take, settle).
    Exit,
    /// Anything else (admin, re-price, crank).
    Other,
}

/// A named flow the adapter can build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSpec {
    /// Flow name, e.g. `refund`.
    pub name: String,
    /// Lifecycle role.
    pub kind: FlowKind,
    /// One-line description for reports.
    pub description: String,
}

/// One transaction of a flow.
///
/// Signers are named by actor; the harness holds the actor keypairs.
/// `extra_signers` carries fresh keypairs the flow itself generated (for
/// example a new account address).
#[derive(Debug)]
pub struct FlowTx {
    /// Actor that pays the transaction fee.
    pub payer: String,
    /// Other actors that must sign.
    pub signers: Vec<String>,
    /// Keypairs created by the flow that must sign.
    pub extra_signers: Vec<Keypair>,
    /// Instructions, in order.
    pub instructions: Vec<Instruction>,
}

/// An account whose balance the harness snapshots before and after every
/// step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedAccount {
    /// Label used in reports and in [`ExpectedDelta`], e.g. `vault`.
    pub label: String,
    /// Token account address.
    pub address: Pubkey,
    /// Mint slot of the account.
    pub slot: String,
    /// True when the account is protocol custody (held by a PDA). Issuer
    /// actions such as a permanent-delegate drain target custody accounts.
    pub custody: bool,
}

/// What the protocol's author expects a flow to do to one tracked account's
/// token balance, in base units. This is the nominal behavior the code was
/// written for, not what Token-2022 actually does: the difference is the
/// point of the test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedDelta {
    /// [`TrackedAccount::label`].
    pub label: String,
    /// Signed change of `amount`.
    pub delta: i128,
}

/// An amount the protocol's own state says it holds for someone, read from
/// program accounts (for example a deposit ledger or share balance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// What is credited, e.g. `deposit[maker]`.
    pub label: String,
    /// Amount in base units of the backing account's mint.
    pub amount: u64,
    /// [`TrackedAccount::label`] of the token account that should hold it.
    pub backed_by: String,
}

/// What the adapter author says the protocol should do with a token config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportPolicy {
    /// No statement. A clean rejection at the entry flow is reported as
    /// `rejected`, not as a failure.
    #[default]
    Unspecified,
    /// The protocol is meant to work with this config; a rejection is a
    /// failure.
    MustSupport,
    /// The protocol is meant to refuse this config at the entry flow;
    /// accepting it is a failure.
    MustReject,
}

/// State the harness shares with the adapter during a run.
///
/// Holds the VM, the issuer, the actors and the provisioned mints, plus
/// human-readable labels for addresses (used in reports).
pub struct World {
    /// The VM. Adapters may read accounts; they should not send transactions
    /// themselves (return [`FlowTx`] instead) so that the harness records them.
    pub svm: LiteSVM,
    /// The token issuer: mint authority, freeze authority, fee authority,
    /// permanent delegate and hook authority of every mint.
    pub issuer: Keypair,
    actors: BTreeMap<String, Keypair>,
    mints: BTreeMap<String, ProvisionedMint>,
    labels: BTreeMap<Pubkey, String>,
}

impl World {
    /// New world around an existing VM.
    pub fn new(svm: LiteSVM, issuer: Keypair) -> Self {
        Self {
            svm,
            issuer,
            actors: BTreeMap::new(),
            mints: BTreeMap::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Register an actor keypair under `name`.
    pub fn add_actor(&mut self, name: impl Into<String>, keypair: Keypair) {
        let name = name.into();
        self.labels.insert(keypair.pubkey(), name.clone());
        self.actors.insert(name, keypair);
    }

    /// Register a provisioned mint under its slot name.
    pub fn add_mint(&mut self, slot: impl Into<String>, mint: ProvisionedMint) {
        let slot = slot.into();
        self.labels.insert(mint.address, slot.clone());
        self.mints.insert(slot, mint);
    }

    /// Actor keypair by name.
    pub fn actor(&self, name: &str) -> Result<&Keypair> {
        self.actors
            .get(name)
            .ok_or_else(|| Error::Adapter(format!("unknown actor `{name}`")))
    }

    /// Actor pubkey by name.
    pub fn pubkey(&self, name: &str) -> Result<Pubkey> {
        self.actor(name).map(|k| k.pubkey())
    }

    /// Provisioned mint by slot name.
    pub fn mint(&self, slot: &str) -> Result<&ProvisionedMint> {
        self.mints
            .get(slot)
            .ok_or_else(|| Error::Adapter(format!("unknown mint slot `{slot}`")))
    }

    /// All provisioned mints, by slot name.
    pub fn mints(&self) -> impl Iterator<Item = (&String, &ProvisionedMint)> {
        self.mints.iter()
    }

    /// Associated token account of an actor for a slot.
    pub fn ata(&self, actor: &str, slot: &str) -> Result<Pubkey> {
        Ok(self.mint(slot)?.ata(&self.pubkey(actor)?))
    }

    /// Attach a report label to an address.
    pub fn label(&mut self, address: Pubkey, label: impl Into<String>) {
        self.labels.insert(address, label.into());
    }

    /// Report label of an address, if one was attached.
    pub fn label_of(&self, address: &Pubkey) -> Option<&str> {
        self.labels.get(address).map(String::as_str)
    }
}

/// A protocol under test.
///
/// Implementations live in the protocol's own repository (or in an
/// `adapters/` crate here) and are registered with the CLI; see
/// `extensionlab_cli::Registry`.
///
/// All methods that return [`Result`] should use [`Error::Adapter`] for
/// adapter-side problems (unknown flow name, account that cannot be derived).
pub trait Adapter {
    /// Short name used on the command line and in run ids, e.g. `escrow`.
    fn name(&self) -> &str;

    /// One-line description for reports.
    fn description(&self) -> &str {
        ""
    }

    /// Names of the token slots the protocol uses, e.g. `["mint_a", "mint_b"]`.
    fn mint_slots(&self) -> Vec<String>;

    /// Wallets to create and fund.
    fn actors(&self) -> Vec<ActorSpec>;

    /// Flows the adapter can build.
    fn flows(&self) -> Vec<FlowSpec>;

    /// Scenarios to run per case. The default pairs every entry flow with
    /// every exit flow and adds the issuer-action scenarios from
    /// [`default_scenarios`].
    fn scenarios(&self) -> Vec<Scenario> {
        default_scenarios(&self.flows())
    }

    /// Load the protocol's program binaries, e.g. with
    /// `svm.add_program_from_file(id, path)`. Token, Token-2022, ATA and Memo
    /// are already loaded by LiteSVM.
    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()>;

    /// The programs under test, recorded in the report's environment next to
    /// the token programs (name, id, binary path, sha256), so that reports of
    /// different builds can be told apart. The default records none.
    fn programs(&self) -> Vec<ProgramInfo> {
        Vec::new()
    }

    /// Protocol-level setup that runs after mints and actors exist and before
    /// the first scenario step (for example initializing a config account).
    /// The default does nothing.
    fn setup(&mut self, world: &World) -> Result<Vec<FlowTx>> {
        let _ = world;
        Ok(Vec::new())
    }

    /// Build the transactions for flow `flow` against the current state.
    fn build_flow(&mut self, flow: &str, world: &World) -> Result<Vec<FlowTx>>;

    /// Accounts to snapshot. Called after setup; addresses may be derived
    /// (PDAs, ATAs) even if the accounts do not exist yet.
    fn tracked_accounts(&self, world: &World) -> Result<Vec<TrackedAccount>>;

    /// Nominal balance changes of flow `flow`. Accounts not listed are not
    /// compared.
    fn expected_deltas(&self, flow: &str, world: &World) -> Result<Vec<ExpectedDelta>>;

    /// Amounts the protocol's state says it holds, read from program
    /// accounts. Used by the credited-equals-received invariant. The default
    /// exposes none, and that invariant is then reported as skipped.
    fn credits(&self, world: &World) -> Result<Vec<Credit>> {
        let _ = world;
        Ok(Vec::new())
    }

    /// Labels of tracked accounts that must no longer exist after a
    /// successful exit flow (vaults, escrow state). The default is none.
    fn closable_accounts(&self, world: &World) -> Result<Vec<String>> {
        let _ = world;
        Ok(Vec::new())
    }

    /// The author's statement about a config in a slot. The default is
    /// [`SupportPolicy::Unspecified`].
    fn support_policy(&self, slot: &str, config: &MintConfig) -> SupportPolicy {
        let _ = (slot, config);
        SupportPolicy::Unspecified
    }
}
