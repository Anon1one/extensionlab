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
    mint::MintConfig,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Version of the JSON report schema. Bump the minor version for additive
/// changes and the major version for breaking ones.
pub const SCHEMA_VERSION: &str = "0.1.0";

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
        if self
            .runs
            .iter()
            .any(|r| r.baseline && r.status == RunStatus::Fail)
        {
            return EXIT_BASELINE;
        }
        if self.runs.iter().any(|r| r.status == RunStatus::Error) {
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

    /// Parse JSON. Rejects reports whose major schema version differs.
    pub fn from_json(text: &str) -> Result<Self> {
        let report: Report = serde_json::from_str(text)?;
        let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
        if major(&report.schema_version) != major(SCHEMA_VERSION) {
            return Err(Error::Adapter(format!(
                "report schema {} is not compatible with {}",
                report.schema_version, SCHEMA_VERSION
            )));
        }
        Ok(report)
    }
}

fn status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Pass => "pass",
        RunStatus::Rejected => "rejected",
        RunStatus::Fail => "FAIL",
        RunStatus::Error => "error",
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
        .filter(|x| x.status == RunStatus::Fail)
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
        "| Total | Pass | Rejected | Fail | Error |\n|---|---|---|---|---|"
    );
    let _ = writeln!(
        o,
        "| {} | {} | {} | {} | {} |\n",
        s.total, s.pass, s.rejected, s.fail, s.error
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
            .unwrap_or_default();
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
                "\n**Invariants**\n\n| Invariant | Slot | Status | Detail |\n|---|---|---|---|"
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
                    "| {} | {} | {} | {} |",
                    name,
                    c.slot.as_deref().unwrap_or("-"),
                    status,
                    cell(&c.detail)
                );
            }
        }
    }
    o
}

fn short_sig(sig: &str) -> String {
    if sig.len() > 12 {
        format!("{}…", &sig[..12])
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
            StepOutcome::Skipped => vec![],
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
            invariant: InvariantKind::Refundability,
            slot: None,
            status: CheckStatus::Violated,
            detail: String::new(),
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
        assert_eq!(r.schema_version, SCHEMA_VERSION);
        assert_eq!(r.summary, Summary::of(&r.runs));
        assert_eq!(r.matrix.cases, r.runs.len());
        // Re-serializing and parsing again is lossless.
        let again = Report::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(r, again);
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
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_BASELINE);
        r.runs[baseline].status = RunStatus::Pass;
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_FAILURES);
        for run in &mut r.runs {
            if run.status == RunStatus::Fail {
                run.status = RunStatus::Rejected;
            }
        }
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_OK);
        assert_eq!(r.exit_code(FailOn::Rejected), EXIT_FAILURES);
        r.runs[0].status = RunStatus::Error;
        assert_eq!(r.exit_code(FailOn::Fail), EXIT_HARNESS);
    }

    #[test]
    fn markdown_lists_failures_with_logs() {
        let r = Report::from_json(EXAMPLE).unwrap();
        let md = render_markdown(&r);
        assert!(md.contains("# ExtensionLab report: `escrow` x `default`"));
        assert!(md.contains("## Failures"));
        assert!(md.contains("### `transfer_fee_200bps@mint_a/make_take`"));
        assert!(md.contains("### `token_2022/make_refund`"));
        assert!(md.contains("transfer_fee 200 bps, uncapped"));
        assert!(md.contains("```text"));
        assert!(md.contains("| vault | mint_a |"));
    }
}
