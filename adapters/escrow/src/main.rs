//! `extensionlab-escrow`: the ExtensionLab CLI with the escrow adapter
//! compiled in.
//!
//! ```text
//! cargo run -p extensionlab-adapter-escrow -- run --adapter escrow --matrix default
//! ```

fn main() -> std::process::ExitCode {
    let mut registry = extensionlab_cli::Registry::default();
    registry.register(
        "escrow",
        extensionlab_adapter_escrow::DESCRIPTION,
        extensionlab_adapter_escrow::boxed,
    );
    registry.register(
        "escrow-existing-atas",
        extensionlab_adapter_escrow::DESCRIPTION_EXISTING_ATAS,
        extensionlab_adapter_escrow::boxed_existing_atas,
    );
    extensionlab_cli::main_with(registry)
}
