fn main() -> std::process::ExitCode {
    let mut registry = extensionlab_cli::Registry::default();
    registry.register(
        "program-examples-escrow",
        extensionlab_adapter_program_examples_escrow::DESCRIPTION,
        extensionlab_adapter_program_examples_escrow::boxed,
    );
    extensionlab_cli::main_with(registry)
}
