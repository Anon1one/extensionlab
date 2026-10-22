//! Generate an isolated adapter crate that refuses to report compatibility
//! until the author wires the real program, transactions and expectations.

use extensionlab_core::{Error, Result};
use std::path::Path;

pub fn package_name(name: &str) -> std::result::Result<String, String> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err("use a lowercase package name starting with a letter, with at most 64 letters, digits, hyphens or underscores".into());
    }
    Ok(name.into())
}

pub fn create(name: &str, out: &Path, extensionlab: &Path) -> Result<()> {
    package_name(name).map_err(Error::Adapter)?;
    let root = extensionlab.canonicalize()?;
    let core = root.join("crates/extensionlab-core");
    let cli = root.join("crates/extensionlab-cli");
    if !core.join("Cargo.toml").is_file() || !cli.join("Cargo.toml").is_file() {
        return Err(Error::Adapter(
            "--extensionlab-path must point to an ExtensionLab checkout".into(),
        ));
    }
    if out.exists() {
        return Err(Error::Adapter(format!(
            "refusing to overwrite {}",
            out.display()
        )));
    }
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(out)?;
    std::fs::create_dir(out.join("src"))?;
    let manifest = format!(
        r#"[package]
name = {name:?}
version = "0.1.0"
edition = "2021"
publish = false

[workspace]

[lib]
name = "protocol_adapter"

[dependencies]
extensionlab-core = {{ path = {core} }}
extensionlab-cli = {{ path = {cli} }}
litesvm = "=0.10.0"
"#,
        core = serde_json::to_string(&core.display().to_string())?,
        cli = serde_json::to_string(&cli.display().to_string())?
    );
    std::fs::write(out.join("Cargo.toml"), manifest)?;
    std::fs::write(out.join(".gitignore"), "/target/\n/extensionlab-report/\n")?;
    std::fs::write(
        out.join("src/main.rs"),
        format!(
            r#"fn main() -> std::process::ExitCode {{
    let mut registry = extensionlab_cli::Registry::default();
    registry.register({name:?}, "Protocol compatibility adapter", || {{
        Box::new(protocol_adapter::ProtocolAdapter)
    }});
    extensionlab_cli::main_with(registry)
}}
"#
        ),
    )?;
    std::fs::write(
        out.join("src/lib.rs"),
        TEMPLATE.replace("__ADAPTER_NAME__", name),
    )?;
    Ok(())
}

const TEMPLATE: &str = r#"use extensionlab_core::{
    adapter::{
        ActorSpec, Adapter, ExpectedDelta, FlowKind, FlowSpec, FlowTx, TrackedAccount, World,
    },
    Error, Result,
};
use litesvm::LiteSVM;

pub struct ProtocolAdapter;

impl Adapter for ProtocolAdapter {
    fn name(&self) -> &str {
        "__ADAPTER_NAME__"
    }

    fn mint_slots(&self) -> Vec<String> {
        vec!["asset".into()]
    }

    fn actors(&self) -> Vec<ActorSpec> {
        // Declare named wallets and their initial token balances here.
        Vec::new()
    }

    fn flows(&self) -> Vec<FlowSpec> {
        vec![
            FlowSpec {
                name: "deposit".into(),
                kind: FlowKind::Entry,
                description: String::new(),
            },
            FlowSpec {
                name: "withdraw".into(),
                kind: FlowKind::Exit,
                description: String::new(),
            },
        ]
    }

    fn load_programs(&self, _svm: &mut LiteSVM) -> Result<()> {
        // Load the compiled protocol ELF under its declared program id.
        Err(Error::NotImplemented(
            "load the protocol's real program binary",
        ))
    }

    fn build_flow(&mut self, _flow: &str, _world: &World) -> Result<Vec<FlowTx>> {
        // Return the instructions the protocol's real client would send.
        Err(Error::NotImplemented("build protocol transactions"))
    }

    fn tracked_accounts(&self, _world: &World) -> Result<Vec<TrackedAccount>> {
        // Include every token account that moves value; mark custody accounts.
        Err(Error::NotImplemented("declare tracked token accounts"))
    }

    fn expected_deltas(&self, _flow: &str, _world: &World) -> Result<Vec<ExpectedDelta>> {
        // This is called before each flow; state the intended balance changes.
        Err(Error::NotImplemented("state protocol balance expectations"))
    }
}
"#;
