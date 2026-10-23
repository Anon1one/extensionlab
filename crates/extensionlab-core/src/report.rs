//! Report types (JSON, schema version [`SCHEMA_VERSION`]), run
//! classification, CI exit codes and Markdown rendering.
//!
//! The JSON written by `extensionlab run` is exactly `serde_json` of
//! [`Report`]. `research/04_architecture.md` documents the schema with an
//! example, and `tests/fixtures/example_report.json` is a copy of that
//! example that the tests deserialize, so the document and the types cannot
//! drift apart silently.

use crate::{
    adapter::SupportPolicy,
    invariants::{CheckStatus, InvariantCheck},
    matrix::{Scenario, SlotStrategy, Step},
    mint::{MintConfig, TokenProgram},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Version of the JSON report schema. Bump the minor version for additive
/// changes and the major version for breaking ones.
pub const SCHEMA_VERSION: &str = "0.3.0";

/// A full report: one adapter, one matrix, every run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// [`SCHEMA_VERSION`] of the writer.
    pub schema_version: String,
    /// Tool that wrote the report.
    pub tool: ToolInfo,
    /// Execution environment.
    pub environment: Environment,
    /// Adapter under test.
    pub adapter: AdapterInfo,
    /// Matrix that was run.
    pub matrix: MatrixInfo,
    /// Counts per status.
    pub summary: Summary,
    /// Every run, in execution order.
    pub runs: Vec<RunResult>,
}

/// Tool name and version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Always `extensionlab`.
    pub name: String,
    /// Crate version.
    pub version: String,
}

/// Execution environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// LiteSVM version.
    pub litesvm: String,
    /// Keypair seed used for this report (older reports used seed zero).
    #[serde(default)]
    pub seed: u64,
    /// Every program loaded into the VM that matters for the result: token
    /// programs and the programs under test.
    pub programs: Vec<ProgramInfo>,
}

/// A program loaded into the VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramInfo {
    /// Human name, e.g. `spl_token_2022`.
    pub name: String,
    /// Program id.
    #[serde(with = "crate::b58")]
    pub program_id: Pubkey,
    /// Where the binary came from, e.g. `bundled with litesvm 0.10.0` or a
    /// file path.
    pub source: String,
    /// SHA-256 of the binary, hex, when the harness computed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Adapter name and description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterInfo {
    /// `Adapter::name`.
    pub name: String,
    /// `Adapter::description`.
    #[serde(default)]
    pub description: String,
}

/// Matrix name, strategy and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixInfo {
    /// Matrix name.
    pub name: String,
    /// Slot strategy.
    pub strategy: SlotStrategy,
    /// Number of cases (equals `runs.len()` for a complete report).
    pub cases: usize,
}

/// Counts per status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Summary {
    /// All runs.
    pub total: usize,
    /// [`RunStatus::Pass`].
    pub pass: usize,
    /// [`RunStatus::Rejected`].
    pub rejected: usize,
    /// [`RunStatus::Fail`].
    pub fail: usize,
    /// [`RunStatus::Error`].
    pub error: usize,
    /// Cases whose issuer action had no eligible accounts or tokens.
    #[serde(default)]
    pub not_applicable: usize,
}

impl Summary {
    /// Count statuses.
    pub fn of(runs: &[RunResult]) -> Self {
        let mut s = Summary {
            total: runs.len(),
            ..Default::default()
        };
        for r in runs {
            match r.status {
                RunStatus::Pass => s.pass += 1,
                RunStatus::Rejected => s.rejected += 1,
                RunStatus::Fail => s.fail += 1,
                RunStatus::Error => s.error += 1,
                RunStatus::NotApplicable => s.not_applicable += 1,
            }
        }
        s
    }
}

/// Verdict of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Every step succeeded and no invariant was violated.
    Pass,
    /// The first step failed atomically and nothing moved: the protocol
    /// refused this config. Compatible by refusal, unless the adapter said it
    /// must support the config.
    Rejected,
    /// An invariant was violated, a later step failed after funds entered
    /// custody, or the outcome contradicts the adapter's support policy.
    Fail,
    /// The harness failed; the run says nothing about the protocol.
    Error,
    /// No failure was observed, but an issuer action had nothing to exercise.
    /// This is not evidence that the program resisted that action.
    NotApplicable,
}

/// Result of one case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResult {
    /// Case id, e.g. `transfer_fee_200bps@mint_a/make_take`.
    pub id: String,
    /// Config of every slot.
    pub slots: BTreeMap<String, MintConfig>,
    /// The varied slot, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub varied_slot: Option<String>,
    /// True when every slot is extension-free.
    pub baseline: bool,
    /// Scenario that ran.
    pub scenario: Scenario,
    /// Verdict.
    pub status: RunStatus,
    /// One record per scenario step.
    pub steps: Vec<StepRecord>,
    /// Expected versus actual balances after the last executed step,
    /// relative to the state before the first step.
    pub balances: Vec<BalanceRow>,
    /// Every invariant check.
    pub invariants: Vec<InvariantCheck>,
    /// Best-effort explanation of which integration assumption broke.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<Diagnosis>,
    /// Harness error text when `status` is `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_error: Option<String>,
}

