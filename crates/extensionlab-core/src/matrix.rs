//! Scenario matrix: adapter x mint configs x scenarios.
//!
//! A [`Matrix`] is a named list of [`MintConfig`]s and a [`SlotStrategy`]
//! saying how to assign them to the adapter's mint slots. [`expand`] turns a
//! matrix plus the adapter's slots and scenarios into concrete [`Case`]s, each
//! of which the runner executes in a fresh LiteSVM instance.

use crate::{
    adapter::{FlowKind, FlowSpec},
    mint::{AccountExtension, MintConfig, MintExtension, TokenProgram},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::{BTreeMap, BTreeSet};

/// How matrix configs are assigned to mint slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SlotStrategy {
    /// Vary one slot at a time. The other slots get the extension-free config
    /// of the same token program, so protocols that require every mint to use
    /// one token program (as the reference escrow does) still type-check.
    #[default]
    OneSlotAtATime,
    /// Give every slot the same config.
    AllSlots,
}

/// An action the token issuer takes between protocol flows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum IssuerAction {
    /// Use the permanent delegate to move the whole balance of every custody
    /// account of every slot whose mint has `PermanentDelegate` into an
    /// issuer-owned account.
    DrainCustody,
    /// Freeze every custody account of `slot` (any mint with a freeze
    /// authority, which every harness mint has).
    FreezeCustody {
        /// Mint slot.
        slot: String,
    },
    /// Harvest withheld transfer fees from every tracked account of `slot` to
    /// the mint. Token-2022 allows anyone to do this.
    HarvestWithheld {
        /// Mint slot.
        slot: String,
    },
    /// Schedule a new transfer fee on `slot`'s mint. Token-2022 applies it two
    /// epochs later (see [`Step::AdvanceEpoch`]); until then the old fee
    /// applies. Does nothing when the mint has no transfer fee.
    SetTransferFee {
        /// Mint slot.
        slot: String,
        /// New fee rate in basis points (0..=10_000).
        basis_points: u16,
        /// New fee cap in base units. `None` keeps the cap in effect now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        maximum_fee: Option<u64>,
    },
    /// Pause `slot`'s mint: transfers, mints and burns fail until
    /// [`IssuerAction::Resume`]. Does nothing when the mint is not
    /// `Pausable`.
    Pause {
        /// Mint slot.
        slot: String,
    },
    /// Resume a paused mint. Does nothing when the mint is not `Pausable`.
    Resume {
        /// Mint slot.
        slot: String,
    },
    /// Pause the mint of every slot whose config has `Pausable`. Mints
    /// without it are left alone; when no slot has it, nothing is sent.
    PauseAll,
}

/// One step of a scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Step {
    /// Run an adapter flow.
    Flow {
        /// Flow name.
        name: String,
    },
    /// Run an issuer action.
    Issuer {
        /// The action.
        #[serde(flatten)]
        action: IssuerAction,
    },
    /// Move the clock's `unix_timestamp` forward (for expiries).
    AdvanceClock {
        /// Seconds to add.
        seconds: i64,
    },
    /// Move the clock's `epoch` forward (for scheduled transfer fees). Only
    /// the `Clock` sysvar changes; the slot and the timestamp stay.
    AdvanceEpoch {
        /// Epochs to add.
        epochs: u64,
    },
}

impl Step {
    /// Flow step helper.
    pub fn flow(name: impl Into<String>) -> Self {
        Step::Flow { name: name.into() }
    }

    /// Short human-readable form, e.g. `flow:make` or `issuer:drain_custody`.
    pub fn describe(&self) -> String {
        match self {
            Step::Flow { name } => format!("flow:{name}"),
            Step::Issuer { action } => match action {
                IssuerAction::DrainCustody => "issuer:drain_custody".into(),
                IssuerAction::FreezeCustody { slot } => format!("issuer:freeze_custody({slot})"),
                IssuerAction::HarvestWithheld { slot } => {
                    format!("issuer:harvest_withheld({slot})")
                }
                IssuerAction::SetTransferFee {
                    slot,
                    basis_points,
                    maximum_fee,
                } => match maximum_fee {
                    Some(max) => {
                        format!("issuer:set_transfer_fee({slot}, {basis_points}bps, max {max})")
                    }
                    None => format!("issuer:set_transfer_fee({slot}, {basis_points}bps)"),
                },
                IssuerAction::Pause { slot } => format!("issuer:pause({slot})"),
                IssuerAction::Resume { slot } => format!("issuer:resume({slot})"),
                IssuerAction::PauseAll => "issuer:pause_all".into(),
            },
            Step::AdvanceClock { seconds } => format!("clock:+{seconds}s"),
            Step::AdvanceEpoch { epochs } => format!("clock:+{epochs}epoch"),
        }
    }
}

