//! Host tests for the mint factory against the token programs LiteSVM 0.10.0
//! bundles (SPL Token 3.5.0, Token-2022 10.0.0, ATA 1.1.1).
//!
//! These exercise only the harness's own setup transactions (create mint,
//! create actor account, mint, transfer between two actor accounts). They say
//! nothing about any protocol.

use extensionlab_core::{
    matrix::{preset, undeployed_hook_program_id},
    mint::{
        create_actor_token_account, create_mint, epoch_transfer_fee, mint_tokens, read_mint,
        read_token_account, send_setup, AccountExtension, MintConfig, MintExtension,
        TokenAccountStatus, TokenProgram,
    },
};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;
use spl_token_2022_interface::{extension::ExtensionType, instruction::transfer_checked};

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    issuer: Keypair,
}

fn env() -> Env {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    let issuer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    svm.airdrop(&issuer.pubkey(), 1_000_000_000).unwrap();
    Env { svm, payer, issuer }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn every_preset_config_creates_a_mint_with_exactly_its_extensions() {
    let all = preset("all").unwrap();
    for config in &all.configs {
        let mut e = env();
        let mint = Keypair::new();
        let issuer = e.issuer.pubkey();
        let m = create_mint(&mut e.svm, config, &e.payer, &mint, &issuer)
            .unwrap_or_else(|err| panic!("{}: {err}", config.label));
        let view = read_mint(&e.svm, &m.address).unwrap();

        assert_eq!(view.owner_program, config.program.id(), "{}", config.label);
        assert_eq!(view.decimals, config.decimals);
        assert_eq!(view.supply, 0);
        // TLV order is not guaranteed to match declaration order, so compare
        // as sets.
        let want: Vec<String> = config
            .mint_extension_types()
            .iter()
            .map(|t| format!("{t:?}"))
            .collect();
        assert_eq!(sorted(view.extensions), sorted(want), "{}", config.label);
        assert_eq!(
            e.svm.get_account(&m.address).unwrap().data.len(),
            config.mint_space().unwrap(),
            "{}",
            config.label
        );
    }
}

#[test]
fn spl_token_baseline_mint_is_82_bytes_and_owned_by_tokenkeg() {
    let mut e = env();
    let mint = Keypair::new();
    let issuer = e.issuer.pubkey();
    let config = MintConfig::spl_token("spl_token", 6);
    let m = create_mint(&mut e.svm, &config, &e.payer, &mint, &issuer).unwrap();
    let account = e.svm.get_account(&m.address).unwrap();
    assert_eq!(account.data.len(), 82);
    assert_eq!(account.owner, TokenProgram::SplToken.id());
}

#[test]
fn actor_accounts_are_created_funded_and_readable_for_every_default_config() {
    let default = preset("default").unwrap();
    for config in &default.configs {
        let mut e = env();
        let mint = Keypair::new();
        let owner = Keypair::new();
        let issuer_pk = e.issuer.pubkey();
        let m = create_mint(&mut e.svm, config, &e.payer, &mint, &issuer_pk).unwrap();
        let ata = create_actor_token_account(&mut e.svm, &m, &e.payer, &owner, &e.issuer)
            .unwrap_or_else(|err| panic!("{}: {err}", config.label));
        assert_eq!(ata, m.ata(&owner.pubkey()));

        mint_tokens(&mut e.svm, &m, &e.payer, &e.issuer, &ata, 1_000_000)
            .unwrap_or_else(|err| panic!("{}: {err}", config.label));

        let view = read_token_account(&e.svm, &ata).unwrap().unwrap();
        assert_eq!(view.amount, 1_000_000, "{}", config.label);
        assert_eq!(view.owner, owner.pubkey());
        assert_eq!(view.mint, m.address);
        // DefaultAccountState(Frozen) accounts are thawed by the harness.
        assert_eq!(
            view.status,
            TokenAccountStatus::Initialized,
            "{}",
            config.label
        );
        if config.has("memo_transfer_required") {
            assert!(
                view.extensions
                    .contains(&format!("{:?}", ExtensionType::MemoTransfer)),
                "{}: {:?}",
                config.label,
                view.extensions
            );
        }
        assert_eq!(read_mint(&e.svm, &m.address).unwrap().supply, 1_000_000);
    }
}

#[test]
fn a_missing_account_reads_as_none() {
    let e = env();
    assert!(read_token_account(&e.svm, &Keypair::new().pubkey())
        .unwrap()
        .is_none());
}

#[test]
fn account_readers_reject_token_layouts_owned_by_another_program() {
    for program in [TokenProgram::SplToken, TokenProgram::Token2022] {
        let mut e = env();
        let mint = Keypair::new();
        let owner = Keypair::new();
        let config = MintConfig::neutral(program, 6);
        let provisioned =
            create_mint(&mut e.svm, &config, &e.payer, &mint, &e.issuer.pubkey()).unwrap();
        let token =
            create_actor_token_account(&mut e.svm, &provisioned, &e.payer, &owner, &e.issuer)
                .unwrap();
        let mut mint_account = e.svm.get_account(&provisioned.address).unwrap();
        mint_account.owner = solana_system_interface::program::id();
        e.svm
            .set_account(provisioned.address, mint_account)
            .unwrap();
        assert!(read_mint(&e.svm, &provisioned.address).is_err());
        assert!(epoch_transfer_fee(&e.svm, &provisioned.address).is_err());

        let mut token_account = e.svm.get_account(&token).unwrap();
        token_account.owner = solana_system_interface::program::id();
        e.svm.set_account(token, token_account).unwrap();
        assert!(read_token_account(&e.svm, &token).is_err());
    }
}

fn two_funded_accounts(
    e: &mut Env,
    config: &MintConfig,
) -> (
    extensionlab_core::mint::ProvisionedMint,
    Keypair,
    solana_pubkey::Pubkey,
    solana_pubkey::Pubkey,
) {
    let mint = Keypair::new();
    let issuer_pk = e.issuer.pubkey();
    let m = create_mint(&mut e.svm, config, &e.payer, &mint, &issuer_pk).unwrap();
    let alice = Keypair::new();
    let bob = Keypair::new();
    let a = create_actor_token_account(&mut e.svm, &m, &e.payer, &alice, &e.issuer).unwrap();
    let b = create_actor_token_account(&mut e.svm, &m, &e.payer, &bob, &e.issuer).unwrap();
    mint_tokens(&mut e.svm, &m, &e.payer, &e.issuer, &a, 10_000).unwrap();
    (m, alice, a, b)
}

#[test]
fn transfer_fee_is_withheld_in_the_destination_account() {
    let mut e = env();
    let config = MintConfig::token_2022("fee", 6).with(MintExtension::TransferFee {
        basis_points: 250,
        maximum_fee: 1_000,
    });
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    let ix = transfer_checked(
        &m.program_id(),
        &a,
        &m.address,
        &b,
        &alice.pubkey(),
        &[],
        10_000,
        6,
    )
    .unwrap();
    send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice]).unwrap();

    let dest = read_token_account(&e.svm, &b).unwrap().unwrap();
    assert_eq!(config.transfer_fee_for(10_000), 250);
    assert_eq!(dest.amount, 9_750);
    assert_eq!(dest.withheld, 250);
    assert_eq!(read_token_account(&e.svm, &a).unwrap().unwrap().amount, 0);
}

