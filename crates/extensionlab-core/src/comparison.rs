//! Compare compatibility evidence between two revisions without hiding lost coverage.

use crate::{
    report::{
        FailOn, Report, RunResult, RunStatus, Summary, EXIT_BASELINE, EXIT_FAILURES, EXIT_HARNESS,
        EXIT_OK,
    },
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Write};

/// One changed, added, or removed case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub before: Option<RunStatus>,
    pub after: Option<RunStatus>,
    /// Why this case changed. Empty for unchanged cases, which are omitted.
    pub reasons: Vec<String>,
    /// A new failure, worse evidence, or lost coverage.
    pub regression: bool,
}

/// Deterministic comparison; addresses, signatures and compute units are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub adapter: String,
    pub matrix: String,
    pub before: Summary,
    pub after: Summary,
    pub unchanged: usize,
    pub changes: Vec<Change>,
    /// Token-runtime differences must be acknowledged explicitly by the caller.
    pub runtime_changes: Vec<String>,
    pub exit_code: u8,
}

fn cases(report: &Report) -> Result<BTreeMap<&str, &RunResult>> {
    if report.runs.is_empty()
        || report.matrix.cases != report.runs.len()
        || report.summary != Summary::of(&report.runs)
    {
        return Err(Error::Adapter(
            "report is empty or its counts do not match its runs".into(),
        ));
    }
    let mut cases = BTreeMap::new();
    for run in &report.runs {
        if cases.insert(run.id.as_str(), run).is_some() {
            return Err(Error::Adapter(format!("duplicate run id `{}`", run.id)));
        }
    }
    Ok(cases)
}

fn bad(status: RunStatus, fail_on: FailOn) -> bool {
    matches!(status, RunStatus::Fail | RunStatus::Error)
        || (fail_on == FailOn::Rejected && status == RunStatus::Rejected)
}

fn violation_keys(run: &RunResult) -> Vec<String> {
    let mut keys: Vec<_> = run
        .violations()
        .map(|c| {
            format!(
                "{}:{}",
                serde_json::to_string(&c.invariant).unwrap(),
                c.slot.as_deref().unwrap_or("*")
            )
        })
        .collect();
    keys.sort();
    keys
}

fn failure_key(run: &RunResult) -> Option<String> {
    run.first_failed_tx().map(|(step, tx)| {
        format!(
            "{}: {}",
            step.step.describe(),
            tx.error.as_deref().unwrap_or_default()
        )
    })
}

