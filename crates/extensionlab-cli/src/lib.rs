//! Command-line front end for ExtensionLab.
//!
//! Adapters are Rust code, so they are compiled into the binary that runs
//! them. The `extensionlab` binary in this crate is built with an empty
//! [`Registry`]. A protocol repository that wants `extensionlab run` against
//! its own adapter adds a small binary of its own:
//!
//! ```ignore
//! // my-protocol/extensionlab/src/main.rs
//! fn main() -> std::process::ExitCode {
//!     let mut registry = extensionlab_cli::Registry::default();
//!     registry.register("escrow", "Turbin3 Q3 escrow", || Box::new(my_adapter::EscrowAdapter::new()));
//!     extensionlab_cli::main_with(registry)
//! }
//! ```
//!
//! Commands that work without an adapter: `matrix list`, `matrix show`,
//! `report render`, `regress`, `adapters`, `init`. `run` needs the adapter compiled
//! into the binary; `adapters/escrow` is an example.

use clap::{Args, Parser, Subcommand, ValueEnum};
use extensionlab_core::{
    adapter::Adapter,
    comparison,
    matrix::{self, PRESET_NAMES},
    regression,
    report::{self, FailOn, Report, EXIT_HARNESS, EXIT_OK},
    runner::Runner,
    Error,
};
use std::path::PathBuf;
use std::process::ExitCode;

mod scaffold;

/// Constructor for an adapter.
pub type AdapterFactory = fn() -> Box<dyn Adapter>;

/// One registered adapter.
pub struct RegistryEntry {
    /// Name used with `--adapter`.
    pub name: String,
    /// One-line description for `extensionlab adapters`.
    pub description: String,
    /// Constructor.
    pub make: AdapterFactory,
}

/// Adapters compiled into this binary.
#[derive(Default)]
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    /// Register an adapter under `name`.
    pub fn register(&mut self, name: &str, description: &str, make: AdapterFactory) {
        self.entries.push(RegistryEntry {
            name: name.to_string(),
            description: description.to_string(),
            make,
        });
    }

    /// Look up an adapter by name.
    pub fn get(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// All entries.
    pub fn entries(&self) -> &[RegistryEntry] {
        &self.entries
    }
}

/// Executable compatibility tests for Solana token integrations.
#[derive(Debug, Parser)]
#[command(name = "extensionlab", version, about, long_about = None)]
pub struct Cli {
    /// Command.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create an adapter crate to wire up your protocol's actual flows.
    Init(InitArgs),
    /// Run an adapter against a matrix of token configurations.
    Run(RunArgs),
    /// List or show built-in matrices.
    Matrix {
        /// Matrix subcommand.
        #[command(subcommand)]
        command: MatrixCommand,
    },
    /// Work with an existing JSON report.
    Report {
        /// Report subcommand.
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Generate a regression test stub for one run of a report.
    Regress(RegressArgs),
    /// List adapters compiled into this binary.
    Adapters,
}

/// Arguments for creating an adapter crate.
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Cargo package and adapter name.
    #[arg(long, value_parser = scaffold::package_name)]
    pub name: String,
    /// New directory for the adapter crate (must not exist).
    #[arg(long)]
    pub out: PathBuf,
    /// ExtensionLab checkout supplying the SDK crates.
    #[arg(long)]
    pub extensionlab_path: Option<PathBuf>,
}

/// Output formats for `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// `report.json` only.
    Json,
    /// `report.md` only.
    Markdown,
    /// Both files.
    Both,
}

/// What fails CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOnArg {
    /// Only runs with status `fail`.
    Fail,
    /// Runs with status `fail` or `rejected`.
    Rejected,
}

/// Arguments of `run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Adapter name (see `extensionlab adapters`).
    #[arg(long)]
    pub adapter: String,
    /// Built-in matrix name or path to a matrix JSON file.
    #[arg(long, default_value = "default")]
    pub matrix: String,
    /// Output directory for report files.
    #[arg(long, default_value = "extensionlab-report")]
    pub out: PathBuf,
    /// Which report files to write.
    #[arg(long, value_enum, default_value_t = Format::Both)]
    pub format: Format,
    /// Which run statuses fail the exit code.
    #[arg(long, value_enum, default_value_t = FailOnArg::Fail)]
    pub fail_on: FailOnArg,
    /// Seed for deterministic keypairs.
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// Local Token-2022 ELF to load instead of LiteSVM's bundled binary.
    #[arg(long)]
    pub token_2022_so: Option<PathBuf>,
    /// Expected SHA-256 of --token-2022-so (checked before any execution).
    #[arg(long, requires = "token_2022_so")]
    pub token_2022_sha256: Option<String>,
    /// Print the planned cases and exit without running them.
    #[arg(long)]
    pub plan: bool,
}

