//! Mint factory: the token configurations a scenario runs against.
//!
//! A [`MintConfig`] describes one token: which program owns it (classic SPL
//! Token or Token-2022), its decimals, which mint extensions it carries, and
//! which account extensions the harness enables on the token accounts it
//! creates for actors (wallet keypairs).
//!
//! The harness plays the token issuer. One issuer key is the mint authority,
//! freeze authority, transfer-fee authorities, permanent delegate,
//! transfer-hook authority and pause authority. Real tokens can split these roles; a single issuer
//! is enough to reproduce what each extension does to a protocol.
//!
//! What is implemented here:
//! - config types with validation ([`MintConfig::validate`]);
//! - instruction builders for mint creation ([`create_mint_instructions`]);
//! - LiteSVM helpers that create mints, actor token accounts, mint tokens and
//!   read balances back ([`create_mint`], [`create_actor_token_account`],
//!   [`mint_tokens`], [`read_token_account`], [`read_mint`]);
//! - the transfer fee in effect at the current epoch, which differs from the
//!   configured one after the issuer schedules a new fee
//!   ([`epoch_transfer_fee`]).
//!
//! What is not implemented: deploying a transfer-hook program. A
//! [`MintExtension::TransferHook`] mint only records the hook program id.

use crate::{Error, Result};
use litesvm::{types::TransactionMetadata, LiteSVM};
use serde::{Deserialize, Serialize};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_associated_token_account_interface::{
    address::get_associated_token_address_with_program_id,
    instruction::create_associated_token_account,
};
use spl_token_2022_interface::{
    extension::{
        cpi_guard::instruction::enable_cpi_guard,
        default_account_state::instruction::initialize_default_account_state,
        memo_transfer::instruction::enable_required_transfer_memos,
        pausable::{self, PausableConfig},
        transfer_fee::{
            instruction::initialize_transfer_fee_config, TransferFee, TransferFeeAmount,
            TransferFeeConfig, MAX_FEE_BASIS_POINTS,
        },
        transfer_hook, BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction::{
        initialize_mint2, initialize_non_transferable_mint, initialize_permanent_delegate, mint_to,
        reallocate, thaw_account,
    },
    state::{Account as TokenAccountState, AccountState, Mint as MintState},
};

/// Which token program owns a mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenProgram {
    /// Classic SPL Token (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`).
    SplToken,
    /// Token-2022 / Token Extensions (`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`).
    #[serde(rename = "token_2022")]
    Token2022,
}

impl TokenProgram {
    /// Program id.
    pub fn id(self) -> Pubkey {
        match self {
            TokenProgram::SplToken => spl_token_2022_interface::inline_spl_token::id(),
            TokenProgram::Token2022 => spl_token_2022_interface::id(),
        }
    }

    /// Short name used in labels and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenProgram::SplToken => "spl_token",
            TokenProgram::Token2022 => "token_2022",
        }
    }
}

/// A Token-2022 mint extension, with the parameters the harness sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MintExtension {
    /// `TransferFeeConfig`. Every transfer withholds a fee in the destination
    /// account: `ceil(amount * basis_points / 10_000)`, capped at
    /// `maximum_fee`.
    TransferFee {
        /// Fee rate in basis points (0..=10_000).
        basis_points: u16,
        /// Fee cap in base units.
        maximum_fee: u64,
    },
    /// `DefaultAccountState` set to `Frozen`: every new token account starts
    /// frozen until the freeze authority thaws it. The harness thaws the
    /// accounts it creates for actors (it is the issuer approving its own
    /// users) but never thaws accounts the protocol creates.
    DefaultAccountStateFrozen,
    /// `PermanentDelegate` set to the issuer: the issuer can transfer or burn
    /// from any token account of this mint without the owner's signature.
    PermanentDelegate,
    /// `TransferHook` pointing at `program_id`. Every transfer must CPI into
    /// that program. The factory only records the id on the mint; it does not
    /// deploy a hook program.
    TransferHook {
        /// Hook program id stored on the mint.
        #[serde(with = "crate::b58")]
        program_id: Pubkey,
    },
    /// `NonTransferable`: transfers fail; owners can still burn and close.
    NonTransferable,
    /// `Pausable` with the issuer as pause authority. While the mint is
    /// paused, transfers, mints and burns of it fail, including the ones a
    /// protocol makes by CPI. Created unpaused.
    Pausable,
}

