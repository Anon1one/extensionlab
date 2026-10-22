fn main() -> std::process::ExitCode {
    let mut registry = extensionlab_cli::Registry::default();
    registry.register(
        "train-htlc",
        extensionlab_adapter_train_htlc::DESCRIPTION,
        extensionlab_adapter_train_htlc::boxed,
    );
    extensionlab_cli::main_with(registry)
}