impl RunResult {
    /// The first transaction that failed, with its step.
    pub fn first_failed_tx(&self) -> Option<(&StepRecord, &TxRecord)> {
        self.steps.iter().find_map(|s| {
            s.transactions
                .iter()
                .find(|t| t.error.is_some())
                .map(|t| (s, t))
        })
    }

    /// Invariant checks that were violated.
    pub fn violations(&self) -> impl Iterator<Item = &InvariantCheck> {
        self.invariants.iter().filter(|c| c.is_violated())
    }
}

/// What happened in one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    /// Every transaction succeeded.
    Succeeded,
    /// A transaction failed (and was rolled back by the runtime).
    Failed,
    /// Not run because an earlier step failed.
    Skipped,
    /// An issuer action had no eligible accounts or tokens; no transaction ran.
    NotApplicable,
}

/// Record of one scenario step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    /// Zero-based index in the scenario.
    pub index: usize,
    /// The step.
    pub step: Step,
    /// Outcome.
    pub outcome: StepOutcome,
    /// Transactions sent for this step.
    pub transactions: Vec<TxRecord>,
}

/// One transaction as LiteSVM executed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxRecord {
    /// First signature of the transaction, base58. It identifies the
    /// transaction inside this LiteSVM run only; it was never sent to a
    /// cluster and cannot be looked up on an explorer.
    pub signature: String,
    /// Compute units consumed.
    pub compute_units: u64,
    /// Fee charged, in lamports.
    pub fee_lamports: u64,
    /// Rendered `TransactionError`, when the transaction failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Program logs.
    pub logs: Vec<String>,
}

/// Expected versus actual balance of one tracked account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceRow {
    /// Tracked account label.
    pub label: String,
    /// Address.
    #[serde(with = "crate::b58")]
    pub address: Pubkey,
    /// Mint slot.
    pub slot: String,
    /// `amount` before the first step (0 if the account did not exist).
    pub before: u64,
    /// `amount` the adapter expected; absent when the adapter made no claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_after: Option<u64>,
    /// `amount` observed.
    pub actual_after: u64,
    /// Transfer fees withheld in the account, observed.
    pub withheld_after: u64,
    /// Whether the account exists at the end.
    pub exists_after: bool,
}

/// Which assumption broke, from the rule catalog in `research/04_architecture.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnosis {
    /// Rule id, e.g. `transfer_fee.close_with_withheld`.
    pub rule: String,
    /// What the rule is about: an extension kind (e.g. `transfer_fee`) or a
    /// token program (`token_2022`) for rules that apply without extensions.
    pub scope: String,
    /// The assumption in plain words.
    pub assumption: String,
    /// Log line or invariant detail that matched.
    pub evidence: String,
}

/// Classify a run from its steps, its invariant checks and the adapter's
/// support policy for the varied config.
///
/// Rules, in order:
/// 1. any violated invariant: `fail`;
/// 2. a flow step failed and no transaction of the run was committed (steps
///    without transactions, such as a clock step, do not count): `rejected`,
///    or `fail` under [`SupportPolicy::MustSupport`];
/// 3. any other failed step: `fail` (something failed after state changed,
///    or an issuer action failed);
/// 4. every step succeeded: `pass`, or `fail` under
///    [`SupportPolicy::MustReject`].
pub fn classify(
    steps: &[StepRecord],
    invariants: &[InvariantCheck],
    policy: SupportPolicy,
) -> RunStatus {
    if invariants.iter().any(|c| c.status == CheckStatus::Violated) {
        return RunStatus::Fail;
    }
    let committed = steps
        .iter()
        .flat_map(|s| s.transactions.iter())
        .any(|t| t.error.is_none());
    let failed_flow = steps
        .iter()
        .any(|s| s.outcome == StepOutcome::Failed && matches!(s.step, Step::Flow { .. }));
    let any_failed = steps.iter().any(|s| s.outcome == StepOutcome::Failed);
    if failed_flow && !committed {
        return match policy {
            SupportPolicy::MustSupport => RunStatus::Fail,
            _ => RunStatus::Rejected,
        };
    }
    if any_failed {
        return RunStatus::Fail;
    }
    match policy {
        SupportPolicy::MustReject => RunStatus::Fail,
        _ if steps
            .iter()
            .any(|s| s.outcome == StepOutcome::NotApplicable) =>
        {
            RunStatus::NotApplicable
        }
        _ => RunStatus::Pass,
    }
}

/// Exit code: everything passed (or was rejected, when allowed).
pub const EXIT_OK: u8 = 0;
/// Exit code: at least one compatibility failure.
pub const EXIT_FAILURES: u8 = 1;
/// Exit code: command-line usage error. clap uses 2 for usage errors, so the
/// report never produces it.
pub const EXIT_USAGE: u8 = 2;
/// Exit code: harness error (setup failed, program binary missing, or a run
/// ended in `error`).
pub const EXIT_HARNESS: u8 = 3;
/// Exit code: a baseline run (every slot an extension-free SPL Token mint)
/// failed, so the adapter or the protocol is broken before Token-2022 or any
/// extension is involved and the other results are not meaningful.
pub const EXIT_BASELINE: u8 = 4;