/// A sequence of steps run in one fresh VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    /// Name used in run ids, e.g. `make_refund`.
    pub name: String,
    /// Steps in order.
    pub steps: Vec<Step>,
    /// Only run this scenario in cases where at least one slot's config has
    /// this extension kind (see `MintExtension::kind`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_extension: Option<String>,
}

/// Scenarios derived from an adapter's flows:
///
/// - `{entry}_{exit}` for every entry flow and every exit flow;
/// - `{entry}_drain_{exit}`: the same with a permanent-delegate drain of the
///   custody accounts in between, only for cases with `permanent_delegate`;
/// - `{entry}_pause_{exit}`: the same with [`IssuerAction::PauseAll`] in
///   between, only for cases with `pausable`.
///
/// An adapter with entry flows but no exit flows gets one scenario per entry
/// flow.
pub fn default_scenarios(flows: &[FlowSpec]) -> Vec<Scenario> {
    let entries: Vec<&FlowSpec> = flows.iter().filter(|f| f.kind == FlowKind::Entry).collect();
    let exits: Vec<&FlowSpec> = flows.iter().filter(|f| f.kind == FlowKind::Exit).collect();
    let mut out = Vec::new();
    for e in &entries {
        if exits.is_empty() {
            out.push(Scenario {
                name: e.name.clone(),
                steps: vec![Step::flow(&e.name)],
                requires_extension: None,
            });
        }
        for x in &exits {
            out.push(Scenario {
                name: format!("{}_{}", e.name, x.name),
                steps: vec![Step::flow(&e.name), Step::flow(&x.name)],
                requires_extension: None,
            });
        }
    }
    let middles = [
        ("drain", IssuerAction::DrainCustody, "permanent_delegate"),
        ("pause", IssuerAction::PauseAll, "pausable"),
    ];
    for (tag, action, kind) in middles {
        for e in &entries {
            for x in &exits {
                out.push(Scenario {
                    name: format!("{}_{tag}_{}", e.name, x.name),
                    steps: vec![
                        Step::flow(&e.name),
                        Step::Issuer {
                            action: action.clone(),
                        },
                        Step::flow(&x.name),
                    ],
                    requires_extension: Some(kind.into()),
                });
            }
        }
    }
    out
}

/// A named set of configs and a slot strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Matrix {
    /// Name, e.g. `default`.
    pub name: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Slot strategy.
    #[serde(default)]
    pub strategy: SlotStrategy,
    /// Configs to run.
    pub configs: Vec<MintConfig>,
}

/// One concrete run: a slot assignment and a scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    /// Run id: `{config}@{slot}/{scenario}` when one slot is varied,
    /// `{config}/{scenario}` otherwise.
    pub id: String,
    /// Config for every slot.
    pub slots: BTreeMap<String, MintConfig>,
    /// The slot this case varies, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub varied_slot: Option<String>,
    /// Scenario to run.
    pub scenario: Scenario,
}

impl Case {
    /// True when every slot is an extension-free SPL Token mint: the baseline
    /// the protocol must pass before any other result means anything. Plain
    /// Token-2022 is not part of it, because failing there is a Token-2022
    /// finding, not a broken setup.
    pub fn is_baseline(&self) -> bool {
        self.slots
            .values()
            .all(|c| c.program == TokenProgram::SplToken && c.is_neutral())
    }
}