/// `matrix` subcommands.
#[derive(Debug, Subcommand)]
pub enum MatrixCommand {
    /// List built-in matrix names.
    List,
    /// Print a matrix as JSON (a built-in name or a file path).
    Show {
        /// Matrix name or path.
        name: String,
    },
}

/// `report` subcommands.
#[derive(Debug, Subcommand)]
pub enum ReportCommand {
    /// Compare two reports; fail on new failures or lost coverage.
    Compare {
        /// Report from the trusted base revision.
        before: PathBuf,
        /// Report from the proposed revision.
        after: PathBuf,
        /// Directory for comparison.json and comparison.md (also prints Markdown).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Which statuses count as a compatibility failure.
        #[arg(long, value_enum, default_value_t = FailOnArg::Fail)]
        fail_on: FailOnArg,
        /// Intentionally compare different Token/ATA runtimes.
        #[arg(long)]
        allow_runtime_change: bool,
    },
    /// Render a JSON report as Markdown on stdout.
    Render {
        /// Path to report.json.
        path: PathBuf,
    },
    /// Print the CI exit code a report maps to, and exit with it.
    ExitCode {
        /// Path to report.json.
        path: PathBuf,
        /// Which run statuses fail the exit code.
        #[arg(long, value_enum, default_value_t = FailOnArg::Fail)]
        fail_on: FailOnArg,
    },
}

/// Whether the generated test asserts `pass` or the observed status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExpectArg {
    /// Assert `pass` (red until fixed).
    Pass,
    /// Assert the observed status (pins current behavior).
    Observed,
}

/// Arguments of `regress`.
#[derive(Debug, Args)]
pub struct RegressArgs {
    /// Path to report.json.
    pub report: PathBuf,
    /// Run id to turn into a test.
    #[arg(long)]
    pub run: String,
    /// Rust expression constructing the adapter as `Box<dyn Adapter>`.
    #[arg(long)]
    pub adapter_expr: Option<String>,
    /// What the test asserts.
    #[arg(long, value_enum, default_value_t = ExpectArg::Pass)]
    pub expect: ExpectArg,
    /// Write the test to this file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

impl From<FailOnArg> for FailOn {
    fn from(v: FailOnArg) -> Self {
        match v {
            FailOnArg::Fail => FailOn::Fail,
            FailOnArg::Rejected => FailOn::Rejected,
        }
    }
}

fn harness_error(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("extensionlab: {e}");
    ExitCode::from(EXIT_HARNESS)
}

fn read_report(path: &PathBuf) -> Result<Report, Error> {
    Report::from_json(&std::fs::read_to_string(path)?)
}

/// Parse arguments and run with the given adapters. Returns the process exit
/// code (see `extensionlab_core::report::EXIT_*`). clap itself exits with 2 on
/// usage errors.
pub fn main_with(registry: Registry) -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => {
            let root = args
                .extensionlab_path
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
            match scaffold::create(&args.name, &args.out, &root) {
                Ok(()) => {
                    println!("Created adapter crate at {}. Wire the real program and flows in src/lib.rs, then run cargo run -- run --adapter {}.", args.out.display(), args.name);
                    println!("Until wired, runs return harness errors and cannot pass compatibility checks.");
                    ExitCode::from(EXIT_OK)
                }
                Err(error) => harness_error(error),
            }
        }
        Command::Adapters => {
            if registry.entries().is_empty() {
                println!(
                    "no adapters are compiled into this binary (see `extensionlab_cli::main_with`)"
                );
            }
            for e in registry.entries() {
                println!("{}\t{}", e.name, e.description);
            }
            ExitCode::from(EXIT_OK)
        }
        Command::Matrix { command } => match command {
            MatrixCommand::List => {
                for name in PRESET_NAMES {
                    let m = matrix::preset(name).expect("preset names resolve");
                    println!("{name}\t{} configs\t{}", m.configs.len(), m.description);
                }
                ExitCode::from(EXIT_OK)
            }
            MatrixCommand::Show { name } => match matrix::load(&name)
                .and_then(|m| serde_json::to_string_pretty(&m).map_err(Error::from))
            {
                Ok(json) => {
                    println!("{json}");
                    ExitCode::from(EXIT_OK)
                }
                Err(e) => harness_error(e),
            },
        },
        Command::Report { command } => match command {
            ReportCommand::Compare {
                before,
                after,
                out,
                fail_on,
                allow_runtime_change,
            } => {
                let result = (|| -> Result<_, Error> {
                    let comparison = comparison::compare(
                        &read_report(&before)?,
                        &read_report(&after)?,
                        fail_on.into(),
                        allow_runtime_change,
                    )?;
                    let markdown = comparison::render_markdown(&comparison);
                    if let Some(out) = out {
                        std::fs::create_dir_all(&out)?;
                        std::fs::write(
                            out.join("comparison.json"),
                            serde_json::to_string_pretty(&comparison)?,
                        )?;
                        std::fs::write(out.join("comparison.md"), &markdown)?;
                    }
                    print!("{markdown}");
                    Ok(comparison.exit_code)
                })();
                match result {
                    Ok(code) => ExitCode::from(code),
                    Err(e) => harness_error(e),
                }
            }
            ReportCommand::Render { path } => match read_report(&path) {
                Ok(r) => {
                    print!("{}", report::render_markdown(&r));
                    ExitCode::from(EXIT_OK)
                }
                Err(e) => harness_error(e),
            },
            ReportCommand::ExitCode { path, fail_on } => match read_report(&path) {
                Ok(r) => {
                    let code = r.exit_code(fail_on.into());
                    println!("{code}");
                    ExitCode::from(code)
                }
                Err(e) => harness_error(e),
            },
        },
        Command::Regress(args) => {
            let opts = regression::Options {
                adapter_expr: args.adapter_expr,
                expectation: match args.expect {
                    ExpectArg::Pass => regression::Expectation::Pass,
                    ExpectArg::Observed => regression::Expectation::Observed,
                },
            };
            let src = match read_report(&args.report)
                .and_then(|r| regression::render(&r, &args.run, &opts))
            {
                Ok(s) => s,
                Err(e) => return harness_error(e),
            };
            match args.out {
                Some(path) => match std::fs::write(&path, src) {
                    Ok(()) => {
                        eprintln!("wrote {}", path.display());
                        ExitCode::from(EXIT_OK)
                    }
                    Err(e) => harness_error(e),
                },
                None => {
                    print!("{src}");
                    ExitCode::from(EXIT_OK)
                }
            }
        }
        Command::Run(args) => run(&registry, args),
    }
}

