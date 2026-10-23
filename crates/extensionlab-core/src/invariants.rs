//! Invariants: pure checks over balance snapshots.
//!
//! The runner captures a [`Snapshot`] of every tracked
//! account and every mint before and after each step, then calls these
//! functions. They take plain data, so they are unit-tested here without a VM.
//!
//! | Invariant | Question it answers |
//! |---|---|
//! | [`conservation`] | Per mint: did tokens only move between tracked accounts, the mint's withheld pool, and supply changes? (sum in == sum out + fees) |
//! | [`expected_balances`] | Did each account change by what the protocol author assumed? |
//! | [`credited_equals_received`] | Does what the protocol's state credits equal what its custody account actually received, and is custody solvent? |
//! | [`refundability`] | After a successful entry, did the exit flow succeed and return the deposit, minus the fees the config predicts? |
//! | [`closeability`] | After a successful exit, are the accounts the protocol says it closes actually gone? |

use crate::{
    adapter::{Credit, ExpectedDelta},
    report::BalanceRow,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::BTreeMap;

/// Token balance of one tracked account at one point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    /// Tracked account label.
    pub label: String,
    /// Address.
    #[serde(with = "crate::b58")]
    pub address: Pubkey,
    /// Mint slot.
    pub slot: String,
    /// False when no account exists at the address (never created, or
    /// closed). `amount`, `withheld` and `lamports` are then 0.
    pub exists: bool,
    /// Spendable token balance.
    pub amount: u64,
    /// Transfer fees withheld in the account.
    pub withheld: u64,
    /// Lamports.
    pub lamports: u64,
}

/// Supply and withheld pool of one mint at one point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintSnapshot {
    /// Mint slot.
    pub slot: String,
    /// Total supply.
    pub supply: u64,
    /// Fees harvested to the mint.
    pub withheld: u64,
}

/// All tracked balances at one point in time.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Tracked token accounts.
    pub accounts: Vec<AccountSnapshot>,
    /// Mints.
    pub mints: Vec<MintSnapshot>,
}

impl Snapshot {
    /// Account by label.
    pub fn account(&self, label: &str) -> Option<&AccountSnapshot> {
        self.accounts.iter().find(|a| a.label == label)
    }

    /// Mint by slot.
    pub fn mint(&self, slot: &str) -> Option<&MintSnapshot> {
        self.mints.iter().find(|m| m.slot == slot)
    }
}

/// Which invariant a check is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvariantKind {
    /// Per-mint token conservation.
    Conservation,
    /// Actual balance changes equal the adapter's expected changes.
    ExpectedBalances,
    /// Protocol credits equal what custody received; custody is solvent.
    CreditedEqualsReceived,
    /// The exit flow succeeds after a successful entry and returns the deposit.
    Refundability,
    /// Accounts the protocol closes are gone after the exit flow.
    Closeability,
}

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The invariant holds.
    Held,
    /// The invariant is broken.
    Violated,
    /// The check did not apply (for example no credits exposed, or the entry
    /// flow never succeeded).
    Skipped,
}

/// Result of one invariant check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantCheck {
    /// Expected and observed balances for this check, when it compares balances.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub balance_rows: Vec<BalanceRow>,
    /// Which invariant.
    pub invariant: InvariantKind,
    /// Mint slot the check is scoped to, when it is per mint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// Scenario step that produced this check. `None` denotes a check over
    /// the whole scenario; older reports do not contain step metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    /// Outcome.
    pub status: CheckStatus,
    /// Human-readable explanation with the numbers involved.
    pub detail: String,
}

impl InvariantCheck {
    fn new(
        invariant: InvariantKind,
        slot: Option<String>,
        status: CheckStatus,
        detail: String,
    ) -> Self {
        Self {
            balance_rows: Vec::new(),
            invariant,
            slot,
            step_index: None,
            status,
            detail,
        }
    }

    /// True when the check is [`CheckStatus::Violated`].
    pub fn is_violated(&self) -> bool {
        self.status == CheckStatus::Violated
    }
}

fn sum_tracked(s: &Snapshot, slot: &str) -> i128 {
    s.accounts
        .iter()
        .filter(|a| a.slot == slot)
        .map(|a| a.amount as i128 + a.withheld as i128)
        .sum()
}