/// Compare like-for-like cases and fail CI on new failures or reduced coverage.
/// Known failures remain visible but do not by themselves fail this gate.
/// Baseline failures and harness errors always fail, even when already known.
pub fn compare(
    before: &Report,
    after: &Report,
    fail_on: FailOn,
    allow_runtime_change: bool,
) -> Result<Comparison> {
    if before.adapter.name != after.adapter.name
        || before.tool != after.tool
        || before.matrix.name != after.matrix.name
        || before.matrix.strategy != after.matrix.strategy
        || before.environment.seed != after.environment.seed
    {
        return Err(Error::Adapter(
            "comparison requires the same tool version, adapter, matrix name, slot strategy and seed".into(),
        ));
    }
    let previous = cases(before)?;
    let current = cases(after)?;
    let mut result = Comparison {
        adapter: after.adapter.name.clone(),
        matrix: after.matrix.name.clone(),
        before: before.summary,
        after: after.summary,
        unchanged: 0,
        changes: Vec::new(),
        runtime_changes: Vec::new(),
        exit_code: EXIT_OK,
    };
    if before.environment.litesvm != after.environment.litesvm {
        result.runtime_changes.push(format!(
            "LiteSVM: {} -> {}",
            before.environment.litesvm, after.environment.litesvm
        ));
    }
    // Protocol ELF changes are expected between revisions. Token and ATA
    // runtime changes are shown separately so they cannot silently change the experiment.
    for name in [
        "spl_token",
        "spl_token_2022",
        "spl_associated_token_account",
    ] {
        let a = before.environment.programs.iter().find(|p| p.name == name);
        let b = after.environment.programs.iter().find(|p| p.name == name);
        let same = match (a, b) {
            (Some(a), Some(b)) => {
                a.program_id == b.program_id
                    && match (&a.sha256, &b.sha256) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => a.source == b.source,
                        _ => false,
                    }
            }
            (None, None) => true,
            _ => false,
        };
        if !same {
            result
                .runtime_changes
                .push(format!("{name}: binary identity changed or is missing"));
        }
    }
    for (id, run) in &current {
        let Some(old) = previous.get(id) else {
            result.changes.push(Change {
                id: (*id).into(),
                before: None,
                after: Some(run.status),
                reasons: vec!["case added".into()],
                regression: bad(run.status, fail_on),
            });
            continue;
        };
        if old.slots != run.slots
            || old.scenario != run.scenario
            || old.varied_slot != run.varied_slot
            || old.baseline != run.baseline
        {
            result.changes.push(Change {
                id: (*id).into(),
                before: Some(old.status),
                after: Some(run.status),
                reasons: vec![
                    "case definition changed; outcomes are not directly comparable".into(),
                ],
                regression: true,
            });
            continue;
        }
        let mut reasons = Vec::new();
        let mut regression = false;
        if old.status != run.status {
            reasons.push(format!("status: {:?} -> {:?}", old.status, run.status));
            regression |= bad(run.status, fail_on) && !bad(old.status, fail_on);
            regression |=
                run.status == RunStatus::NotApplicable && old.status != RunStatus::NotApplicable;
        }
        let mut old_violations = violation_keys(old);
        for violation in violation_keys(run) {
            if let Some(index) = old_violations.iter().position(|v| v == &violation) {
                old_violations.remove(index);
            } else {
                reasons.push(format!("new invariant violation: {violation}"));
                regression = true;
            }
        }
        if let Some(failure) = failure_key(run) {
            if failure_key(old).as_ref() != Some(&failure) && bad(run.status, fail_on) {
                reasons.push(format!("new transaction failure: {failure}"));
                regression = true;
            }
        }
        for balance in &run.balances {
            let Some(expected) = balance.expected_after else {
                continue;
            };
            let Some(previous) = old
                .balances
                .iter()
                .find(|b| b.label == balance.label && b.slot == balance.slot)
            else {
                continue;
            };
            let Some(previous_expected) = previous.expected_after else {
                continue;
            };
            let gap = balance.actual_after.abs_diff(expected);
            let previous_gap = previous.actual_after.abs_diff(previous_expected);
            if gap > previous_gap {
                reasons.push(format!(
                    "{} balance mismatch grew: {} -> {}",
                    balance.label, previous_gap, gap
                ));
                regression = true;
            }
        }
        for previous in old.balances.iter().filter(|b| b.expected_after.is_some()) {
            let next = run
                .balances
                .iter()
                .find(|b| b.label == previous.label && b.slot == previous.slot);
            if next.and_then(|b| b.expected_after) != previous.expected_after {
                reasons.push(format!(
                    "{} balance expectation changed or was removed",
                    previous.label
                ));
                regression = true;
            }
        }
        // A still-failing case can also lose checks; do not call that improvement.
        let counts = |r: &RunResult| {
            let mut counts = BTreeMap::<String, usize>::new();
            for check in r
                .invariants
                .iter()
                .filter(|c| c.status != crate::invariants::CheckStatus::Skipped)
            {
                let key = format!(
                    "{:?} ({})",
                    check.invariant,
                    check.slot.as_deref().unwrap_or("*")
                );
                *counts.entry(key).or_default() += 1;
            }
            counts
        };
        let current_counts = counts(run);
        for (key, count) in counts(old) {
            if current_counts.get(&key).copied().unwrap_or(0) < count {
                reasons.push(format!("invariant coverage lost: {key}"));
                regression = true;
            }
        }
        if reasons.is_empty() {
            result.unchanged += 1;
        } else {
            result.changes.push(Change {
                id: (*id).into(),
                before: Some(old.status),
                after: Some(run.status),
                reasons,
                regression,
            });
        }
    }
    for (id, old) in &previous {
        if !current.contains_key(id) {
            result.changes.push(Change {
                id: (*id).into(),
                before: Some(old.status),
                after: None,
                reasons: vec!["case removed; coverage lost".into()],
                regression: true,
            });
        }
    }
    result.changes.sort_by(|a, b| a.id.cmp(&b.id));
    result.exit_code = if after
        .runs
        .iter()
        .any(|r| r.baseline && r.status == RunStatus::Fail)
    {
        EXIT_BASELINE
    } else if after.runs.iter().any(|r| r.status == RunStatus::Error)
        || after
            .runs
            .iter()
            .all(|r| r.status == RunStatus::NotApplicable)
        || (!allow_runtime_change && !result.runtime_changes.is_empty())
    {
        EXIT_HARNESS
    } else if result.changes.iter().any(|c| c.regression) {
        EXIT_FAILURES
    } else {
        EXIT_OK
    };
    Ok(result)
}

fn cell(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "\\|")
        .replace(['\n', '\r'], " ")
        .replace('`', "&#96;")
}