impl MintExtension {
    /// The Token-2022 extension type this config initializes.
    pub fn extension_type(&self) -> ExtensionType {
        match self {
            MintExtension::TransferFee { .. } => ExtensionType::TransferFeeConfig,
            MintExtension::DefaultAccountStateFrozen => ExtensionType::DefaultAccountState,
            MintExtension::PermanentDelegate => ExtensionType::PermanentDelegate,
            MintExtension::TransferHook { .. } => ExtensionType::TransferHook,
            MintExtension::NonTransferable => ExtensionType::NonTransferable,
            MintExtension::Pausable => ExtensionType::Pausable,
        }
    }

    /// Stable snake_case name, identical to the serde `kind` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            MintExtension::TransferFee { .. } => "transfer_fee",
            MintExtension::DefaultAccountStateFrozen => "default_account_state_frozen",
            MintExtension::PermanentDelegate => "permanent_delegate",
            MintExtension::TransferHook { .. } => "transfer_hook",
            MintExtension::NonTransferable => "non_transferable",
            MintExtension::Pausable => "pausable",
        }
    }
}

/// A Token-2022 account extension the harness enables on actor token
/// accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountExtension {
    /// `MemoTransfer` with required memos enabled: incoming transfers must be
    /// preceded by a memo instruction (or, inside a program, a memo CPI)
    /// immediately before the transfer.
    MemoTransferRequired,
    /// `CpiGuard` enabled: inside a CPI, the owner's signature can no longer
    /// transfer from, approve, close or change the authority of the account.
    /// A protocol that pulls tokens by CPI with the user as signer fails; it
    /// has to use a delegate instead.
    CpiGuard,
}

impl AccountExtension {
    /// The Token-2022 extension type this config enables.
    pub fn extension_type(&self) -> ExtensionType {
        match self {
            AccountExtension::MemoTransferRequired => ExtensionType::MemoTransfer,
            AccountExtension::CpiGuard => ExtensionType::CpiGuard,
        }
    }

    /// Stable snake_case name, identical to the serde `kind` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            AccountExtension::MemoTransferRequired => "memo_transfer_required",
            AccountExtension::CpiGuard => "cpi_guard",
        }
    }
}

/// One token configuration in a matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintConfig {
    /// Short identifier used in run ids, e.g. `transfer_fee_200bps`. Must be
    /// non-empty and must not contain `/` or `@`.
    pub label: String,
    /// Owning token program.
    pub program: TokenProgram,
    /// Mint decimals.
    pub decimals: u8,
    /// Mint extensions (Token-2022 only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<MintExtension>,
    /// Account extensions the harness enables on token accounts it creates
    /// for actors (Token-2022 only). Never applied to accounts the protocol
    /// creates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actor_account_extensions: Vec<AccountExtension>,
}

impl MintConfig {
    /// Classic SPL Token mint with no extensions.
    pub fn spl_token(label: impl Into<String>, decimals: u8) -> Self {
        Self {
            label: label.into(),
            program: TokenProgram::SplToken,
            decimals,
            extensions: Vec::new(),
            actor_account_extensions: Vec::new(),
        }
    }

    /// Token-2022 mint with no extensions.
    pub fn token_2022(label: impl Into<String>, decimals: u8) -> Self {
        Self {
            label: label.into(),
            program: TokenProgram::Token2022,
            decimals,
            extensions: Vec::new(),
            actor_account_extensions: Vec::new(),
        }
    }

    /// The extension-free config for `program`, used to fill mint slots that
    /// a case is not varying. Labels are `spl_token` and `token_2022`.
    pub fn neutral(program: TokenProgram, decimals: u8) -> Self {
        match program {
            TokenProgram::SplToken => Self::spl_token("spl_token", decimals),
            TokenProgram::Token2022 => Self::token_2022("token_2022", decimals),
        }
    }

    /// Builder: add a mint extension.
    pub fn with(mut self, ext: MintExtension) -> Self {
        self.extensions.push(ext);
        self
    }

    /// Builder: add an actor account extension.
    pub fn with_actor_account(mut self, ext: AccountExtension) -> Self {
        self.actor_account_extensions.push(ext);
        self
    }

