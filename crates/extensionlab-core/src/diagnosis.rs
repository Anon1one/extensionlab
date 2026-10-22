//! Broken-assumption catalog.
//!
//! Maps the evidence in a finished run (program logs of failed transactions,
//! violated invariants, issuer actions in the scenario) to the integration
//! assumption that most likely broke. The first matching rule wins.
//!
//! This is a heuristic. Log matching depends on the exact error text the
//! programs print. Status of each log substring:
//! - withheld-fee close, frozen account, missing memo, non-transferable:
//!   checked against the Token-2022 10.0.0 binary bundled with LiteSVM 0.10.0
//!   by `tests/mint_factory.rs`, for top-level instructions sent by the
//!   harness;
//! - withheld-fee close and `ConstraintAssociated`: also observed inside the
//!   reference escrow's CPIs in `research/01_repro_escrow_fee_token.md`;
//! - CPI Guard and paused mint: checked inside the reference escrow's CPIs
//!   by `adapters/escrow/tests/golden.rs` (escrow `make` and `take` with a
//!   guarded account; `take` after a pause). The paused-mint text is also
//!   checked for top-level transfers by `tests/mint_factory.rs`. The CPI
//!   Guard substring is the part the program's log ("... without using a
//!   delegate") shares with the `spl-token-2022-interface` 2.1.0 error
//!   string ("... via CPI without using a delegate");
//! - the invariant-based rules and `transfer_hook.plain_cpi`: not checked
//!   against a real run.

use crate::{
    invariants::InvariantKind,
    matrix::{IssuerAction, Step},
    mint::TokenProgram,
    report::{Diagnosis, RunResult},
};

/// What a rule looks for.
#[derive(Debug, Clone, Copy)]
pub enum Evidence {
    /// A log line of a failed transaction contains one of these substrings.
    Log(&'static [&'static str]),
    /// One of these invariants is violated (checked in this order).
    Violation(&'static [InvariantKind]),
    /// The scenario froze custody accounts (an issuer action) and a failed
    /// transaction's logs contain one of these strings.
    FreezeThenLog(&'static [&'static str]),
    /// The scenario paused a mint (`Pause` or `PauseAll`, issuer actions) and
    /// a failed transaction's logs contain one of these strings.
    PauseThenLog(&'static [&'static str]),
    /// The scenario drained custody with the permanent delegate and one of
    /// these invariants is violated (checked in this order).
    DrainThenViolation(&'static [InvariantKind]),
    /// Any transaction failed.
    AnyFailure,
}

/// One catalog entry.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable id, e.g. `transfer_fee.close_with_withheld`.
    pub id: &'static str,
    /// Extension kind that must be present in some slot's config, or
    /// `token_2022` for rules that only need some slot to be a Token-2022
    /// mint.
    pub scope: &'static str,
    /// The assumption, in plain words.
    pub assumption: &'static str,
    /// What to look for.
    pub evidence: Evidence,
}

/// The catalog, in match order.
pub const RULES: &[Rule] = &[
    Rule {
        // Observed for the reference escrow's `refund` in
        // research/01_repro_escrow_fee_token.md (finding F1); the Anchor
        // behavior is in anchor-syn 1.1.2 src/codegen/accounts/constraints.rs.
        id: "token_2022.ata_constraint_without_program",
        scope: "token_2022",
        assumption: "An Anchor `associated_token` constraint without `associated_token::token_program` accepts Token-2022 accounts. Anchor 1.1.2 then derives the expected address with the legacy SPL Token program id, so every Token-2022 associated token account fails the check.",
        evidence: Evidence::Log(&["ConstraintAssociated"]),
    },
    Rule {
        id: "transfer_fee.close_with_withheld",
        scope: "transfer_fee",
        assumption: "A custody token account can be closed once its `amount` has been transferred out. With TransferFee, fees withheld in the account block CloseAccount until someone harvests them to the mint.",
        evidence: Evidence::Log(&["withheld fee balance is zero"]),
    },
    Rule {
        id: "freeze_authority.custody_frozen",
        scope: "freeze_authority",
        assumption: "Custody accounts stay usable for the life of a position. The mint's freeze authority can freeze them at any time, and every flow that moves custody funds then fails.",
        evidence: Evidence::FreezeThenLog(&["Account is frozen"]),
    },
    Rule {
        id: "default_frozen.new_accounts_usable",
        scope: "default_account_state_frozen",
        assumption: "Token accounts the protocol creates can receive and send tokens immediately. With DefaultAccountState(Frozen) they start frozen and only the issuer can thaw them.",
        evidence: Evidence::Log(&["Account is frozen"]),
    },
    Rule {
        id: "memo_required.no_memo_needed",
        scope: "memo_transfer_required",
        assumption: "Transfers to a user's token account need no memo. With required memos, a program must CPI the memo program immediately before its transfer CPI.",
        evidence: Evidence::Log(&["No memo in previous instruction"]),
    },
    Rule {
        id: "non_transferable.transferable",
        scope: "non_transferable",
        assumption: "Every mint the protocol handles can be transferred. A NonTransferable mint cannot be transferred at all, so every flow that moves it fails. If the entry flow moved it, the protocol refused the deposit atomically; if a later flow moves it, funds that already entered custody stay there.",
        evidence: Evidence::Log(&["Transfer is disabled for this mint"]),
    },
    Rule {
        id: "cpi_guard.user_signed_cpi",
        scope: "cpi_guard",
        assumption: "A program can move a user's tokens by CPI with the user's own signature. With CPI Guard on the user's token account, Token-2022 only allows that through a delegate the user approved in a top-level instruction.",
        evidence: Evidence::Log(&[
            "CPI Guard is enabled, and a program attempted to transfer user funds",
        ]),
    },
    Rule {
        id: "pausable.paused_mint",
        scope: "pausable",
        assumption: "Custody can always be moved out. A Pausable mint's pause authority can stop every transfer of it, including the protocol's exit, until it resumes the mint, so a protocol should treat that failure as retryable rather than as lost funds.",
        evidence: Evidence::PauseThenLog(&["paused on this mint"]),
    },
    Rule {
        id: "permanent_delegate.custody_exclusive",
        scope: "permanent_delegate",
        assumption: "Only the protocol's PDA can move tokens out of its custody accounts. A permanent delegate can, and the exit flow then pays out whatever is left (for example `vault.amount`) without detecting the shortfall.",
        evidence: Evidence::DrainThenViolation(&[
            InvariantKind::Refundability,
            InvariantKind::CreditedEqualsReceived,
            InvariantKind::ExpectedBalances,
            InvariantKind::Conservation,
        ]),
    },
    Rule {
        id: "transfer_fee.received_equals_sent",
        scope: "transfer_fee",
        assumption: "The amount that arrives equals the amount sent. With TransferFee the destination receives `amount - fee`, so balances or credits computed from the sent amount are too high.",
        evidence: Evidence::Violation(&[
            InvariantKind::CreditedEqualsReceived,
            InvariantKind::ExpectedBalances,
            InvariantKind::Refundability,
        ]),
    },
    Rule {
        id: "transfer_hook.plain_cpi",
        scope: "transfer_hook",
        assumption: "A `transfer_checked` CPI with source, mint, destination and authority is enough. A TransferHook mint also needs the hook program and its extra-account-metas accounts forwarded. With the `transfer_hook_undeployed` preset no hook program exists, so every transfer of that mint fails whatever the protocol does; such a run only shows where the protocol first moves the mint.",
        evidence: Evidence::AnyFailure,
    },
];