fn run(registry: &Registry, args: RunArgs) -> ExitCode {
    let Some(entry) = registry.get(&args.adapter) else {
        let known: Vec<&str> = registry.entries().iter().map(|e| e.name.as_str()).collect();
        return harness_error(format!(
            "adapter `{}` is not compiled into this binary (known: {})",
            args.adapter,
            if known.is_empty() {
                "none".to_string()
            } else {
                known.join(", ")
            }
        ));
    };
    let matrix = match matrix::load(&args.matrix) {
        Ok(m) => m,
        Err(e) => return harness_error(e),
    };
    let mut adapter = (entry.make)();
    let mut runner = Runner::new(extensionlab_core::runner::RunnerOptions {
        seed: args.seed,
        ..Default::default()
    });
    if let Some(path) = &args.token_2022_so {
        runner = match runner.with_token_2022_binary(path, args.token_2022_sha256.as_deref()) {
            Ok(runner) => runner,
            Err(error) => return harness_error(error),
        };
    }
    if args.plan {
        return match runner.plan(adapter.as_ref(), &matrix) {
            Ok(cases) => {
                for c in &cases {
                    println!("{}", c.id);
                }
                eprintln!("{} cases", cases.len());
                ExitCode::from(EXIT_OK)
            }
            Err(e) => harness_error(e),
        };
    }
    let report = match runner.run(adapter.as_mut(), &matrix) {
        Ok(r) => r,
        Err(e) => return harness_error(e),
    };
    if let Err(e) = write_outputs(&report, &args.out, args.format) {
        return harness_error(e);
    }
    let s = report.summary;
    eprintln!(
        "{} runs: {} pass, {} rejected, {} fail, {} error, {} not applicable",
        s.total, s.pass, s.rejected, s.fail, s.error, s.not_applicable
    );
    let code = report.exit_code(args.fail_on.into());
    if code == report::EXIT_BASELINE {
        eprintln!("{}", report::baseline_note(&report));
    }
    eprintln!("wrote {}", args.out.display());
    ExitCode::from(code)
}

fn write_outputs(report: &Report, out: &PathBuf, format: Format) -> Result<(), Error> {
    std::fs::create_dir_all(out)?;
    if matches!(format, Format::Json | Format::Both) {
        std::fs::write(out.join("report.json"), report.to_json()?)?;
    }
    if matches!(format, Format::Markdown | Format::Both) {
        std::fs::write(out.join("report.md"), report::render_markdown(report))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn run_parses_the_documented_invocation() {
        let cli = Cli::try_parse_from([
            "extensionlab",
            "run",
            "--adapter",
            "escrow",
            "--matrix",
            "default",
        ])
        .unwrap();
        match cli.command {
            Command::Run(a) => {
                assert_eq!(a.adapter, "escrow");
                assert_eq!(a.matrix, "default");
                assert_eq!(a.format, Format::Both);
                assert_eq!(a.fail_on, FailOnArg::Fail);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
