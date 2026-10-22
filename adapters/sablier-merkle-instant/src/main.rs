fn main() -> std::process::ExitCode {
    let mut registry = extensionlab_cli::Registry::default();
    registry.register(
        "sablier-merkle-instant",
        extensionlab_adapter_sablier_merkle_instant::DESCRIPTION,
        extensionlab_adapter_sablier_merkle_instant::boxed,
    );
    registry.register(
        "sablier-merkle-fee-aware",
        extensionlab_adapter_sablier_merkle_instant::FEE_AWARE_DESCRIPTION,
        extensionlab_adapter_sablier_merkle_instant::boxed_fee_aware,
    );
    extensionlab_cli::main_with(registry)
}
