//! Error type shared by the core crate.

/// Errors returned by ExtensionLab.
///
/// These are harness errors: the tool could not do what was asked. A protocol
/// transaction failing during a run is not an `Error`; it is recorded in the
/// report as a step outcome.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested functionality is part of the design but has not been
    /// written yet. The string names the missing piece.
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),

    /// A mint configuration is internally inconsistent (for example an SPL
    /// Token mint with Token-2022 extensions).
    #[error("invalid mint config `{label}`: {reason}")]
    InvalidMintConfig {
        /// Label of the offending config.
        label: String,
        /// Why it is invalid.
        reason: String,
    },

    /// A matrix name was neither a built-in preset nor a readable file.
    #[error("unknown matrix `{0}`")]
    UnknownMatrix(String),

    /// A run id was not found in a report.
    #[error("run `{0}` not found in report")]
    UnknownRun(String),

    /// An adapter reported a problem (bad flow name, missing account, ...).
    #[error("adapter error: {0}")]
    Adapter(String),

    /// Building a token program instruction failed.
    #[error("could not build instruction: {0}")]
    Instruction(String),

    /// A transaction the harness itself sends (mint creation, funding, ...)
    /// failed. Contains the transaction error and the program logs.
    #[error("harness setup transaction `{what}` failed: {err}\n{logs}")]
    SetupTransaction {
        /// What the harness was doing.
        what: String,
        /// Rendered transaction error.
        err: String,
        /// Program logs, newline separated.
        logs: String,
    },

    /// An account the harness expected to read was missing or not decodable.
    #[error("account {address} could not be read: {reason}")]
    AccountRead {
        /// Base58 address.
        address: String,
        /// Why reading failed.
        reason: String,
    },

    /// JSON (de)serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// File I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