#[test]
fn non_transferable_mint_rejects_transfers() {
    let mut e = env();
    let config = MintConfig::token_2022("nt", 6).with(MintExtension::NonTransferable);
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    let ix = transfer_checked(
        &m.program_id(),
        &a,
        &m.address,
        &b,
        &alice.pubkey(),
        &[],
        1,
        6,
    )
    .unwrap();
    let err = send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice]).unwrap_err();
    println!("non-transferable transfer error: {err}");
}

#[test]
fn memo_required_account_rejects_a_transfer_without_memo() {
    let mut e = env();
    let config = MintConfig::token_2022("memo", 6)
        .with_actor_account(AccountExtension::MemoTransferRequired);
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    let ix = transfer_checked(
        &m.program_id(),
        &a,
        &m.address,
        &b,
        &alice.pubkey(),
        &[],
        1,
        6,
    )
    .unwrap();
    let err = send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice]).unwrap_err();
    println!("memo-required transfer error: {err}");
}

#[test]
fn transfer_hook_mint_records_the_hook_program_id() {
    let mut e = env();
    let config = MintConfig::token_2022("hook", 6).with(MintExtension::TransferHook {
        program_id: undeployed_hook_program_id(),
    });
    let mint = Keypair::new();
    let issuer = e.issuer.pubkey();
    let m = create_mint(&mut e.svm, &config, &e.payer, &mint, &issuer).unwrap();
    let data = e.svm.get_account(&m.address).unwrap().data;
    let state = spl_token_2022_interface::extension::StateWithExtensions::<
        spl_token_2022_interface::state::Mint,
    >::unpack(&data)
    .unwrap();
    assert_eq!(
        spl_token_2022_interface::extension::transfer_hook::get_program_id(&state),
        Some(undeployed_hook_program_id())
    );
}