/// What counts as a failure for the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailOn {
    /// Only `fail` runs fail CI. Clean rejections are accepted.
    #[default]
    Fail,
    /// `fail` and `rejected` runs fail CI: the protocol must support every
    /// config in the matrix.
    Rejected,
}

impl Report {
    /// Validate the report envelope before using it as CI evidence.
    ///
    /// Read the published 0.1, 0.2 and 0.3 schemas. A newer minor schema must be
    /// explicitly supported: an older reader cannot know whether newly added
    /// evidence changes the meaning of a passing result.
    pub fn validate(&self) -> Result<()> {
        let version: Vec<_> = self.schema_version.split('.').collect();
        let numeric = |part: &str| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        };
        if version.len() != 3
            || !version.iter().all(|part| numeric(part))
            || version[0] != "0"
            || !matches!(version[1], "1" | "2" | "3")
        {
            return Err(Error::Adapter(format!(
                "report schema {} is not supported by {}",
                self.schema_version, SCHEMA_VERSION
            )));
        }
        if self.matrix.cases != self.runs.len() || self.summary != Summary::of(&self.runs) {
            return Err(Error::Adapter("report counts do not match its runs".into()));
        }
        let mut ids = BTreeSet::new();
        for run in &self.runs {
            if !ids.insert(run.id.as_str()) {
                return Err(Error::Adapter(format!("duplicate run id `{}`", run.id)));
            }
            let baseline = !run.slots.is_empty()
                && run
                    .slots
                    .values()
                    .all(|config| config.program == TokenProgram::SplToken && config.is_neutral());
            if version[1] != "1" && run.baseline != baseline {
                return Err(Error::Adapter(format!(
                    "run `{}` has a baseline flag inconsistent with its mint slots",
                    run.id
                )));
            }
            if run.steps.len() != run.scenario.steps.len()
                || run.steps.iter().zip(&run.scenario.steps).enumerate().any(
                    |(index, (record, expected))| record.index != index || &record.step != expected,
                )
            {
                return Err(Error::Adapter(format!(
                    "run `{}` has missing or inconsistent scenario step records",
                    run.id
                )));
            }
            if run.invariants.iter().any(|check| {
                check
                    .step_index
                    .is_some_and(|index| index >= run.steps.len())
            }) {
                return Err(Error::Adapter(format!(
                    "run `{}` has an invariant outside its scenario steps",
                    run.id
                )));
            }
            if matches!(
                run.status,
                RunStatus::Pass | RunStatus::Rejected | RunStatus::NotApplicable
            ) && (run.harness_error.is_some() || run.violations().next().is_some())
            {
                return Err(Error::Adapter(format!(
                    "run `{}` has a non-failing status but explicit failure evidence",
                    run.id
                )));
            }
            if matches!(
                run.status,
                RunStatus::Pass | RunStatus::Rejected | RunStatus::NotApplicable
            ) && run
                .balances
                .iter()
                .chain(run.invariants.iter().flat_map(|check| &check.balance_rows))
                .any(|row| {
                    row.expected_after
                        .is_some_and(|expected| expected != row.actual_after)
                })
            {
                return Err(Error::Adapter(format!(
                    "run `{}` has a non-failing status but mismatched balance evidence",
                    run.id
                )));
            }
            if run.status != RunStatus::Error
                && run.steps.iter().any(|step| match step.outcome {
                    StepOutcome::Succeeded => {
                        step.transactions.iter().any(|tx| tx.error.is_some())
                            || (matches!(step.step, Step::Flow { .. })
                                && step.transactions.is_empty())
                    }
                    StepOutcome::Failed => !step.transactions.iter().any(|tx| tx.error.is_some()),
                    StepOutcome::Skipped | StepOutcome::NotApplicable => {
                        !step.transactions.is_empty()
                    }
                })
            {
                return Err(Error::Adapter(format!(
                    "run `{}` has step outcomes inconsistent with its transactions",
                    run.id
                )));
            }
            if matches!(run.status, RunStatus::Rejected | RunStatus::NotApplicable)
                && classify(&run.steps, &run.invariants, SupportPolicy::Unspecified) != run.status
            {
                return Err(Error::Adapter(format!(
                    "run `{}` has a rejection or applicability status inconsistent with its evidence",
                    run.id
                )));
            }
            if run.status == RunStatus::Pass
                && (!run
                    .steps
                    .iter()
                    .any(|step| matches!(step.step, Step::Flow { .. }))
                    || run.steps.iter().any(|step| {
                        step.outcome != StepOutcome::Succeeded
                            || step.transactions.iter().any(|tx| tx.error.is_some())
                            || (matches!(step.step, Step::Flow { .. })
                                && step.transactions.is_empty())
                    }))
            {
                return Err(Error::Adapter(format!(
                    "passing run `{}` contains unsuccessful or unexecuted steps",
                    run.id
                )));
            }
        }
        Ok(())
    }

    /// Assemble a report and compute its summary.
    pub fn new(
        environment: Environment,
        adapter: AdapterInfo,
        matrix_name: &str,
        strategy: SlotStrategy,
        runs: Vec<RunResult>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            tool: ToolInfo {
                name: "extensionlab".into(),
                version: crate::TOOL_VERSION.into(),
            },
            environment,
            adapter,
            matrix: MatrixInfo {
                name: matrix_name.to_string(),
                strategy,
                cases: runs.len(),
            },
            summary: Summary::of(&runs),
            runs,
        }
    }

    /// Exit code for CI, in priority order: [`EXIT_BASELINE`],
    /// [`EXIT_HARNESS`], [`EXIT_FAILURES`], [`EXIT_OK`].
    pub fn exit_code(&self, fail_on: FailOn) -> u8 {
        if self.validate().is_err() {
            return EXIT_HARNESS;
        }
        if self
            .runs
            .iter()
            .any(|r| r.baseline && matches!(r.status, RunStatus::Fail | RunStatus::Rejected))
        {
            return EXIT_BASELINE;
        }
        if self.runs.iter().any(|r| r.status == RunStatus::Error) {
            return EXIT_HARNESS;
        }
        if self.runs.is_empty()
            || self
                .runs
                .iter()
                .all(|r| r.status == RunStatus::NotApplicable)
        {
            return EXIT_HARNESS;
        }
        let failing = |r: &&RunResult| match fail_on {
            FailOn::Fail => r.status == RunStatus::Fail,
            FailOn::Rejected => matches!(r.status, RunStatus::Fail | RunStatus::Rejected),
        };
        if self.runs.iter().any(|r| failing(&r)) {
            EXIT_FAILURES
        } else {
            EXIT_OK
        }
    }

    /// Run by id.
    pub fn run(&self, id: &str) -> Result<&RunResult> {
        self.runs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| Error::UnknownRun(id.to_string()))
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse JSON and reject unsupported schemas or inconsistent evidence counts.
    pub fn from_json(text: &str) -> Result<Self> {
        let report: Report = serde_json::from_str(text)?;
        report.validate()?;
        Ok(report)
    }
}

