//! `extensionlab` binary with no adapters compiled in.
//!
//! See `extensionlab_cli::main_with` for building a binary that includes a
//! protocol's adapter.

fn main() -> std::process::ExitCode {
    extensionlab_cli::main_with(extensionlab_cli::Registry::default())
}