    /// True when the config has no mint or account extensions.
    pub fn is_neutral(&self) -> bool {
        self.extensions.is_empty() && self.actor_account_extensions.is_empty()
    }

    /// True when a mint extension of the given kind is present.
    pub fn has(&self, kind: &str) -> bool {
        self.extensions.iter().any(|e| e.kind() == kind)
            || self
                .actor_account_extensions
                .iter()
                .any(|e| e.kind() == kind)
    }

    /// The transfer fee parameters, when the config has one.
    pub fn transfer_fee(&self) -> Option<(u16, u64)> {
        self.extensions.iter().find_map(|e| match e {
            MintExtension::TransferFee {
                basis_points,
                maximum_fee,
            } => Some((*basis_points, *maximum_fee)),
            _ => None,
        })
    }

    /// Mint extension types, in the order they are declared.
    pub fn mint_extension_types(&self) -> Vec<ExtensionType> {
        self.extensions.iter().map(|e| e.extension_type()).collect()
    }

    /// Check that the config can be created.
    ///
    /// Rejects: empty labels or labels containing `/` or `@`; SPL Token
    /// configs with any extension; duplicate extensions; transfer fees above
    /// 10_000 basis points; and the combinations Token-2022's own
    /// `ExtensionType::check_for_invalid_mint_extension_combinations` rejects.
    pub fn validate(&self) -> Result<()> {
        let invalid = |reason: &str| Error::InvalidMintConfig {
            label: self.label.clone(),
            reason: reason.to_string(),
        };
        if self.label.is_empty() || self.label.contains('/') || self.label.contains('@') {
            return Err(invalid(
                "label must be non-empty and must not contain '/' or '@'",
            ));
        }
        if self.program == TokenProgram::SplToken && !self.is_neutral() {
            return Err(invalid(
                "SPL Token mints cannot carry Token-2022 extensions",
            ));
        }
        let mut seen = Vec::new();
        for ty in self.mint_extension_types().into_iter().chain(
            self.actor_account_extensions
                .iter()
                .map(|e| e.extension_type()),
        ) {
            if seen.contains(&ty) {
                return Err(invalid(&format!("extension {ty:?} listed twice")));
            }
            seen.push(ty);
        }
        if let Some((bps, _)) = self.transfer_fee() {
            if bps > MAX_FEE_BASIS_POINTS {
                return Err(invalid("transfer fee basis points above 10_000"));
            }
        }
        ExtensionType::check_for_invalid_mint_extension_combinations(&self.mint_extension_types())
            .map_err(|e| invalid(&format!("Token-2022 rejects this combination: {e}")))?;
        Ok(())
    }

    /// Size in bytes of the mint account for this config.
    pub fn mint_space(&self) -> Result<usize> {
        ExtensionType::try_calculate_account_len::<MintState>(&self.mint_extension_types())
            .map_err(|e| Error::Instruction(format!("mint size for `{}`: {e}", self.label)))
    }

    /// Fee Token-2022 would withhold on a transfer of `amount` with the fee
    /// the mint was created with. Returns 0 when the config has no transfer
    /// fee. After an `issuer:set_transfer_fee` step the fee in effect can
    /// differ; read it with [`epoch_transfer_fee`].
    pub fn transfer_fee_for(&self, amount: u64) -> u64 {
        match self.transfer_fee() {
            None => 0,
            Some((bps, max)) => fee_for(bps, max, amount),
        }
    }
}

/// Fee Token-2022 withholds on a transfer of `amount` at `basis_points`,
/// capped at `maximum_fee`: `ceil(amount * basis_points / 10_000)`.
pub fn fee_for(basis_points: u16, maximum_fee: u64, amount: u64) -> u64 {
    TransferFee {
        epoch: 0u64.into(),
        maximum_fee: maximum_fee.into(),
        transfer_fee_basis_points: basis_points.into(),
    }
    .calculate_fee(amount)
    .unwrap_or(0)
}

/// A mint that exists in a LiteSVM instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionedMint {
    /// Mint address.
    #[serde(with = "crate::b58")]
    pub address: Pubkey,
    /// The config it was created from.
    pub config: MintConfig,
}