/// Per-mint conservation between two snapshots.
///
/// For each mint slot: change in (amount + withheld) summed over tracked
/// accounts, plus change in the mint's withheld pool, must equal the change in
/// supply. A mismatch means tokens moved to or from an account the adapter
/// does not track (for example a permanent-delegate drain to an outside
/// account), or the adapter's tracked set is incomplete.
pub fn conservation(before: &Snapshot, after: &Snapshot) -> Vec<InvariantCheck> {
    after
        .mints
        .iter()
        .map(|m_after| {
            let slot = m_after.slot.clone();
            let Some(m_before) = before.mint(&slot) else {
                return InvariantCheck::new(
                    InvariantKind::Conservation,
                    Some(slot),
                    CheckStatus::Skipped,
                    "mint missing from the earlier snapshot".into(),
                );
            };
            let tracked = sum_tracked(after, &slot) - sum_tracked(before, &slot);
            let pool = m_after.withheld as i128 - m_before.withheld as i128;
            let supply = m_after.supply as i128 - m_before.supply as i128;
            let unaccounted = supply - (tracked + pool);
            let numbers = format!(
                "tracked accounts changed by {tracked}, mint withheld pool by {pool}, supply by {supply}"
            );
            if unaccounted == 0 {
                InvariantCheck::new(InvariantKind::Conservation, Some(slot), CheckStatus::Held, numbers)
            } else {
                InvariantCheck::new(
                    InvariantKind::Conservation,
                    Some(slot),
                    CheckStatus::Violated,
                    format!(
                        "{numbers}; {} base units moved {} accounts the adapter does not track",
                        unaccounted.unsigned_abs(),
                        // Positive: tracked balances fell by more than supply did.
                        if unaccounted > 0 { "to" } else { "from" }
                    ),
                )
            }
        })
        .collect()
}

/// Compare actual balance changes with the adapter's expected deltas.
///
/// Returns one [`BalanceRow`] per tracked account in `after` (with
/// `expected_after` set only for accounts listed in `expected`) and one check.
/// An expected label that is not tracked is an adapter error.
pub fn expected_balances(
    before: &Snapshot,
    after: &Snapshot,
    expected: &[ExpectedDelta],
) -> Result<(Vec<BalanceRow>, InvariantCheck)> {
    let mut want: BTreeMap<&str, i128> = BTreeMap::new();
    for e in expected {
        if after.account(&e.label).is_none() {
            return Err(Error::Adapter(format!(
                "expected delta for `{}`, which is not a tracked account",
                e.label
            )));
        }
        *want.entry(e.label.as_str()).or_default() += e.delta;
    }

    let mut rows = Vec::new();
    let mut mismatches = Vec::new();
    for a in &after.accounts {
        let before_amount = before.account(&a.label).map(|b| b.amount).unwrap_or(0);
        let expected_after = want
            .get(a.label.as_str())
            .map(|d| before_amount as i128 + d);
        if let Some(exp) = expected_after {
            if exp != a.amount as i128 {
                let diff = a.amount as i128 - exp;
                mismatches.push(format!(
                    "{}: expected {exp}, actual {} ({}{})",
                    a.label,
                    a.amount,
                    if diff < 0 { "short " } else { "over " },
                    diff.unsigned_abs()
                ));
            }
        }
        rows.push(BalanceRow {
            label: a.label.clone(),
            address: a.address,
            slot: a.slot.clone(),
            before: before_amount,
            expected_after: expected_after.and_then(|v| u64::try_from(v).ok()),
            actual_after: a.amount,
            withheld_after: a.withheld,
            exists_after: a.exists,
        });
    }
    let mut check = if want.is_empty() {
        InvariantCheck::new(
            InvariantKind::ExpectedBalances,
            None,
            CheckStatus::Skipped,
            "adapter declared no expected deltas".into(),
        )
    } else if mismatches.is_empty() {
        InvariantCheck::new(
            InvariantKind::ExpectedBalances,
            None,
            CheckStatus::Held,
            format!("{} account(s) changed as expected", want.len()),
        )
    } else {
        InvariantCheck::new(
            InvariantKind::ExpectedBalances,
            None,
            CheckStatus::Violated,
            mismatches.join("; "),
        )
    };
    check.balance_rows = rows.clone();
    Ok((rows, check))
}

/// Credits versus custody.
///
/// For every backing account: the change in the sum of credits backed by it
/// must equal the change in its `amount` (credited == received), and the sum
/// of credits after must not exceed its `amount` (solvency).
pub fn credited_equals_received(
    credits_before: &[Credit],
    credits_after: &[Credit],
    before: &Snapshot,
    after: &Snapshot,
) -> InvariantCheck {
    if credits_before.is_empty() && credits_after.is_empty() {
        return InvariantCheck::new(
            InvariantKind::CreditedEqualsReceived,
            None,
            CheckStatus::Skipped,
            "adapter exposes no credit readers".into(),
        );
    }
    let total = |cs: &[Credit]| {
        let mut m: BTreeMap<String, i128> = BTreeMap::new();
        for c in cs {
            *m.entry(c.backed_by.clone()).or_default() += c.amount as i128;
        }
        m
    };
    let (tb, ta) = (total(credits_before), total(credits_after));
    let mut problems = Vec::new();
    let backing: std::collections::BTreeSet<&String> = tb.keys().chain(ta.keys()).collect();
    for label in backing {
        let credited = ta.get(label).copied().unwrap_or(0) - tb.get(label).copied().unwrap_or(0);
        let held_after = after.account(label).map(|a| a.amount as i128).unwrap_or(0);
        let held_before = before.account(label).map(|a| a.amount as i128).unwrap_or(0);
        let received = held_after - held_before;
        if credited != received {
            problems.push(format!("{label}: credited {credited}, received {received}"));
        }
        let owed = ta.get(label).copied().unwrap_or(0);
        if owed > held_after {
            problems.push(format!(
                "{label}: insolvent, owes {owed} but holds {held_after}"
            ));
        }
    }
    if problems.is_empty() {
        InvariantCheck::new(
            InvariantKind::CreditedEqualsReceived,
            None,
            CheckStatus::Held,
            "credits match custody balances".into(),
        )
    } else {
        InvariantCheck::new(
            InvariantKind::CreditedEqualsReceived,
            None,
            CheckStatus::Violated,
            problems.join("; "),
        )
    }
}