#[test]
fn invalid_configs_are_rejected_before_any_transaction() {
    let spl_with_ext = MintConfig::spl_token("bad", 6).with(MintExtension::NonTransferable);
    assert!(spl_with_ext.validate().is_err());
    let dup = MintConfig::token_2022("dup", 6)
        .with(MintExtension::PermanentDelegate)
        .with(MintExtension::PermanentDelegate);
    assert!(dup.validate().is_err());
    let fee = MintConfig::token_2022("fee", 6).with(MintExtension::TransferFee {
        basis_points: 10_001,
        maximum_fee: 0,
    });
    assert!(fee.validate().is_err());
    assert!(MintConfig::token_2022("a/b", 6).validate().is_err());
}

#[test]
fn an_account_with_withheld_fees_cannot_be_closed() {
    let mut e = env();
    let config = MintConfig::token_2022("fee", 6).with(MintExtension::TransferFee {
        basis_points: 250,
        maximum_fee: 1_000,
    });
    let mint = Keypair::new();
    let issuer_pk = e.issuer.pubkey();
    let m = create_mint(&mut e.svm, &config, &e.payer, &mint, &issuer_pk).unwrap();
    let alice = Keypair::new();
    let bob = Keypair::new();
    let a = create_actor_token_account(&mut e.svm, &m, &e.payer, &alice, &e.issuer).unwrap();
    let b = create_actor_token_account(&mut e.svm, &m, &e.payer, &bob, &e.issuer).unwrap();
    mint_tokens(&mut e.svm, &m, &e.payer, &e.issuer, &a, 10_000).unwrap();

    let p = m.program_id();
    let to_bob = transfer_checked(&p, &a, &m.address, &b, &alice.pubkey(), &[], 10_000, 6).unwrap();
    send_setup(&mut e.svm, "alice to bob", &e.payer, &[to_bob], &[&alice]).unwrap();
    // Bob moves out everything he can spend; 250 stays withheld in his account.
    let back = transfer_checked(&p, &b, &m.address, &a, &bob.pubkey(), &[], 9_750, 6).unwrap();
    send_setup(&mut e.svm, "bob to alice", &e.payer, &[back], &[&bob]).unwrap();
    let bob_view = read_token_account(&e.svm, &b).unwrap().unwrap();
    assert_eq!((bob_view.amount, bob_view.withheld), (0, 250));

    let close = spl_token_2022_interface::instruction::close_account(
        &p,
        &b,
        &bob.pubkey(),
        &bob.pubkey(),
        &[],
    )
    .unwrap();
    let err = send_setup(&mut e.svm, "close bob", &e.payer, &[close], &[&bob]).unwrap_err();
    let text = err.to_string();
    println!("close-with-withheld error: {text}");
    assert!(text.contains("withheld fee balance is zero"), "{text}");
    assert!(read_token_account(&e.svm, &b).unwrap().is_some());
}

#[test]
fn a_frozen_by_default_account_rejects_incoming_tokens_until_thawed() {
    let mut e = env();
    let config = MintConfig::token_2022("frozen", 6).with(MintExtension::DefaultAccountStateFrozen);
    let mint = Keypair::new();
    let issuer_pk = e.issuer.pubkey();
    let m = create_mint(&mut e.svm, &config, &e.payer, &mint, &issuer_pk).unwrap();
    // Create the account the way a protocol would: plain ATA creation, no thaw.
    let owner = Keypair::new();
    let ix = spl_associated_token_account_interface::instruction::create_associated_token_account(
        &e.payer.pubkey(),
        &owner.pubkey(),
        &m.address,
        &m.program_id(),
    );
    send_setup(&mut e.svm, "create ata", &e.payer, &[ix], &[]).unwrap();
    let ata = m.ata(&owner.pubkey());
    assert_eq!(
        read_token_account(&e.svm, &ata).unwrap().unwrap().status,
        TokenAccountStatus::Frozen
    );
    let err = mint_tokens(&mut e.svm, &m, &e.payer, &e.issuer, &ata, 1).unwrap_err();
    let text = err.to_string();
    println!("frozen-account error: {text}");
    assert!(text.contains("Account is frozen"), "{text}");
}