impl ProvisionedMint {
    /// Owning token program id.
    pub fn program_id(&self) -> Pubkey {
        self.config.program.id()
    }

    /// Associated token account address of `owner` for this mint.
    pub fn ata(&self, owner: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, &self.address, &self.program_id())
    }
}

/// Instructions that create and initialize a mint for `config`.
///
/// Order: `create_account` with the full extension size, one initializer per
/// mint extension, then `InitializeMint2`. Token-2022 requires mint
/// extensions to be initialized before the mint itself.
///
/// The mint authority and freeze authority are both `issuer`.
pub fn create_mint_instructions(
    config: &MintConfig,
    payer: &Pubkey,
    mint: &Pubkey,
    issuer: &Pubkey,
    rent_lamports: u64,
) -> Result<Vec<Instruction>> {
    config.validate()?;
    let program = config.program.id();
    let space = config.mint_space()?;
    let fmt = |what: &str, e: &dyn std::fmt::Display| Error::Instruction(format!("{what}: {e}"));

    let mut ixs = vec![solana_system_interface::instruction::create_account(
        payer,
        mint,
        rent_lamports,
        space as u64,
        &program,
    )];
    for ext in &config.extensions {
        let ix = match ext {
            MintExtension::TransferFee {
                basis_points,
                maximum_fee,
            } => initialize_transfer_fee_config(
                &program,
                mint,
                Some(issuer),
                Some(issuer),
                *basis_points,
                *maximum_fee,
            )
            .map_err(|e| fmt("initialize_transfer_fee_config", &e))?,
            MintExtension::DefaultAccountStateFrozen => {
                initialize_default_account_state(&program, mint, &AccountState::Frozen)
                    .map_err(|e| fmt("initialize_default_account_state", &e))?
            }
            MintExtension::PermanentDelegate => {
                initialize_permanent_delegate(&program, mint, issuer)
                    .map_err(|e| fmt("initialize_permanent_delegate", &e))?
            }
            MintExtension::TransferHook { program_id } => transfer_hook::instruction::initialize(
                &program,
                mint,
                Some(*issuer),
                Some(*program_id),
            )
            .map_err(|e| fmt("transfer_hook::initialize", &e))?,
            MintExtension::NonTransferable => initialize_non_transferable_mint(&program, mint)
                .map_err(|e| fmt("initialize_non_transferable_mint", &e))?,
            MintExtension::Pausable => pausable::instruction::initialize(&program, mint, issuer)
                .map_err(|e| fmt("pausable::initialize", &e))?,
        };
        ixs.push(ix);
    }
    ixs.push(
        initialize_mint2(&program, mint, issuer, Some(issuer), config.decimals)
            .map_err(|e| fmt("initialize_mint2", &e))?,
    );
    Ok(ixs)
}

/// Sign and send `ixs` with `payer` as fee payer.
///
/// `candidates` are keypairs that may be needed as signers; only those whose
/// pubkey the message actually requires are used, each once. Returns the
/// LiteSVM metadata on success, or [`Error::SetupTransaction`] with the logs.
pub fn send_setup(
    svm: &mut LiteSVM,
    what: &str,
    payer: &Keypair,
    ixs: &[Instruction],
    candidates: &[&Keypair],
) -> Result<TransactionMetadata> {
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
        .map_err(|e| Error::SetupTransaction {
            what: what.to_string(),
            err: format!("signing: {e}"),
            logs: String::new(),
        })?;
    svm.send_transaction(tx)
        .map_err(|failed| Error::SetupTransaction {
            what: what.to_string(),
            err: format!("{:?}", failed.err),
            logs: failed.meta.logs.join("\n"),
        })
}

/// Create a mint for `config` in `svm`. `mint` is the new mint keypair;
/// `issuer` becomes mint authority, freeze authority and every extension
/// authority.
pub fn create_mint(
    svm: &mut LiteSVM,
    config: &MintConfig,
    payer: &Keypair,
    mint: &Keypair,
    issuer: &Pubkey,
) -> Result<ProvisionedMint> {
    let rent = svm.minimum_balance_for_rent_exemption(config.mint_space()?);
    let ixs = create_mint_instructions(config, &payer.pubkey(), &mint.pubkey(), issuer, rent)?;
    send_setup(
        svm,
        &format!("create mint `{}`", config.label),
        payer,
        &ixs,
        &[mint],
    )?;
    Ok(ProvisionedMint {
        address: mint.pubkey(),
        config: config.clone(),
    })
}