/// What happened when the harness tried the exit flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitAttempt {
    /// Exit flow name.
    pub flow: String,
    /// Whether every transaction of the flow succeeded.
    pub succeeded: bool,
    /// Rendered error of the failing transaction, if any.
    pub error: Option<String>,
}

/// Refundability of a position.
///
/// `deposited` is what left the depositor in the entry flow; `returned` is
/// what came back to the depositor (or counterparty, for a swap) in the exit
/// flow; `allowed_loss` is the verified token-fee allowance for the round trip.
/// The runner observes fees in account and mint withheld pools so that each
/// individual transfer's rounding and cap are counted. Pass `entry_succeeded = false` when
/// the entry flow failed: there is nothing to refund and the check is skipped.
pub fn refundability(
    entry_succeeded: bool,
    exit: &ExitAttempt,
    deposited: u64,
    returned: u64,
    allowed_loss: u64,
) -> InvariantCheck {
    let k = InvariantKind::Refundability;
    if !entry_succeeded {
        return InvariantCheck::new(
            k,
            None,
            CheckStatus::Skipped,
            "entry flow did not succeed".into(),
        );
    }
    if !exit.succeeded {
        return InvariantCheck::new(
            k,
            None,
            CheckStatus::Violated,
            format!(
                "exit flow `{}` failed after a successful entry; the {deposited} base units deposited were not returned: {}",
                exit.flow,
                exit.error.as_deref().unwrap_or("no error recorded")
            ),
        );
    }
    let loss = deposited.saturating_sub(returned);
    if loss > allowed_loss {
        InvariantCheck::new(
            k,
            None,
            CheckStatus::Violated,
            format!(
                "exit flow `{}` returned {returned} of {deposited}; loss {loss} exceeds the {allowed_loss} the config's fees explain",
                exit.flow
            ),
        )
    } else {
        InvariantCheck::new(
            k,
            None,
            CheckStatus::Held,
            format!(
                "exit flow `{}` returned {returned} of {deposited} (fee-explained loss {loss})",
                exit.flow
            ),
        )
    }
}