fn status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Pass => "pass",
        RunStatus::Rejected => "rejected",
        RunStatus::Fail => "FAIL",
        RunStatus::Error => "error",
        RunStatus::NotApplicable => "not applicable",
    }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn describe_config(c: &MintConfig) -> String {
    let mut parts: Vec<String> = c
        .extensions
        .iter()
        .map(|e| match e {
            crate::mint::MintExtension::TransferFee {
                basis_points,
                maximum_fee,
            } if *maximum_fee == u64::MAX => format!("transfer_fee {basis_points} bps, uncapped"),
            crate::mint::MintExtension::TransferFee {
                basis_points,
                maximum_fee,
            } => format!("transfer_fee {basis_points} bps, max {maximum_fee}"),
            other => other.kind().to_string(),
        })
        .collect();
    parts.extend(
        c.actor_account_extensions
            .iter()
            .map(|e| e.kind().to_string()),
    );
    if parts.is_empty() {
        format!("{} ({})", c.label, c.program.as_str())
    } else {
        format!("{} ({}: {})", c.label, c.program.as_str(), parts.join(", "))
    }
}

/// Render a report as Markdown (suitable for `$GITHUB_STEP_SUMMARY`).
///
/// Layout: header, summary table, one row per run, then a details section
/// for every run that is not `pass` or `rejected`, with steps, the logs of
/// the first failing transaction, balances and invariants.
/// One paragraph on the baseline runs (every slot an extension-free SPL Token
/// mint): how many
/// there are and which failed. A failed baseline makes [`Report::exit_code`]
/// return [`EXIT_BASELINE`].
pub fn baseline_note(r: &Report) -> String {
    let baseline: Vec<&RunResult> = r.runs.iter().filter(|x| x.baseline).collect();
    let failed: Vec<String> = baseline
        .iter()
        .filter(|x| matches!(x.status, RunStatus::Fail | RunStatus::Rejected))
        .map(|x| format!("`{}`", x.id))
        .collect();
    if failed.is_empty() {
        format!(
            "Baseline: {} run(s) with every slot on SPL Token without extensions; none failed.",
            baseline.len()
        )
    } else {
        format!(
            "Baseline: {} run(s) with every slot on SPL Token without extensions; {} failed: {}. A failed baseline sets exit code {EXIT_BASELINE}: the protocol (or its adapter) fails before Token-2022 or any extension is involved, so read the other results with that in mind.",
            baseline.len(),
            failed.len(),
            failed.join(", ")
        )
    }
}