/// Expand a matrix into cases.
///
/// Every config is validated first. Scenarios with `requires_extension` are
/// kept only for cases where some slot has that extension. Duplicate run ids
/// are an error.
pub fn expand(matrix: &Matrix, slots: &[String], scenarios: &[Scenario]) -> Result<Vec<Case>> {
    if slots.is_empty() {
        return Err(Error::Adapter("adapter declares no mint slots".into()));
    }
    let mut declared = BTreeSet::new();
    for slot in slots {
        if slot.is_empty() || !declared.insert(slot) {
            return Err(Error::Adapter(
                "mint slot names must be non-empty and unique".into(),
            ));
        }
    }
    let mut cases = Vec::new();
    let mut ids = BTreeSet::new();
    for config in &matrix.configs {
        config.validate()?;
        let all_same = || {
            slots
                .iter()
                .map(|s| (s.clone(), config.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let assignments: Vec<(Option<String>, BTreeMap<String, MintConfig>)> = match matrix.strategy
        {
            SlotStrategy::AllSlots => vec![(None, all_same())],
            SlotStrategy::OneSlotAtATime if config.is_neutral() => vec![(None, all_same())],
            SlotStrategy::OneSlotAtATime => slots
                .iter()
                .map(|varied| {
                    let neutral = MintConfig::neutral(config.program, config.decimals);
                    let map = slots
                        .iter()
                        .map(|s| {
                            let c = if s == varied {
                                config.clone()
                            } else {
                                neutral.clone()
                            };
                            (s.clone(), c)
                        })
                        .collect();
                    (Some(varied.clone()), map)
                })
                .collect(),
        };
        for (varied, map) in assignments {
            for scenario in scenarios {
                if let Some(kind) = &scenario.requires_extension {
                    if !map.values().any(|c| c.has(kind)) {
                        continue;
                    }
                }
                let id = match &varied {
                    Some(slot) => format!("{}@{}/{}", config.label, slot, scenario.name),
                    None => format!("{}/{}", config.label, scenario.name),
                };
                if !ids.insert(id.clone()) {
                    return Err(Error::Adapter(format!("duplicate run id `{id}`")));
                }
                cases.push(Case {
                    id,
                    slots: map.clone(),
                    varied_slot: varied.clone(),
                    scenario: scenario.clone(),
                });
            }
        }
    }
    Ok(cases)
}

/// Program id recorded on the `transfer_hook_undeployed` preset mint. Nothing
/// is deployed at this address, so every transfer of that mint is expected to
/// fail. It is the 32 ASCII bytes `extensionlab-undeployed-hook-id!`.
pub fn undeployed_hook_program_id() -> Pubkey {
    Pubkey::new_from_array(*b"extensionlab-undeployed-hook-id!")
}

/// Names of the built-in matrices.
pub const PRESET_NAMES: &[&str] = &["baseline", "default", "all"];

/// Built-in matrix by name.
///
/// - `baseline`: SPL Token and plain Token-2022. Only the SPL Token cases
///   count as baseline for [`Case::is_baseline`] and the exit code.
/// - `default`: baseline plus TransferFee (200 bps uncapped, and 0 bps),
///   DefaultAccountState(Frozen), PermanentDelegate, MemoTransfer on actor
///   accounts, and NonTransferable.
/// - `all`: `default` plus a TransferHook mint whose hook program is not
///   deployed, a Pausable mint, and CpiGuard on actor accounts. A meaningful
///   TransferHook test needs a deployed hook program; see the open questions
///   in `research/04_architecture.md`. The Pausable config also gets the
///   `{entry}_pause_{exit}` scenarios of [`default_scenarios`].
///
/// All presets use 6 decimals and [`SlotStrategy::OneSlotAtATime`].
pub fn preset(name: &str) -> Option<Matrix> {
    const D: u8 = 6;
    let baseline = vec![
        MintConfig::spl_token("spl_token", D),
        MintConfig::token_2022("token_2022", D),
    ];
    let extended = || {
        vec![
            // 200 bps with no cap, the parameters of the escrow reproduction in
            // research/01_repro_escrow_fee_token.md.
            MintConfig::token_2022("transfer_fee_200bps", D).with(MintExtension::TransferFee {
                basis_points: 200,
                maximum_fee: u64::MAX,
            }),
            MintConfig::token_2022("transfer_fee_0bps", D).with(MintExtension::TransferFee {
                basis_points: 0,
                maximum_fee: 0,
            }),
            MintConfig::token_2022("default_frozen", D)
                .with(MintExtension::DefaultAccountStateFrozen),
            MintConfig::token_2022("permanent_delegate", D).with(MintExtension::PermanentDelegate),
            MintConfig::token_2022("memo_required", D)
                .with_actor_account(AccountExtension::MemoTransferRequired),
            MintConfig::token_2022("non_transferable", D).with(MintExtension::NonTransferable),
        ]
    };
    let m = |name: &str, description: &str, configs: Vec<MintConfig>| Matrix {
        name: name.into(),
        description: description.into(),
        strategy: SlotStrategy::OneSlotAtATime,
        configs,
    };
    match name {
        "baseline" => Some(m(
            "baseline",
            "SPL Token and Token-2022 without extensions",
            baseline,
        )),
        "default" => Some(m(
            "default",
            "Baseline plus one Token-2022 extension per config",
            baseline.into_iter().chain(extended()).collect(),
        )),
        "all" => Some(m(
            "all",
            "Default plus a TransferHook mint with an undeployed hook program, Pausable and CpiGuard",
            baseline
                .into_iter()
                .chain(extended())
                .chain([
                    MintConfig::token_2022("transfer_hook_undeployed", D).with(
                        MintExtension::TransferHook {
                            program_id: undeployed_hook_program_id(),
                        },
                    ),
                    MintConfig::token_2022("pausable", D).with(MintExtension::Pausable),
                    MintConfig::token_2022("cpi_guard", D)
                        .with_actor_account(AccountExtension::CpiGuard),
                ])
                .collect(),
        )),
        _ => None,
    }
}

/// Resolve `--matrix`: a preset name, or a path to a JSON file holding a
/// [`Matrix`].
pub fn load(name_or_path: &str) -> Result<Matrix> {
    if let Some(m) = preset(name_or_path) {
        return Ok(m);
    }
    let path = std::path::Path::new(name_or_path);
    if path.is_file() {
        let text = std::fs::read_to_string(path)?;
        return Ok(serde_json::from_str(&text)?);
    }
    Err(Error::UnknownMatrix(name_or_path.to_string()))
}

/// Token programs used by a matrix, for reporting.
pub fn programs_in(matrix: &Matrix) -> BTreeSet<TokenProgram> {
    matrix.configs.iter().map(|c| c.program).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escrow_flows() -> Vec<FlowSpec> {
        let f = |name: &str, kind| FlowSpec {
            name: name.into(),
            kind,
            description: String::new(),
        };
        vec![
            f("make", FlowKind::Entry),
            f("take", FlowKind::Exit),
            f("refund", FlowKind::Exit),
            f("update", FlowKind::Other),
        ]
    }

    #[test]
    fn default_scenarios_pair_entries_with_exits() {
        let names: Vec<String> = default_scenarios(&escrow_flows())
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "make_take",
                "make_refund",
                "make_drain_take",
                "make_drain_refund",
                "make_pause_take",
                "make_pause_refund"
            ]
        );
    }

    #[test]
    fn pause_scenarios_run_only_for_pausable_cases() {
        let slots = vec!["mint_a".to_string(), "mint_b".to_string()];
        let scenarios = default_scenarios(&escrow_flows());
        let cases = expand(&preset("all").unwrap(), &slots, &scenarios).unwrap();

        // `default` (32 cases)
        // + transfer_hook_undeployed, pausable, cpi_guard x 2 slots x 2 plain scenarios
        // + pausable x 2 slots x 2 pause scenarios
        assert_eq!(cases.len(), 32 + 3 * 2 * 2 + 2 * 2);

        let paused: Vec<&str> = cases
            .iter()
            .filter(|c| c.scenario.name.contains("_pause_"))
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(
            paused,
            vec![
                "pausable@mint_a/make_pause_take",
                "pausable@mint_a/make_pause_refund",
                "pausable@mint_b/make_pause_take",
                "pausable@mint_b/make_pause_refund",
            ]
        );
        let c = cases
            .iter()
            .find(|c| c.id == "pausable@mint_b/make_pause_take")
            .unwrap();
        assert_eq!(
            c.scenario
                .steps
                .iter()
                .map(Step::describe)
                .collect::<Vec<_>>(),
            ["flow:make", "issuer:pause_all", "flow:take"]
        );
        assert!(!c.slots["mint_a"].has("pausable"));
        assert!(cases.iter().any(|c| c.id == "cpi_guard@mint_a/make_take"));
    }

    #[test]
    fn all_presets_validate() {
        for name in PRESET_NAMES {
            let m = preset(name).unwrap();
            for c in &m.configs {
                c.validate().unwrap_or_else(|e| panic!("{name}: {e}"));
            }
        }
    }

    #[test]
    fn empty_or_duplicate_mint_slot_declarations_are_rejected() {
        for strategy in [SlotStrategy::OneSlotAtATime, SlotStrategy::AllSlots] {
            let mut matrix = preset("baseline").unwrap();
            matrix.strategy = strategy;
            for slots in [vec![String::new()], vec!["asset".into(), "asset".into()]] {
                assert!(expand(&matrix, &slots, &default_scenarios(&escrow_flows())).is_err());
            }
        }
    }

    #[test]
    fn one_slot_at_a_time_fills_other_slots_with_the_same_program() {
        let slots = vec!["mint_a".to_string(), "mint_b".to_string()];
        let scenarios = default_scenarios(&escrow_flows());
        let cases = expand(&preset("default").unwrap(), &slots, &scenarios).unwrap();

        // 2 neutral configs x 2 plain scenarios
        // + 6 extension configs x 2 slots x 2 plain scenarios
        // + permanent_delegate x 2 slots x 2 drain scenarios
        assert_eq!(cases.len(), 2 * 2 + 6 * 2 * 2 + 2 * 2);

        let c = cases
            .iter()
            .find(|c| c.id == "transfer_fee_200bps@mint_a/make_refund")
            .unwrap();
        assert_eq!(c.slots["mint_b"].label, "token_2022");
        assert_eq!(c.slots["mint_b"].program, TokenProgram::Token2022);
        assert!(!c.is_baseline());

        let b = cases
            .iter()
            .find(|c| c.id == "spl_token/make_take")
            .unwrap();
        assert!(b.is_baseline());
        assert!(b.varied_slot.is_none());

        // Plain Token-2022 is extension-free but not baseline.
        let t = cases
            .iter()
            .find(|c| c.id == "token_2022/make_take")
            .unwrap();
        assert!(t.slots.values().all(MintConfig::is_neutral));
        assert!(!t.is_baseline());

        assert!(cases
            .iter()
            .any(|c| c.id == "permanent_delegate@mint_a/make_drain_refund"));
        assert!(!cases
            .iter()
            .any(|c| c.id.starts_with("transfer_fee_200bps") && c.id.contains("drain")));
    }

    #[test]
    fn matrix_round_trips_through_json() {
        let m = preset("all").unwrap();
        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: Matrix = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn scenario_steps_round_trip_through_json() {
        let scenarios = default_scenarios(&escrow_flows());
        let json = serde_json::to_string(&scenarios).unwrap();
        assert!(
            json.contains(r#"{"type":"issuer","action":"drain_custody"}"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"type":"issuer","action":"pause_all"}"#),
            "{json}"
        );
        let back: Vec<Scenario> = serde_json::from_str(&json).unwrap();
        assert_eq!(scenarios, back);

        let freeze = Step::Issuer {
            action: IssuerAction::FreezeCustody {
                slot: "mint_a".into(),
            },
        };
        let json = serde_json::to_string(&freeze).unwrap();
        assert_eq!(
            json,
            r#"{"type":"issuer","action":"freeze_custody","slot":"mint_a"}"#
        );
        assert_eq!(serde_json::from_str::<Step>(&json).unwrap(), freeze);
    }

    #[test]
    fn undeployed_hook_id_is_stable() {
        assert_eq!(
            undeployed_hook_program_id().to_bytes(),
            *b"extensionlab-undeployed-hook-id!"
        );
    }
}