fn failed_logs(run: &RunResult) -> impl Iterator<Item = &String> {
    run.steps
        .iter()
        .flat_map(|s| s.transactions.iter())
        .filter(|t| t.error.is_some())
        .flat_map(|t| t.logs.iter())
}

fn failed_log_containing(run: &RunResult, needles: &[&str]) -> Option<String> {
    failed_logs(run)
        .find(|l| needles.iter().any(|n| l.contains(n)))
        .cloned()
}

fn first_violation(run: &RunResult, kinds: &[InvariantKind]) -> Option<String> {
    kinds.iter().find_map(|k| {
        run.invariants
            .iter()
            .find(|c| c.invariant == *k && c.is_violated())
            .map(|c| c.detail.clone())
    })
}

/// Diagnose a finished run. Returns `None` when no rule matches. SPL Token
/// baseline runs match nothing unless the scenario froze custody, since every
/// other rule needs an extension or a Token-2022 mint.
pub fn diagnose(run: &RunResult) -> Option<Diagnosis> {
    let drained = run.scenario.steps.iter().any(|s| {
        matches!(
            s,
            Step::Issuer {
                action: IssuerAction::DrainCustody
            }
        )
    });
    let froze = run.scenario.steps.iter().any(|s| {
        matches!(
            s,
            Step::Issuer {
                action: IssuerAction::FreezeCustody { .. }
            }
        )
    });
    let paused = run.scenario.steps.iter().any(|s| {
        matches!(
            s,
            Step::Issuer {
                action: IssuerAction::Pause { .. } | IssuerAction::PauseAll
            }
        )
    });
    for rule in RULES {
        let in_scope = run.slots.values().any(|c| match rule.scope {
            "token_2022" => c.program == TokenProgram::Token2022,
            // Every harness mint has a freeze authority (the issuer).
            "freeze_authority" => true,
            kind => c.has(kind),
        });
        if !in_scope {
            continue;
        }
        let evidence = match rule.evidence {
            Evidence::Log(needles) => failed_log_containing(run, needles),
            Evidence::Violation(kinds) => first_violation(run, kinds),
            Evidence::DrainThenViolation(kinds) if drained => first_violation(run, kinds),
            Evidence::DrainThenViolation(_) => None,
            Evidence::FreezeThenLog(needles) if froze => failed_log_containing(run, needles),
            Evidence::FreezeThenLog(_) => None,
            Evidence::PauseThenLog(needles) if paused => failed_log_containing(run, needles),
            Evidence::PauseThenLog(_) => None,
            Evidence::AnyFailure => run.first_failed_tx().map(|(_, t)| {
                t.logs
                    .last()
                    .cloned()
                    .or_else(|| t.error.clone())
                    .unwrap_or_default()
            }),
        };
        if let Some(evidence) = evidence {
            return Some(Diagnosis {
                rule: rule.id.into(),
                scope: rule.scope.into(),
                assumption: rule.assumption.into(),
                evidence,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Report;

    #[test]
    fn example_report_diagnoses_are_reproduced_by_the_catalog() {
        let r = Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        for run in &r.runs {
            assert_eq!(diagnose(run), run.diagnosis, "{}", run.id);
        }
    }

    #[test]
    fn rule_ids_are_unique() {
        let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), RULES.len());
    }
}