pub fn render_markdown(r: &Report) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "# ExtensionLab report: `{}` x `{}`\n",
        r.adapter.name, r.matrix.name
    );
    let _ = writeln!(
        o,
        "extensionlab {} · LiteSVM {} · report schema {}\n",
        r.tool.version, r.environment.litesvm, r.schema_version
    );
    let s = &r.summary;
    let _ = writeln!(
        o,
        "| Total | Pass | Rejected | Fail | Error | Not applicable |\n|---|---|---|---|---|---|"
    );
    let _ = writeln!(
        o,
        "| {} | {} | {} | {} | {} | {} |\n",
        s.total, s.pass, s.rejected, s.fail, s.error, s.not_applicable
    );
    let _ = writeln!(o, "{}\n", baseline_note(r));

    let _ = writeln!(
        o,
        "## Runs\n\n| Run | Status | Broken invariants | Broken assumption |\n|---|---|---|---|"
    );
    for run in &r.runs {
        let broken: Vec<String> = run
            .violations()
            .map(|c| {
                let name = serde_json::to_value(c.invariant)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                match &c.slot {
                    Some(slot) => format!("{name} ({slot})"),
                    None => name,
                }
            })
            .collect();
        let assumption = run
            .diagnosis
            .as_ref()
            .map(|d| d.assumption.clone())
            .unwrap_or_else(|| {
                run.steps
                    .iter()
                    .filter(|s| s.outcome == StepOutcome::NotApplicable)
                    .map(|s| format!("{}: no eligible accounts or tokens", s.step.describe()))
                    .collect::<Vec<_>>()
                    .join("; ")
            });
        let _ = writeln!(
            o,
            "| `{}` | {} | {} | {} |",
            cell(&run.id),
            status_str(run.status),
            cell(&broken.join(", ")),
            cell(&assumption)
        );
    }

    let details: Vec<&RunResult> = r
        .runs
        .iter()
        .filter(|x| matches!(x.status, RunStatus::Fail | RunStatus::Error))
        .collect();
    if !details.is_empty() {
        let _ = writeln!(o, "\n## Failures");
    }
    for run in details {
        let _ = writeln!(o, "\n### `{}`\n", run.id);
        let _ = writeln!(o, "- **Status:** {}", status_str(run.status));
        let mints: Vec<String> = run
            .slots
            .iter()
            .map(|(slot, c)| format!("{slot} = {}", describe_config(c)))
            .collect();
        let _ = writeln!(o, "- **Mints:** {}", mints.join("; "));
        if let Some(d) = &run.diagnosis {
            let _ = writeln!(
                o,
                "- **Broken assumption:** {} (rule `{}`, heuristic)\n- **Evidence:** `{}`",
                d.assumption,
                d.rule,
                cell(&d.evidence)
            );
        }
        if let Some(e) = &run.harness_error {
            let _ = writeln!(o, "- **Harness error:** {}", cell(e));
        }
        let _ = writeln!(o, "\n**Steps**\n");
        for st in &run.steps {
            let outcome = match st.outcome {
                StepOutcome::Succeeded => "succeeded",
                StepOutcome::Failed => "failed",
                StepOutcome::Skipped => "skipped",
                StepOutcome::NotApplicable => "not applicable (no eligible accounts or tokens)",
            };
            let tx = st
                .transactions
                .iter()
                .map(|t| match &t.error {
                    Some(err) => format!("`{}`: {}", short_sig(&t.signature), cell(err)),
                    None => format!("`{}`, {} CU", short_sig(&t.signature), t.compute_units),
                })
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(
                o,
                "{}. `{}`: {} {}",
                st.index + 1,
                st.step.describe(),
                outcome,
                tx
            );
        }
        if let Some((st, tx)) = run.first_failed_tx() {
            let _ = writeln!(
                o,
                "\n<details><summary>Logs of the failing transaction (step {})</summary>\n\n```text",
                st.index + 1
            );
            for l in &tx.logs {
                let _ = writeln!(o, "{l}");
            }
            let _ = writeln!(o, "```\n\n</details>");
        }
        if !run.balances.is_empty() {
            let _ = writeln!(
                o,
                "\n**Balances**\n\n| Account | Slot | Before | Expected after | Actual after | Withheld after | Exists |\n|---|---|---|---|---|---|---|"
            );
            for b in &run.balances {
                let _ = writeln!(
                    o,
                    "| {} | {} | {} | {} | {} | {} | {} |",
                    cell(&b.label),
                    cell(&b.slot),
                    b.before,
                    b.expected_after
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                    b.actual_after,
                    b.withheld_after,
                    if b.exists_after { "yes" } else { "no" }
                );
            }
        }
        if !run.invariants.is_empty() {
            let _ = writeln!(
                o,
                "\n**Invariants**\n\n| Invariant | Slot | Step | Status | Detail |\n|---|---|---|---|---|"
            );
            for c in &run.invariants {
                let name = serde_json::to_value(c.invariant)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let status = match c.status {
                    CheckStatus::Held => "held",
                    CheckStatus::Violated => "VIOLATED",
                    CheckStatus::Skipped => "skipped",
                };
                let _ = writeln!(
                    o,
                    "| {} | {} | {} | {} | {} |",
                    name,
                    c.slot.as_deref().unwrap_or("-"),
                    c.step_index
                        .map(|index| (index + 1).to_string())
                        .unwrap_or_else(|| "whole run".into()),
                    status,
                    cell(&c.detail)
                );
            }
        }
    }
    o
}