#[test]
fn a_paused_mint_rejects_transfers_until_resumed() {
    let mut e = env();
    let config = MintConfig::token_2022("pausable", 6).with(MintExtension::Pausable);
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    assert!(!read_mint(&e.svm, &m.address).unwrap().paused);
    // Token-2022 adds the PausableAccount extension to every account of the mint.
    assert!(read_token_account(&e.svm, &a)
        .unwrap()
        .unwrap()
        .extensions
        .contains(&format!("{:?}", ExtensionType::PausableAccount)));

    let p = m.program_id();
    // Amounts differ so that no two transactions are identical.
    let send = |e: &mut Env, amount: u64| {
        let ix = transfer_checked(&p, &a, &m.address, &b, &alice.pubkey(), &[], amount, 6).unwrap();
        send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice])
    };
    let issuer_pk = e.issuer.pubkey();
    let pause = spl_token_2022_interface::extension::pausable::instruction::pause(
        &p,
        &m.address,
        &issuer_pk,
        &[],
    )
    .unwrap();
    let resume = spl_token_2022_interface::extension::pausable::instruction::resume(
        &p,
        &m.address,
        &issuer_pk,
        &[],
    )
    .unwrap();

    send(&mut e, 1).unwrap();
    send_setup(&mut e.svm, "pause", &e.payer, &[pause], &[&e.issuer]).unwrap();
    assert!(read_mint(&e.svm, &m.address).unwrap().paused);
    let text = send(&mut e, 2).unwrap_err().to_string();
    println!("paused transfer error: {text}");
    assert!(text.contains("paused on this mint"), "{text}");
    send_setup(&mut e.svm, "resume", &e.payer, &[resume], &[&e.issuer]).unwrap();
    assert!(!read_mint(&e.svm, &m.address).unwrap().paused);
    send(&mut e, 3).unwrap();
    assert_eq!(read_token_account(&e.svm, &b).unwrap().unwrap().amount, 4);
}

#[test]
fn a_cpi_guard_actor_account_is_locked_but_direct_transfers_work() {
    let mut e = env();
    let config =
        MintConfig::token_2022("cpi_guard", 6).with_actor_account(AccountExtension::CpiGuard);
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    let data = e.svm.get_account(&a).unwrap().data;
    let state = spl_token_2022_interface::extension::StateWithExtensions::<
        spl_token_2022_interface::state::Account,
    >::unpack(&data)
    .unwrap();
    use spl_token_2022_interface::extension::BaseStateWithExtensions;
    let guard = state
        .get_extension::<spl_token_2022_interface::extension::cpi_guard::CpiGuard>()
        .unwrap();
    assert!(bool::from(guard.lock_cpi));

    // The guard only applies inside a CPI; the owner can still sign a
    // top-level transfer. (The escrow golden test covers the CPI case.)
    let ix = transfer_checked(
        &m.program_id(),
        &a,
        &m.address,
        &b,
        &alice.pubkey(),
        &[],
        10,
        6,
    )
    .unwrap();
    send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice]).unwrap();
    assert_eq!(read_token_account(&e.svm, &b).unwrap().unwrap().amount, 10);
}

#[test]
fn a_new_transfer_fee_applies_two_epochs_after_it_is_set() {
    let mut e = env();
    let config = MintConfig::token_2022("fee", 6).with(MintExtension::TransferFee {
        basis_points: 200,
        maximum_fee: u64::MAX,
    });
    let (m, alice, a, b) = two_funded_accounts(&mut e, &config);
    assert_eq!(
        epoch_transfer_fee(&e.svm, &m.address).unwrap(),
        Some((200, u64::MAX))
    );
    let set = spl_token_2022_interface::extension::transfer_fee::instruction::set_transfer_fee(
        &m.program_id(),
        &m.address,
        &e.issuer.pubkey(),
        &[],
        500,
        u64::MAX,
    )
    .unwrap();
    send_setup(&mut e.svm, "set fee", &e.payer, &[set], &[&e.issuer]).unwrap();

    let mut clock: solana_clock::Clock = e.svm.get_sysvar();
    let start = clock.epoch;
    for (epoch, want) in [(start, 200), (start + 1, 200), (start + 2, 500)] {
        clock.epoch = epoch;
        e.svm.set_sysvar(&clock);
        assert_eq!(
            epoch_transfer_fee(&e.svm, &m.address).unwrap(),
            Some((want, u64::MAX)),
            "epoch {epoch}"
        );
    }
    // Token-2022 agrees: a transfer now withholds 5%.
    let ix = transfer_checked(
        &m.program_id(),
        &a,
        &m.address,
        &b,
        &alice.pubkey(),
        &[],
        10_000,
        6,
    )
    .unwrap();
    send_setup(&mut e.svm, "transfer", &e.payer, &[ix], &[&alice]).unwrap();
    assert_eq!(
        read_token_account(&e.svm, &b).unwrap().unwrap().withheld,
        500
    );
    assert!(epoch_transfer_fee(&e.svm, &solana_pubkey::Pubkey::new_unique()).is_err());
}