/// Closeability: every label in `closable` must refer to an account that no
/// longer exists in `after`.
pub fn closeability(closable: &[String], after: &Snapshot) -> InvariantCheck {
    let k = InvariantKind::Closeability;
    if closable.is_empty() {
        return InvariantCheck::new(
            k,
            None,
            CheckStatus::Skipped,
            "adapter declares no closable accounts".into(),
        );
    }
    let mut open = Vec::new();
    for label in closable {
        match after.account(label) {
            Some(a) if a.exists => open.push(format!(
                "{label} still open (amount {}, withheld {}, lamports {})",
                a.amount, a.withheld, a.lamports
            )),
            Some(_) => {}
            None => open.push(format!("{label} is not a tracked account")),
        }
    }
    if open.is_empty() {
        InvariantCheck::new(
            k,
            None,
            CheckStatus::Held,
            format!("{} account(s) closed", closable.len()),
        )
    } else {
        InvariantCheck::new(k, None, CheckStatus::Violated, open.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(label: &str, slot: &str, amount: u64, withheld: u64) -> AccountSnapshot {
        AccountSnapshot {
            label: label.into(),
            address: Pubkey::new_from_array([label.len() as u8; 32]),
            slot: slot.into(),
            exists: true,
            amount,
            withheld,
            lamports: 2_039_280,
        }
    }

    fn mint(slot: &str, supply: u64, withheld: u64) -> MintSnapshot {
        MintSnapshot {
            slot: slot.into(),
            supply,
            withheld,
        }
    }

    /// The make step of the escrow with a 250 bps fee on mint_a:
    /// maker sends 10_000_000, the vault is credited 9_750_000 and 250_000 is
    /// withheld in the vault.
    fn fee_make() -> (Snapshot, Snapshot) {
        let before = Snapshot {
            accounts: vec![acct("maker_ata_a", "mint_a", 100_000_000, 0), {
                let mut v = acct("vault", "mint_a", 0, 0);
                v.exists = false;
                v.lamports = 0;
                v
            }],
            mints: vec![mint("mint_a", 100_000_000, 0)],
        };
        let after = Snapshot {
            accounts: vec![
                acct("maker_ata_a", "mint_a", 90_000_000, 0),
                acct("vault", "mint_a", 9_750_000, 250_000),
            ],
            mints: vec![mint("mint_a", 100_000_000, 0)],
        };
        (before, after)
    }

    #[test]
    fn conservation_counts_withheld_fees() {
        let (before, after) = fee_make();
        let checks = conservation(&before, &after);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Held, "{}", checks[0].detail);
    }

    #[test]
    fn conservation_flags_tokens_leaving_tracked_accounts() {
        let (before, mut after) = fee_make();
        after.accounts[1].amount = 0; // drained to an untracked account
        let checks = conservation(&before, &after);
        assert_eq!(checks[0].status, CheckStatus::Violated);
        assert!(
            checks[0].detail.contains("9750000 base units moved to"),
            "{}",
            checks[0].detail
        );
    }

    #[test]
    fn expected_balances_reports_the_fee_shortfall() {
        let (before, after) = fee_make();
        let expected = vec![
            ExpectedDelta {
                label: "maker_ata_a".into(),
                delta: -10_000_000,
            },
            ExpectedDelta {
                label: "vault".into(),
                delta: 10_000_000,
            },
        ];
        let (rows, check) = expected_balances(&before, &after, &expected).unwrap();
        assert_eq!(check.status, CheckStatus::Violated);
        assert_eq!(
            check.detail,
            "vault: expected 10000000, actual 9750000 (short 250000)"
        );
        let vault = rows.iter().find(|r| r.label == "vault").unwrap();
        assert_eq!(vault.expected_after, Some(10_000_000));
        assert_eq!(vault.actual_after, 9_750_000);
        assert_eq!(vault.withheld_after, 250_000);
    }

    #[test]
    fn expected_balances_rejects_unknown_labels() {
        let (before, after) = fee_make();
        let bad = vec![ExpectedDelta {
            label: "nope".into(),
            delta: 1,
        }];
        assert!(expected_balances(&before, &after, &bad).is_err());
    }

    #[test]
    fn credits_must_match_custody() {
        let (before, after) = fee_make();
        let credit = |amount| Credit {
            label: "deposit[maker]".into(),
            amount,
            backed_by: "vault".into(),
        };
        // Protocol credits the nominal amount: mismatch and insolvency.
        let c = credited_equals_received(&[], &[credit(10_000_000)], &before, &after);
        assert_eq!(c.status, CheckStatus::Violated);
        assert!(
            c.detail.contains("credited 10000000, received 9750000"),
            "{}",
            c.detail
        );
        assert!(c.detail.contains("insolvent"), "{}", c.detail);
        // Protocol credits the received amount: holds.
        let c = credited_equals_received(&[], &[credit(9_750_000)], &before, &after);
        assert_eq!(c.status, CheckStatus::Held, "{}", c.detail);
        // No credit readers: skipped.
        let c = credited_equals_received(&[], &[], &before, &after);
        assert_eq!(c.status, CheckStatus::Skipped);
    }

    #[test]
    fn refundability_cases() {
        let ok = ExitAttempt {
            flow: "refund".into(),
            succeeded: true,
            error: None,
        };
        let failed = ExitAttempt {
            flow: "refund".into(),
            succeeded: false,
            error: Some("custom program error: 0x29".into()),
        };
        assert_eq!(
            refundability(false, &failed, 0, 0, 0).status,
            CheckStatus::Skipped
        );
        assert_eq!(
            refundability(true, &failed, 10, 0, 0).status,
            CheckStatus::Violated
        );
        assert_eq!(
            refundability(true, &ok, 10_000_000, 9_506_250, 493_750).status,
            CheckStatus::Held
        );
        assert_eq!(
            refundability(true, &ok, 10_000_000, 0, 493_750).status,
            CheckStatus::Violated
        );
    }

    #[test]
    fn closeability_requires_accounts_gone() {
        let (_, after) = fee_make();
        let c = closeability(&["vault".into()], &after);
        assert_eq!(c.status, CheckStatus::Violated);
        assert!(c.detail.contains("withheld 250000"), "{}", c.detail);

        let mut closed = after.clone();
        closed.accounts[1].exists = false;
        assert_eq!(
            closeability(&["vault".into()], &closed).status,
            CheckStatus::Held
        );
        assert_eq!(closeability(&[], &closed).status, CheckStatus::Skipped);
    }
}