fn short_sig(sig: &str) -> String {
    if sig.chars().count() > 12 {
        format!("{}…", sig.chars().take(12).collect::<String>())
    } else {
        sig.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invariants::InvariantKind;

    const EXAMPLE: &str = include_str!("../tests/fixtures/example_report.json");

    fn tx(error: Option<&str>) -> TxRecord {
        TxRecord {
            signature: String::new(),
            compute_units: 0,
            fee_lamports: 5000,
            error: error.map(str::to_string),
            logs: vec![],
        }
    }

    /// A flow step with one transaction that matches its outcome.
    fn step(index: usize, name: &str, outcome: StepOutcome) -> StepRecord {
        let transactions = match outcome {
            StepOutcome::Succeeded => vec![tx(None)],
            StepOutcome::Failed => vec![tx(Some("InstructionError(0, Custom(1))"))],
            StepOutcome::Skipped | StepOutcome::NotApplicable => vec![],
        };
        StepRecord {
            index,
            step: Step::flow(name),
            outcome,
            transactions,
        }
    }

    fn violated() -> InvariantCheck {
        InvariantCheck {
            step_index: None,
            balance_rows: Vec::new(),
            invariant: InvariantKind::Refundability,
            slot: None,
            status: CheckStatus::Violated,
            detail: String::new(),
        }
    }

    fn make_clean_rejection(run: &mut RunResult) {
        run.status = RunStatus::Rejected;
        run.harness_error = None;
        run.invariants.retain(|check| !check.is_violated());
        for (index, record) in run.steps.iter_mut().enumerate() {
            record.outcome = if index == 0 {
                StepOutcome::Failed
            } else {
                StepOutcome::Skipped
            };
            record.transactions = if index == 0 {
                vec![tx(Some("InstructionError(0, Custom(1))"))]
            } else {
                Vec::new()
            };
        }
        for row in &mut run.balances {
            row.expected_after = Some(row.before);
            row.actual_after = row.before;
            row.withheld_after = 0;
        }
    }

    #[test]
    fn classification_rules() {
        use StepOutcome::*;
        let u = SupportPolicy::Unspecified;
        assert_eq!(
            classify(
                &[step(0, "make", Succeeded), step(1, "refund", Succeeded)],
                &[],
                u
            ),
            RunStatus::Pass
        );
        assert_eq!(
            classify(
                &[step(0, "make", Failed), step(1, "refund", Skipped)],
                &[],
                u
            ),
            RunStatus::Rejected
        );
        assert_eq!(
            classify(
                &[step(0, "make", Failed), step(1, "refund", Skipped)],
                &[],
                SupportPolicy::MustSupport
            ),
            RunStatus::Fail
        );
        assert_eq!(
            classify(
                &[step(0, "make", Succeeded), step(1, "refund", Failed)],
                &[],
                u
            ),
            RunStatus::Fail
        );
        assert_eq!(
            classify(&[step(0, "make", Succeeded)], &[violated()], u),
            RunStatus::Fail
        );
        assert_eq!(
            classify(
                &[step(0, "make", Succeeded)],
                &[],
                SupportPolicy::MustReject
            ),
            RunStatus::Fail
        );
    }

    #[test]
    fn rejection_means_nothing_was_committed() {
        use StepOutcome::*;
        let u = SupportPolicy::Unspecified;
        // A first flow whose first transaction committed before the second
        // failed is not a clean rejection.
        let partial = StepRecord {
            transactions: vec![tx(None), tx(Some("InstructionError(0, Custom(1))"))],
            ..step(0, "make", Failed)
        };
        assert_eq!(
            classify(&[partial, step(1, "take", Skipped)], &[], u),
            RunStatus::Fail
        );
        // A clock step (no transactions) before a rejected flow keeps it a
        // rejection.
        let clock = StepRecord {
            index: 0,
            step: Step::AdvanceClock { seconds: 1 },
            outcome: Succeeded,
            transactions: vec![],
        };
        assert_eq!(
            classify(&[clock, step(1, "make", Failed)], &[], u),
            RunStatus::Rejected
        );
        // A failed issuer action is never a protocol rejection.
        let freeze = StepRecord {
            index: 0,
            step: Step::Issuer {
                action: crate::matrix::IssuerAction::FreezeCustody {
                    slot: "mint_a".into(),
                },
            },
            outcome: Failed,
            transactions: vec![tx(Some("InstructionError(0, Custom(1))"))],
        };
        assert_eq!(
            classify(&[freeze, step(1, "make", Skipped)], &[], u),
            RunStatus::Fail
        );
    }

    #[test]
    fn example_report_parses_and_matches_its_summary() {
        let r = Report::from_json(EXAMPLE).expect("example report must match the types");
        assert_eq!(r.schema_version, "0.2.0");
        assert_eq!(r.summary, Summary::of(&r.runs));
        assert_eq!(r.matrix.cases, r.runs.len());
        // Re-serializing and parsing again is lossless.
        let again = Report::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(r, again);
        let old = EXAMPLE.replace(
            "\"schema_version\": \"0.2.0\"",
            "\"schema_version\": \"0.1.0\"",
        );
        assert!(
            Report::from_json(&old).is_ok(),
            "older reports remain readable"
        );
        let mut current = r;
        current.schema_version = SCHEMA_VERSION.into();
        assert_eq!(
            Report::from_json(&current.to_json().unwrap()).unwrap(),
            current
        );
    }

    #[test]
    fn inconsistent_or_ambiguous_reports_cannot_be_used_as_ci_evidence() {
        let original = Report::from_json(EXAMPLE).unwrap();
        let mut wrong_summary = original.clone();
        wrong_summary.summary.fail = 0;
        let mut incomplete = original.clone();
        incomplete.runs.retain(|run| run.status == RunStatus::Pass);
        let mut duplicate = original.clone();
        duplicate.runs.push(duplicate.runs[0].clone());
        duplicate.matrix.cases = duplicate.runs.len();
        duplicate.summary = Summary::of(&duplicate.runs);
        for report in [wrong_summary, incomplete, duplicate] {
            assert!(Report::from_json(&report.to_json().unwrap()).is_err());
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
    }

    #[test]
    fn unsupported_or_malformed_schema_versions_fail_closed() {
        let mut report = Report::from_json(EXAMPLE).unwrap();
        for version in ["0", "0.foo.0", "0.02.0", "0.3.0.extra", "0.4.0", "1.0.0"] {
            report.schema_version = version.into();
            assert!(
                Report::from_json(&report.to_json().unwrap()).is_err(),
                "{version}"
            );
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS, "{version}");
        }
    }

    #[test]
    fn explicit_failure_evidence_cannot_be_overridden_by_a_non_failing_status() {
        let original = Report::from_json(EXAMPLE).unwrap();
        let mut flipped = original.clone();
        for run in &mut flipped.runs {
            if run.status == RunStatus::Fail {
                run.status = RunStatus::Pass;
            }
        }
        flipped.summary = Summary::of(&flipped.runs);
        assert_eq!(flipped.summary.fail, 0);
        assert_eq!(flipped.exit_code(FailOn::Fail), EXIT_HARNESS);
        assert!(Report::from_json(&flipped.to_json().unwrap()).is_err());

        for status in [
            RunStatus::Pass,
            RunStatus::Rejected,
            RunStatus::NotApplicable,
        ] {
            for harness_error in [false, true] {
                let mut report = original.clone();
                report.runs[0].status = status;
                if harness_error {
                    report.runs[0].harness_error = Some("incomplete execution".into());
                } else {
                    report.runs[0].invariants.push(violated());
                }
                report.summary = Summary::of(&report.runs);
                assert!(report.validate().is_err(), "{status:?}");
                assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
            }
        }
    }

    #[test]
    fn passing_status_requires_successful_steps_and_transactions() {
        let original = Report::from_json(EXAMPLE).unwrap();
        for outcome in [
            StepOutcome::Failed,
            StepOutcome::Skipped,
            StepOutcome::NotApplicable,
        ] {
            let mut report = original.clone();
            report.runs[0].steps[0].outcome = outcome;
            assert!(report.validate().is_err(), "{outcome:?}");
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
        let mut report = original;
        report.runs[0].steps[0].transactions[0].error =
            Some("InstructionError(0, Custom(1))".into());
        assert!(report.validate().is_err());
        assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
    }

    #[test]
    fn complete_ordered_step_evidence_is_required_for_reports() {
        let original = Report::from_json(EXAMPLE).unwrap();
        let mut missing = original.clone();
        missing.runs[0].steps.pop();
        let mut wrong_index = original.clone();
        wrong_index.runs[0].steps[0].index = 5;
        let mut wrong_step = original.clone();
        wrong_step.runs[0].steps[0].step = Step::flow("different_flow");
        let mut no_transactions = original.clone();
        no_transactions.runs[0].steps[0].transactions.clear();
        let mut no_flows = original.clone();
        no_flows.runs[0].scenario.steps.clear();
        no_flows.runs[0].steps.clear();
        no_flows.runs[0].invariants.clear();
        let mut outside_step = original;
        outside_step.runs[0].invariants[0].step_index = Some(outside_step.runs[0].steps.len());
        for report in [
            missing,
            wrong_index,
            wrong_step,
            no_transactions,
            no_flows,
            outside_step,
        ] {
            assert!(Report::from_json(&report.to_json().unwrap()).is_err());
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
    }

    #[test]
    fn rejection_and_applicability_cannot_hide_committed_or_failed_transactions() {
        let original = Report::from_json(EXAMPLE).unwrap();
        for status in [RunStatus::Rejected, RunStatus::NotApplicable] {
            let mut report = original.clone();
            for run in &mut report.runs {
                if run.status == RunStatus::Fail {
                    run.status = status;
                    run.invariants.clear();
                }
            }
            report.summary = Summary::of(&report.runs);
            assert_eq!(report.summary.fail, 0);
            assert!(report.validate().is_err(), "{status:?}");
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
        let mut partial_error = original;
        let run = &mut partial_error.runs[1];
        run.status = RunStatus::Error;
        run.harness_error = Some("signing the next transaction failed".into());
        run.invariants.clear();
        run.steps[0].outcome = StepOutcome::Failed;
        run.steps[1].outcome = StepOutcome::Skipped;
        run.steps[1].transactions.clear();
        partial_error.summary = Summary::of(&partial_error.runs);
        assert!(partial_error.validate().is_ok());
        assert_eq!(partial_error.exit_code(FailOn::Fail), EXIT_HARNESS);
    }

    #[test]
    fn modern_baseline_flags_must_match_the_mint_slots() {
        let original = Report::from_json(EXAMPLE).unwrap();
        for schema in ["0.2.0", "0.3.0"] {
            let mut report = original.clone();
            report.schema_version = schema.into();
            report.runs[0].baseline = false;
            assert!(report.validate().is_err());
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
        let mut legacy = original;
        legacy.schema_version = "0.1.0".into();
        legacy.runs[1].baseline = true;
        assert!(Report::from_json(&legacy.to_json().unwrap()).is_ok());
    }

    #[test]
    fn non_failing_statuses_require_matching_balance_evidence() {
        let original = Report::from_json(EXAMPLE).unwrap();
        for embedded in [false, true] {
            let mut report = original.clone();
            let run = &mut report.runs[0];
            let index = run
                .balances
                .iter()
                .position(|row| row.expected_after.is_some())
                .unwrap();
            let mut row = run.balances[index].clone();
            row.actual_after = row.expected_after.unwrap() + 1;
            if embedded {
                run.invariants[0].balance_rows.push(row);
            } else {
                run.balances[index] = row;
            }
            assert!(report.validate().is_err());
            assert_eq!(report.exit_code(FailOn::Fail), EXIT_HARNESS);
        }
    }

    #[test]
    fn example_report_statuses_follow_the_classifier() {
        let r = Report::from_json(EXAMPLE).unwrap();
        for run in &r.runs {
            assert_eq!(
                classify(&run.steps, &run.invariants, SupportPolicy::Unspecified),
                run.status,
                "{}",
                run.id
            );
        }
    }

    #[test]
    fn exit_codes() {
        let mut r = Report::from_json(EXAMPLE).unwrap();
        // The example's plain Token-2022 failure is not a baseline failure.
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_FAILURES);
        let baseline = r.runs.iter().position(|x| x.baseline).unwrap();
        r.runs[baseline].status = RunStatus::Fail;
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_BASELINE);
        make_clean_rejection(&mut r.runs[baseline]);
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_BASELINE);
        assert_eq!(r.exit_code(FailOn::Rejected), EXIT_BASELINE);
        assert!(baseline_note(&r).contains("1 failed"));
        r.runs[baseline] = Report::from_json(EXAMPLE).unwrap().runs[baseline].clone();
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_FAILURES);
        for run in &mut r.runs {
            if run.status == RunStatus::Fail {
                make_clean_rejection(run);
            }
        }
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_OK);
        assert_eq!(r.exit_code(FailOn::Rejected), EXIT_FAILURES);
        r.runs[0].status = RunStatus::Error;
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_HARNESS);
        for run in &mut r.runs {
            run.status = RunStatus::NotApplicable;
            run.invariants.clear();
            run.harness_error = None;
            for record in &mut run.steps {
                record.outcome = StepOutcome::Succeeded;
                record.transactions = vec![tx(None)];
            }
            let issuer = Step::Issuer {
                action: crate::matrix::IssuerAction::DrainCustody,
            };
            let index = run.steps.len();
            run.scenario.steps.push(issuer.clone());
            run.steps.push(StepRecord {
                index,
                step: issuer,
                outcome: StepOutcome::NotApplicable,
                transactions: Vec::new(),
            });
        }
        r.summary = Summary::of(&r.runs);
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_HARNESS);
        r.runs.clear();
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_HARNESS);
    }

    #[test]
    fn markdown_lists_failures_with_logs() {
        let mut r = Report::from_json(EXAMPLE).unwrap();
        r.runs[1].invariants[0].step_index = Some(0);
        let md = render_markdown(&r);
        assert!(md.contains("# ExtensionLab report: `escrow` x `default`"));
        assert!(md.contains("## Failures"));
        assert!(md.contains("### `transfer_fee_200bps@mint_a/make_take`"));
        assert!(md.contains("### `token_2022/make_refund`"));
        assert!(md.contains("transfer_fee 200 bps, uncapped"));
        assert!(md.contains("```text"));
        assert!(md.contains("| vault | mint_a |"));
        assert!(md.contains("| Invariant | Slot | Step | Status | Detail |"));
        assert!(md.contains("| conservation | mint_a | 1 |"));
    }

    #[test]
    fn markdown_handles_non_ascii_signature_text_without_panicking() {
        let mut report = Report::from_json(EXAMPLE).unwrap();
        report.runs[1].steps[0].transactions[0].signature = "a😀😀😀".into();
        assert!(render_markdown(&report).contains("a😀😀😀"));
        assert_eq!(short_sig(&"😀".repeat(13)), format!("{}…", "😀".repeat(12)));
    }
}