/// A compact pull-request summary; the full reports retain the transaction evidence.
pub fn render_markdown(comparison: &Comparison) -> String {
    let mut out = format!(
        "# ExtensionLab compatibility changes\n\nAdapter: `{}` · Matrix: `{}`\n\n",
        cell(&comparison.adapter),
        cell(&comparison.matrix)
    );
    let regressions = comparison.changes.iter().filter(|c| c.regression).count();
    let _ = writeln!(out, "{} unchanged cases · {} changed cases · {} regressions or coverage losses · gate exit {}\n", comparison.unchanged, comparison.changes.len(), regressions, comparison.exit_code);
    if !comparison.runtime_changes.is_empty() {
        let _ = writeln!(
            out,
            "Token runtime changed: {}.\n",
            cell(&comparison.runtime_changes.join("; "))
        );
    }
    let _ = writeln!(
        out,
        "| Case | Before | After | Gate | Detail |\n|---|---|---|---|---|"
    );
    for change in &comparison.changes {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            cell(&change.id),
            change
                .before
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "—".into()),
            change
                .after
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "—".into()),
            if change.regression { "FAIL" } else { "info" },
            cell(&change.reasons.join("; "))
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invariants::{CheckStatus, InvariantKind};
    const FIXTURE: &str = include_str!("../tests/fixtures/example_report.json");
    fn report() -> Report {
        Report::from_json(FIXTURE).unwrap()
    }
    fn recount(report: &mut Report) {
        report.summary = Summary::of(&report.runs);
        report.matrix.cases = report.runs.len();
    }

    #[test]
    fn known_failures_are_visible_without_failing_the_change_gate() {
        let report = report();
        let result = compare(&report, &report, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_OK);
        assert_eq!(result.unchanged, 4);
        assert_eq!(result.after.fail, 2);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn new_failures_and_lost_cases_fail_the_gate() {
        let before = report();
        let mut after = before.clone();
        after.runs[0].status = RunStatus::Fail;
        recount(&mut after);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_BASELINE
        );
        after.runs.remove(0);
        recount(&mut after);
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result.changes[0].reasons[0].contains("coverage lost"));
    }

    #[test]
    fn harness_errors_and_lost_applicability_never_turn_green() {
        let before = report();
        for (status, code) in [
            (RunStatus::Error, EXIT_HARNESS),
            (RunStatus::NotApplicable, EXIT_FAILURES),
        ] {
            let mut after = before.clone();
            after.runs[0].status = status;
            recount(&mut after);
            assert_eq!(
                compare(&before, &after, FailOn::Fail, false)
                    .unwrap()
                    .exit_code,
                code
            );
        }
    }

    #[test]
    fn a_new_bug_in_an_already_failing_case_is_a_regression() {
        let before = report();
        let mut after = before.clone();
        let check = after.runs[1]
            .invariants
            .iter_mut()
            .find(|c| c.invariant == InvariantKind::Conservation)
            .unwrap();
        check.status = CheckStatus::Violated;
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result
            .changes
            .iter()
            .any(|c| c.reasons.iter().any(|r| r.contains("new invariant"))));
    }

    #[test]
    fn case_definitions_and_runtime_changes_cannot_silently_change_the_experiment() {
        let before = report();
        let mut after = before.clone();
        after.runs[0].slots.get_mut("mint_a").unwrap().decimals = 8;
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
        after = before.clone();
        after.environment.programs[1].sha256 = Some("a".repeat(64));
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_HARNESS
        );
        assert_eq!(
            compare(&before, &after, FailOn::Fail, true)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
    }

    #[test]
    fn rejects_ambiguous_or_inconsistent_reports() {
        let before = report();
        let mut after = before.clone();
        after.runs.push(after.runs[0].clone());
        recount(&mut after);
        assert!(compare(&before, &after, FailOn::Fail, false).is_err());
        after = before.clone();
        after.adapter.name = "another-program".into();
        assert!(compare(&before, &after, FailOn::Fail, false).is_err());
        after = before.clone();
        after.summary.fail = 0;
        assert!(compare(&before, &after, FailOn::Fail, false).is_err());
    }

    #[test]
    fn worse_balance_evidence_and_removed_expectations_fail_the_gate() {
        let before = report();
        let mut after = before.clone();
        let row = after.runs[1]
            .balances
            .iter_mut()
            .find(|b| b.expected_after.is_some() && b.actual_after > b.expected_after.unwrap())
            .unwrap();
        row.actual_after += 1;
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result
            .changes
            .iter()
            .any(|c| c.reasons.iter().any(|r| r.contains("mismatch grew"))));
        after = before.clone();
        after.runs[1]
            .balances
            .iter_mut()
            .find(|b| b.expected_after.is_some())
            .unwrap()
            .expected_after = None;
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
    }

    #[test]
    fn rejection_policy_and_new_failing_cases_are_respected() {
        let mut before = report();
        before.runs.last_mut().unwrap().status = RunStatus::Pass;
        recount(&mut before);
        let mut after = before.clone();
        after.runs.last_mut().unwrap().status = RunStatus::Rejected;
        recount(&mut after);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
        assert_eq!(
            compare(&before, &after, FailOn::Rejected, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
        after = before.clone();
        let mut new = after.runs[1].clone();
        new.id = "new_failure".into();
        after.runs.push(new);
        recount(&mut after);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
    }
}