/// Create the associated token account of `owner` for `mint`, as an actor
/// account.
///
/// - For `DefaultAccountStateFrozen` mints the issuer thaws the new account.
/// - For each [`AccountExtension`] in the config, the account is reallocated
///   for that extension and the extension is enabled (signed by `owner`).
///
/// Returns the account address.
pub fn create_actor_token_account(
    svm: &mut LiteSVM,
    mint: &ProvisionedMint,
    payer: &Keypair,
    owner: &Keypair,
    issuer: &Keypair,
) -> Result<Pubkey> {
    let program = mint.program_id();
    let owner_pk = owner.pubkey();
    let ata = mint.ata(&owner_pk);
    let fmt = |what: &str, e: &dyn std::fmt::Display| Error::Instruction(format!("{what}: {e}"));

    let mut ixs = vec![create_associated_token_account(
        &payer.pubkey(),
        &owner_pk,
        &mint.address,
        &program,
    )];
    if mint.config.has("default_account_state_frozen") {
        ixs.push(
            thaw_account(&program, &ata, &mint.address, &issuer.pubkey(), &[])
                .map_err(|e| fmt("thaw_account", &e))?,
        );
    }
    if !mint.config.actor_account_extensions.is_empty() {
        let types: Vec<ExtensionType> = mint
            .config
            .actor_account_extensions
            .iter()
            .map(|e| e.extension_type())
            .collect();
        ixs.push(
            reallocate(&program, &ata, &payer.pubkey(), &owner_pk, &[], &types)
                .map_err(|e| fmt("reallocate", &e))?,
        );
        for ext in &mint.config.actor_account_extensions {
            match ext {
                AccountExtension::MemoTransferRequired => ixs.push(
                    enable_required_transfer_memos(&program, &ata, &owner_pk, &[])
                        .map_err(|e| fmt("enable_required_transfer_memos", &e))?,
                ),
                AccountExtension::CpiGuard => ixs.push(
                    enable_cpi_guard(&program, &ata, &owner_pk, &[])
                        .map_err(|e| fmt("enable_cpi_guard", &e))?,
                ),
            }
        }
    }
    send_setup(
        svm,
        &format!("create actor account for `{}`", mint.config.label),
        payer,
        &ixs,
        &[owner, issuer],
    )?;
    Ok(ata)
}

/// Mint `amount` base units of `mint` into `account`, signed by the issuer.
pub fn mint_tokens(
    svm: &mut LiteSVM,
    mint: &ProvisionedMint,
    payer: &Keypair,
    issuer: &Keypair,
    account: &Pubkey,
    amount: u64,
) -> Result<()> {
    let ix = mint_to(
        &mint.program_id(),
        &mint.address,
        account,
        &issuer.pubkey(),
        &[],
        amount,
    )
    .map_err(|e| Error::Instruction(format!("mint_to: {e}")))?;
    send_setup(
        svm,
        &format!("mint {amount} of `{}`", mint.config.label),
        payer,
        &[ix],
        &[issuer],
    )?;
    Ok(())
}

/// Whether a token account is usable or frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenAccountStatus {
    /// Initialized and not frozen.
    Initialized,
    /// Frozen by the freeze authority.
    Frozen,
}

/// Decoded view of a token account (SPL Token or Token-2022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenAccountView {
    /// Mint.
    #[serde(with = "crate::b58")]
    pub mint: Pubkey,
    /// Owner.
    #[serde(with = "crate::b58")]
    pub owner: Pubkey,
    /// Spendable balance in base units.
    pub amount: u64,
    /// Transfer fees withheld in this account (Token-2022 TransferFeeAmount;
    /// 0 when absent).
    pub withheld: u64,
    /// Account state.
    pub status: TokenAccountStatus,
    /// Lamports held by the account.
    pub lamports: u64,
    /// Account extension types present in the TLV data, as Debug names.
    pub extensions: Vec<String>,
}

