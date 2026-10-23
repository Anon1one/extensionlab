//! Compare compatibility evidence between two revisions without hiding lost coverage.

use crate::{
    report::{
        BalanceRow, FailOn, Report, RunResult, RunStatus, Summary, EXIT_BASELINE, EXIT_FAILURES,
        EXIT_HARNESS, EXIT_OK,
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
    report.validate()?;
    if report.runs.is_empty() {
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

fn violation_keys(run: &RunResult, step_aware: bool) -> Vec<String> {
    let mut keys: Vec<_> = run
        .violations()
        .map(|c| {
            format!(
                "{}:{}{}",
                serde_json::to_string(&c.invariant).unwrap(),
                c.slot.as_deref().unwrap_or("*"),
                if step_aware {
                    format!("@{:?}", c.step_index)
                } else {
                    String::new()
                }
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

fn balance_changes(
    before: &[BalanceRow],
    after: &[BalanceRow],
    context: &str,
    compare_delta: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    for balance in after {
        let Some(expected) = balance.expected_after else {
            continue;
        };
        let Some(previous) = before
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
                "{context}{} balance mismatch grew: {} -> {}",
                balance.label, previous_gap, gap
            ));
        }
    }
    for previous in before.iter().filter(|b| b.expected_after.is_some()) {
        let next = after
            .iter()
            .find(|b| b.label == previous.label && b.slot == previous.slot);
        let same_expectation =
            match next.and_then(|b| b.expected_after.map(|expected| (b, expected))) {
                Some((next, expected)) if compare_delta => {
                    expected as i128 - next.before as i128
                        == previous.expected_after.unwrap() as i128 - previous.before as i128
                }
                Some((_, expected)) => Some(expected) == previous.expected_after,
                None => false,
            };
        if !same_expectation {
            reasons.push(format!(
                "{context}{} balance expectation changed or was removed",
                previous.label
            ));
        }
    }
    reasons
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
    // Reports before 0.3 did not record a check's step. Preserve their aggregate
    // comparison; once both schemas support it, lost per-step coverage must fail.
    let step_aware =
        before.schema_version.starts_with("0.3.") && after.schema_version.starts_with("0.3.");
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
        let mut old_violations = violation_keys(old, step_aware);
        for violation in violation_keys(run, step_aware) {
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
        let changes = balance_changes(&old.balances, &run.balances, "", false);
        regression |= !changes.is_empty();
        reasons.extend(changes);
        // Final balances can match after a bad deposit is followed by an equally
        // bad withdrawal. Retain and compare the evidence from each flow as well.
        let mut available: Vec<_> = run.invariants.iter().collect();
        for previous in old.invariants.iter().filter(|c| !c.balance_rows.is_empty()) {
            let context = previous
                .step_index
                .map(|index| format!("step {}: ", index + 1))
                .unwrap_or_else(|| "whole run: ".into());
            let matching = available.iter().position(|next| {
                next.invariant == previous.invariant
                    && next.slot == previous.slot
                    && (!step_aware || next.step_index == previous.step_index)
            });
            let changes = if let Some(index) = matching {
                let next = available.remove(index);
                balance_changes(
                    &previous.balance_rows,
                    &next.balance_rows,
                    &context,
                    previous.step_index.is_some(),
                )
            } else {
                vec![format!("{context}balance evidence was removed")]
            };
            regression |= !changes.is_empty();
            reasons.extend(changes);
        }
        // Credits and other checks have no structured numeric evidence yet.
        // A changed, still-violated check needs review rather than being called
        // unchanged merely because its invariant kind stayed the same.
        let mut available: Vec<_> = run.invariants.iter().collect();
        for previous in old
            .violations()
            .filter(|check| check.balance_rows.is_empty())
        {
            let same_check = |next: &&crate::invariants::InvariantCheck| {
                next.invariant == previous.invariant
                    && next.slot == previous.slot
                    && (!step_aware || next.step_index == previous.step_index)
            };
            let matching = available
                .iter()
                .position(|next| {
                    same_check(next)
                        && next.status == crate::invariants::CheckStatus::Violated
                        && next.detail == previous.detail
                })
                .or_else(|| {
                    available.iter().position(|next| {
                        same_check(next) && next.status == crate::invariants::CheckStatus::Violated
                    })
                })
                .or_else(|| available.iter().position(same_check));
            if let Some(index) = matching {
                let next = available.remove(index);
                if next.status == crate::invariants::CheckStatus::Violated
                    && next.detail != previous.detail
                {
                    reasons.push(format!(
                        "{:?} ({}), step {:?}: unstructured failure evidence changed; review required",
                        previous.invariant, previous.slot.as_deref().unwrap_or("*"), previous.step_index
                    ));
                    regression = true;
                }
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
                    "{:?} ({}){}",
                    check.invariant,
                    check.slot.as_deref().unwrap_or("*"),
                    if step_aware {
                        format!("@{:?}", check.step_index)
                    } else {
                        String::new()
                    }
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
        .any(|r| r.baseline && matches!(r.status, RunStatus::Fail | RunStatus::Rejected))
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

    fn make_clean_rejection(run: &mut RunResult) {
        let mut failed = run
            .steps
            .iter()
            .flat_map(|step| &step.transactions)
            .next()
            .unwrap()
            .clone();
        failed.error = Some("InstructionError(0, Custom(1))".into());
        run.status = RunStatus::Rejected;
        run.harness_error = None;
        run.invariants.retain(|check| !check.is_violated());
        for (index, record) in run.steps.iter_mut().enumerate() {
            record.outcome = if index == 0 {
                crate::report::StepOutcome::Failed
            } else {
                crate::report::StepOutcome::Skipped
            };
            record.transactions = if index == 0 {
                vec![failed.clone()]
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

    fn failing_flow_report() -> Report {
        let mut report = report();
        report.schema_version = crate::report::SCHEMA_VERSION.into();
        let mut run = report.runs[0].clone();
        run.baseline = false;
        run.id = "token_2022/deposit_withdraw".into();
        run.slots = report.runs[1].slots.clone();
        run.status = RunStatus::Fail;
        for balance in &mut run.balances {
            balance.expected_after = Some(balance.actual_after);
        }
        let mut check = run.invariants[0].clone();
        check.invariant = InvariantKind::ExpectedBalances;
        check.status = CheckStatus::Violated;
        check.step_index = Some(0);
        check.detail = "the flow moved less than its declared amount".into();
        check.balance_rows = vec![BalanceRow {
            label: "vault".into(),
            address: solana_pubkey::Pubkey::default(),
            slot: "mint_a".into(),
            before: 0,
            expected_after: Some(100),
            actual_after: 75,
            withheld_after: 0,
            exists_after: true,
        }];
        run.invariants = vec![check];
        report.runs = vec![run];
        recount(&mut report);
        report
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
        let mut before = report();
        let issuer = crate::matrix::Step::Issuer {
            action: crate::matrix::IssuerAction::FreezeCustody {
                slot: "mint_a".into(),
            },
        };
        let successful_tx = before.runs[0].steps[0].transactions[0].clone();
        let index = before.runs[0].steps.len();
        before.runs[0].scenario.steps.push(issuer.clone());
        before.runs[0].steps.push(crate::report::StepRecord {
            index,
            step: issuer,
            outcome: crate::report::StepOutcome::Succeeded,
            transactions: vec![successful_tx],
        });
        for (status, code) in [
            (RunStatus::Error, EXIT_HARNESS),
            (RunStatus::NotApplicable, EXIT_FAILURES),
        ] {
            let mut after = before.clone();
            after.runs[0].status = status;
            if status == RunStatus::NotApplicable {
                let issuer = after.runs[0].steps.last_mut().unwrap();
                issuer.outcome = crate::report::StepOutcome::NotApplicable;
                issuer.transactions.clear();
            }
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
    fn even_a_known_clean_baseline_rejection_fails_the_gate() {
        let mut report = report();
        make_clean_rejection(&mut report.runs[0]);
        recount(&mut report);
        assert_eq!(
            compare(&report, &report, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_BASELINE
        );
    }

    #[test]
    fn indexed_checks_cannot_replace_coverage_of_a_different_step() {
        let mut before = report();
        before.schema_version = crate::report::SCHEMA_VERSION.into();
        before.runs[0].invariants[0].step_index = Some(0);
        let mut after = before.clone();
        after.runs[0].invariants[0].step_index = Some(1);
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result.changes[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("invariant coverage lost")));
    }

    #[test]
    fn legacy_aggregate_coverage_matches_indexed_checks_without_reusing_them() {
        let mut before = report();
        let mut after = before.clone();
        after.schema_version = crate::report::SCHEMA_VERSION.into();
        after.runs[0].invariants[0].step_index = Some(0);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
        let duplicate = before.runs[0].invariants[0].clone();
        before.runs[0].invariants.push(duplicate);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
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
    fn worse_per_flow_gaps_fail_even_when_final_balances_match() {
        let before = failing_flow_report();
        assert_eq!(
            compare(&before, &before, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
        let mut after = before.clone();
        after.runs[0].invariants[0].balance_rows[0].actual_after = 50;
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result.changes[0].reasons.iter().any(|reason| {
            reason.contains("step 1") && reason.contains("balance mismatch grew: 25 -> 50")
        }));
        after = before.clone();
        after.runs[0].invariants[0].balance_rows.clear();
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
        after = before.clone();
        after.runs[0].invariants[0].balance_rows[0].expected_after = Some(75);
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_FAILURES
        );
    }

    #[test]
    fn per_flow_expectations_compare_deltas_when_starting_balances_change() {
        let before = failing_flow_report();
        let mut after = before.clone();
        let row = &mut after.runs[0].invariants[0].balance_rows[0];
        row.before = 25;
        row.expected_after = Some(125);
        row.actual_after = 100;
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
    }

    #[test]
    fn changed_unstructured_credit_failure_requires_review() {
        let mut before = failing_flow_report();
        let mut credit = before.runs[0].invariants[0].clone();
        credit.invariant = InvariantKind::CreditedEqualsReceived;
        credit.balance_rows.clear();
        credit.detail = "protocol credits 101, custody has 0".into();
        before.runs[0].invariants.push(credit);
        assert_eq!(
            compare(&before, &before, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
        let mut after = before.clone();
        after.runs[0].invariants[1].detail = "protocol credits 200, custody has 0".into();
        let result = compare(&before, &after, FailOn::Fail, false).unwrap();
        assert_eq!(result.exit_code, EXIT_FAILURES);
        assert!(result.changes[0].reasons.iter().any(
            |reason| reason.contains("unstructured failure evidence changed; review required")
        ));
        after.runs[0].invariants[1].status = CheckStatus::Held;
        assert_eq!(
            compare(&before, &after, FailOn::Fail, false)
                .unwrap()
                .exit_code,
            EXIT_OK
        );
    }

    #[test]
    fn rejection_policy_and_new_failing_cases_are_respected() {
        let mut before = report();
        let successful_tx = before.runs[0].steps[0].transactions[0].clone();
        let passing = before.runs.last_mut().unwrap();
        passing.status = RunStatus::Pass;
        for step in &mut passing.steps {
            step.outcome = crate::report::StepOutcome::Succeeded;
            for tx in &mut step.transactions {
                tx.error = None;
            }
            if matches!(step.step, crate::matrix::Step::Flow { .. }) && step.transactions.is_empty()
            {
                step.transactions.push(successful_tx.clone());
            }
        }
        recount(&mut before);
        let mut after = before.clone();
        make_clean_rejection(after.runs.last_mut().unwrap());
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
