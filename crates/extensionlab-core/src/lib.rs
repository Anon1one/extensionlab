//! # extensionlab-core
//!
//! Executable compatibility tests for Solana token integrations.
//!
//! A protocol author implements [`adapter::Adapter`] for their program (escrow,
//! payments, vault, ...). ExtensionLab then runs the protocol's real flows
//! (deposit, withdraw, refund, ...) on [LiteSVM](https://github.com/LiteSVM/litesvm)
//! against a matrix of controlled token configurations (classic SPL Token and
//! Token-2022 with selected extensions) and reports, per run, the failing
//! transaction and logs, expected versus actual balances, which invariant broke,
//! and a regression test stub that can run in CI.
//!
//! ## Status of this crate
//!
//! | Module | State |
//! |---|---|
//! | [`adapter`] | Trait and data types defined. No adapters ship in this crate. |
//! | [`mint`] | Config types implemented. Mint and token-account creation implemented for the SPL Token baseline and for Token-2022 with TransferFee, DefaultAccountState(Frozen), PermanentDelegate, TransferHook (program id only, no hook program), NonTransferable and Pausable, with MemoTransfer and CpiGuard on actor accounts. Exercised by the host tests in `tests/mint_factory.rs`. |
//! | [`matrix`] | Presets and case expansion implemented and unit-tested; Pausable and CpiGuard are in the `all` preset only. Issuer steps (drain, freeze, harvest, set transfer fee, pause, resume, pause all) and clock and epoch steps; `tests/runner_steps.rs` runs them with a program-free adapter. |
//! | [`invariants`] | Pure checks over balance snapshots implemented and unit-tested. The runner captures the snapshots. |
//! | [`runner`] | Implemented. [`runner::Runner::run_case`] executes one case in a fresh LiteSVM; harness problems become `error` runs. |
//! | [`report`] | JSON types, summary, CI exit codes and Markdown rendering implemented. |
//! | [`diagnosis`] | Broken-assumption rule catalog implemented (heuristic log and invariant matching). |
//! | [`regression`] | Regression test stub generation implemented (string template). |
//!
//! Nothing in this crate claims that a given protocol is or is not compatible
//! with a given extension. Only a run of the runner with that protocol's adapter
//! produces such results.

pub mod adapter;
pub mod diagnosis;
pub mod error;
pub mod invariants;
pub mod matrix;
pub mod mint;
pub mod regression;
pub mod report;
pub mod runner;

pub use error::{Error, Result};

/// Version of this crate, recorded in every report.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of LiteSVM this build is pinned to.
///
/// LiteSVM 0.10.0 loads these program binaries by default
/// (`litesvm-0.10.0/src/programs/mod.rs`): SPL Token 3.5.0, Token-2022 10.0.0,
/// Associated Token Account 1.1.1, and Memo 1.0.0 and 3.0.0. Those are the
/// token program versions every run executes against unless an adapter
/// replaces them.
pub const LITESVM_VERSION: &str = "0.10.0";

/// Serde helpers that write a `Pubkey` as a base58 string in reports and
/// config files, instead of the 32-element byte array `solana-pubkey` emits.
pub mod b58 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};
    use solana_pubkey::Pubkey;
    use std::str::FromStr;

    /// Serialize a `Pubkey` as a base58 string.
    pub fn serialize<S: Serializer>(key: &Pubkey, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&key.to_string())
    }

    /// Deserialize a `Pubkey` from a base58 string.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Pubkey, D::Error> {
        let s = String::deserialize(d)?;
        Pubkey::from_str(&s).map_err(D::Error::custom)
    }
}