/// Decoded view of a mint (SPL Token or Token-2022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintView {
    /// Owning program.
    #[serde(with = "crate::b58")]
    pub owner_program: Pubkey,
    /// Total supply in base units.
    pub supply: u64,
    /// Decimals.
    pub decimals: u8,
    /// Fees harvested to the mint and not yet withdrawn (TransferFeeConfig;
    /// 0 when absent).
    pub withheld: u64,
    /// True when the mint has `Pausable` and is paused.
    pub paused: bool,
    /// Mint extension types present in the TLV data, as Debug names.
    pub extensions: Vec<String>,
}

/// Read a token account. Returns `Ok(None)` when no account exists at
/// `address` (for example after it was closed).
///
/// Uses Token-2022's `StateWithExtensions`, which also decodes plain 165-byte
/// SPL Token accounts because the base layout is shared.
pub fn read_token_account(svm: &LiteSVM, address: &Pubkey) -> Result<Option<TokenAccountView>> {
    let Some(account) = svm.get_account(address) else {
        return Ok(None);
    };
    let read_err = |reason: String| Error::AccountRead {
        address: address.to_string(),
        reason,
    };
    let state = StateWithExtensions::<TokenAccountState>::unpack(&account.data)
        .map_err(|e| read_err(format!("not a token account: {e}")))?;
    let withheld = state
        .get_extension::<TransferFeeAmount>()
        .map(|f| u64::from(f.withheld_amount))
        .unwrap_or(0);
    let extensions = state
        .get_extension_types()
        .map_err(|e| read_err(format!("bad TLV data: {e}")))?
        .into_iter()
        .map(|t| format!("{t:?}"))
        .collect();
    Ok(Some(TokenAccountView {
        mint: state.base.mint,
        owner: state.base.owner,
        amount: state.base.amount,
        withheld,
        status: if state.base.is_frozen() {
            TokenAccountStatus::Frozen
        } else {
            TokenAccountStatus::Initialized
        },
        lamports: account.lamports,
        extensions,
    }))
}

/// Read a mint.
pub fn read_mint(svm: &LiteSVM, address: &Pubkey) -> Result<MintView> {
    let read_err = |reason: String| Error::AccountRead {
        address: address.to_string(),
        reason,
    };
    let account = svm
        .get_account(address)
        .ok_or_else(|| read_err("account does not exist".into()))?;
    let state = StateWithExtensions::<MintState>::unpack(&account.data)
        .map_err(|e| read_err(format!("not a mint: {e}")))?;
    let withheld = state
        .get_extension::<TransferFeeConfig>()
        .map(|c| u64::from(c.withheld_amount))
        .unwrap_or(0);
    let paused = state
        .get_extension::<PausableConfig>()
        .map(|p| bool::from(p.paused))
        .unwrap_or(false);
    let extensions = state
        .get_extension_types()
        .map_err(|e| read_err(format!("bad TLV data: {e}")))?
        .into_iter()
        .map(|t| format!("{t:?}"))
        .collect();
    Ok(MintView {
        owner_program: account.owner,
        supply: state.base.supply,
        decimals: state.base.decimals,
        withheld,
        paused,
        extensions,
    })
}

/// The transfer fee in effect for `address` at the VM's current epoch, as
/// `(basis_points, maximum_fee)`, or `None` when the mint has no transfer
/// fee. Token-2022 applies a fee set with `SetTransferFee` two epochs after
/// the epoch it was set in; until then the older fee applies.
pub fn epoch_transfer_fee(svm: &LiteSVM, address: &Pubkey) -> Result<Option<(u16, u64)>> {
    let read_err = |reason: String| Error::AccountRead {
        address: address.to_string(),
        reason,
    };
    let account = svm
        .get_account(address)
        .ok_or_else(|| read_err("account does not exist".into()))?;
    let state = StateWithExtensions::<MintState>::unpack(&account.data)
        .map_err(|e| read_err(format!("not a mint: {e}")))?;
    let Ok(config) = state.get_extension::<TransferFeeConfig>() else {
        return Ok(None);
    };
    let epoch = svm.get_sysvar::<solana_clock::Clock>().epoch;
    let fee = config.get_epoch_fee(epoch);
    Ok(Some((
        u16::from(fee.transfer_fee_basis_points),
        u64::from(fee.maximum_fee),
    )))
}
