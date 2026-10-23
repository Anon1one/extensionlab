//! Regression test stub generation.
//!
//! Turns one run of a report into a Rust integration test the protocol's
//! repository can commit. The stub embeds the exact [`Case`] (slot configs
//! and scenario steps) as JSON, re-runs it through [`crate::runner::Runner`],
//! and asserts on the status.
//!
//! The generated file needs `extensionlab-core` and `serde_json` as
//! dev-dependencies, and a line that constructs the adapter. If no adapter
//! expression is given, that line is a `todo!()` naming the adapter.

use crate::{
    matrix::Case,
    report::{Report, RunStatus},
    Result,
};

/// What the generated test asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Expectation {
    /// Assert `pass`: the test fails until the incompatibility is fixed.
    #[default]
    Pass,
    /// Assert the status observed in the report: pins current behavior, for
    /// documenting a known incompatibility.
    Observed,
}

/// Options for [`render`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Rust expression of type `Box<dyn extensionlab_core::adapter::Adapter>`,
    /// e.g. `Box::new(my_escrow_tests::EscrowAdapter::new())`.
    pub adapter_expr: Option<String>,
    /// What to assert.
    pub expectation: Expectation,
}

/// Test function name for a run id: `extensionlab_` followed by the adapter
/// name and run id in lowercase ASCII alphanumerics, with every run of other
/// characters collapsed to one `_` (so the name passes the `non_snake_case`
/// lint).
pub fn test_fn_name(adapter: &str, run_id: &str) -> String {
    let mut s = String::from("extensionlab");
    for ch in format!("_{adapter}_{run_id}").chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    s.trim_end_matches('_').to_string()
}

fn raw_string(text: &str) -> String {
    let mut hashes = 1;
    while text.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}\"{text}\"{h}")
}

fn status_variant(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Pass => "Pass",
        RunStatus::Rejected => "Rejected",
        RunStatus::Fail => "Fail",
        RunStatus::Error => "Error",
        RunStatus::NotApplicable => "NotApplicable",
    }
}

fn comment_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Render the regression test for run `run_id` of `report`.
pub fn render(report: &Report, run_id: &str, opts: &Options) -> Result<String> {
    report.validate()?;
    let run = report.run(run_id)?;
    if run.status == RunStatus::Error || run.harness_error.is_some() {
        return Err(crate::Error::Adapter(format!(
            "cannot export `{run_id}`: fix its harness error and run the case again"
        )));
    }
    if run.status == RunStatus::NotApplicable {
        return Err(crate::Error::Adapter(format!(
            "cannot export `{run_id}`: its issuer action was not exercised"
        )));
    }
    let case = Case {
        id: run.id.clone(),
        slots: run.slots.clone(),
        varied_slot: run.varied_slot.clone(),
        scenario: run.scenario.clone(),
    };
    let case_json = serde_json::to_string_pretty(&case)?;
    let fn_name = test_fn_name(&report.adapter.name, &run.id);
    let allow = if opts.adapter_expr.is_none() {
        "#[allow(unreachable_code, unused_variables, unused_mut)] // remove once the adapter line is filled in\n"
    } else {
        ""
    };
    let adapter_expr = opts.adapter_expr.clone().unwrap_or_else(|| {
        let message = format!(
            "construct the `{}` adapter, e.g. Box::new(my_crate::MyAdapter::new())",
            report.adapter.name
        );
        format!("todo!(\"{{}}\", {message:?})")
    });
    let (assert_status, intent) = match opts.expectation {
        Expectation::Pass => (RunStatus::Pass, "fails until the incompatibility is fixed"),
        Expectation::Observed => (run.status, "pins the observed status"),
    };
    let mut regen_flags = String::new();
    if opts.expectation == Expectation::Observed {
        regen_flags.push_str(" --expect observed");
    }
    if let Some(expr) = &opts.adapter_expr {
        regen_flags.push_str(&format!(" --adapter-expr '{expr}'"));
    }
    let diagnosis = run
        .diagnosis
        .as_ref()
        .map(|d| {
            format!(
                "\n//! Broken assumption ({}): {}",
                comment_line(&d.rule),
                comment_line(&d.assumption)
            )
        })
        .unwrap_or_default();
    let token_hash = report
        .environment
        .programs
        .iter()
        .find(|p| p.name == "spl_token_2022")
        .and_then(|p| p.sha256.as_ref());
    let token_hash_literal = token_hash
        .map(|hash| format!("Some({hash:?})"))
        .unwrap_or_else(|| "None".into());

    let id_literal = format!("{:?}", run.id);
    Ok(format!(
        r#"//! Regression test generated by extensionlab {tool} (report schema {schema}).
//!
//! Run:      {id}
//! Adapter:  {adapter}
//! Observed: {observed}{diagnosis}
//!
//! This test {intent}.
//! Regenerate with: extensionlab regress <report.json> --run '{id}'{regen_flags}
//!
//! Needs dev-dependencies: extensionlab-core, serde_json.

use extensionlab_core::{{adapter::Adapter, matrix::Case, report::RunStatus, runner::{{Runner, RunnerOptions}}}};

const CASE_JSON: &str = {case_lit};
const RUN_ID: &str = {id_literal};
const TOKEN_2022_SHA256: Option<&str> = {token_hash_literal};

#[test]
{allow}fn {fn_name}() {{
    let case: Case = serde_json::from_str(CASE_JSON).expect("embedded case is valid");
    let mut adapter: Box<dyn Adapter> = {adapter_expr};
    let mut runner = Runner::new(RunnerOptions {{ seed: {seed}, ..Default::default() }});
    if let Some(path) = std::env::var_os("EXTENSIONLAB_TOKEN_2022_SO") {{
        runner = runner.with_token_2022_binary(path, TOKEN_2022_SHA256).expect("pinned Token-2022 ELF");
    }}
    if let Some(expected) = TOKEN_2022_SHA256 {{
        let programs = runner.runtime_programs(adapter.as_ref()).expect("load runtime programs");
        let actual = programs.iter().find(|p| p.name == "spl_token_2022").and_then(|p| p.sha256.as_deref());
        assert_eq!(actual, Some(expected), "Token-2022 runtime changed; set EXTENSIONLAB_TOKEN_2022_SO to the recorded ELF");
    }}
    let result = runner
        .run_case(adapter.as_mut(), &case)
        .expect("run_case");
    assert!(
        result.harness_error.is_none(),
        "{{}}: harness error: {{}}",
        RUN_ID,
        result.harness_error.as_deref().unwrap_or_default()
    );
    assert_eq!(
        result.status,
        RunStatus::{status},
        "{{}}: invariants {{:#?}}",
        RUN_ID,
        result.invariants
    );
}}
"#,
        tool = comment_line(&report.tool.version),
        schema = comment_line(&report.schema_version),
        id = comment_line(&run.id),
        id_literal = id_literal,
        adapter = comment_line(&report.adapter.name),
        observed = status_variant(run.status).to_lowercase(),
        diagnosis = diagnosis,
        intent = intent,
        case_lit = raw_string(&case_json),
        fn_name = fn_name,
        allow = allow,
        adapter_expr = adapter_expr,
        status = status_variant(assert_status),
        token_hash_literal = token_hash_literal,
        seed = report.environment.seed,
        regen_flags = comment_line(&regen_flags),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_regressions_keep_the_seed_hash_and_escape_run_ids() {
        let mut report =
            Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        report.environment.seed = 123;
        report
            .environment
            .programs
            .iter_mut()
            .find(|p| p.name == "spl_token_2022")
            .unwrap()
            .sha256 = Some("a".repeat(64));
        report.runs[0].id = "quote\"{braces}\nsecond line".into();
        let src = render(&report, &report.runs[0].id, &Options::default()).unwrap();
        assert!(src.contains("seed: 123"));
        assert!(src.contains(&format!("Some(\"{}\")", "a".repeat(64))));
        assert!(src.contains("const RUN_ID: &str = \"quote\\\"{braces}\\nsecond line\";"));
        assert!(!src.contains("//! Run:      quote\"{braces}\nsecond line"));
    }

    #[test]
    fn names_are_valid_identifiers() {
        assert_eq!(
            test_fn_name("escrow", "transfer_fee_200bps@mint_a/make_take"),
            "extensionlab_escrow_transfer_fee_200bps_mint_a_make_take"
        );
    }

    #[test]
    fn raw_string_picks_enough_hashes() {
        assert_eq!(raw_string("a"), "r#\"a\"#");
        assert_eq!(raw_string("\"#x"), "r##\"\"#x\"##");
    }

    #[test]
    fn stub_embeds_a_parseable_case() {
        let report =
            Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        let id = "transfer_fee_200bps@mint_a/make_take";
        let src = render(&report, id, &Options::default()).unwrap();
        assert!(src.contains("fn extensionlab_escrow_transfer_fee_200bps_mint_a_make_take()"));
        assert!(src.contains("#[allow(unreachable_code"));
        assert!(src.contains("RunStatus::Pass"));
        assert!(src.contains("todo!(\"{}\", \"construct the `escrow` adapter"));

        // Pull the embedded JSON back out and parse it as a Case.
        let start = src.find("r#\"").unwrap() + 3;
        let end = src[start..].find("\"#;").unwrap() + start;
        let case: Case = serde_json::from_str(&src[start..end]).unwrap();
        assert_eq!(case.id, id);
        assert_eq!(case.slots["mint_a"].label, "transfer_fee_200bps");

        let pinned = render(
            &report,
            id,
            &Options {
                adapter_expr: Some("Box::new(escrow_adapter::EscrowAdapter::default())".into()),
                expectation: Expectation::Observed,
            },
        )
        .unwrap();
        assert!(pinned.contains("RunStatus::Fail"));
        assert!(pinned.contains("Box::new(escrow_adapter::EscrowAdapter::default())"));
    }

    #[test]
    fn unknown_run_is_an_error() {
        let report =
            Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        assert!(render(&report, "nope", &Options::default()).is_err());
    }

    #[test]
    fn harness_errors_and_unexercised_cases_are_not_exported_as_regressions() {
        let mut report =
            Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        let id = report.runs[0].id.clone();
        for status in [RunStatus::Error, RunStatus::NotApplicable] {
            report.runs[0].status = status;
            if status == RunStatus::NotApplicable {
                let step = crate::matrix::Step::Issuer {
                    action: crate::matrix::IssuerAction::DrainCustody,
                };
                let index = report.runs[0].steps.len();
                report.runs[0].scenario.steps.push(step.clone());
                report.runs[0].steps.push(crate::report::StepRecord {
                    index,
                    step,
                    outcome: crate::report::StepOutcome::NotApplicable,
                    transactions: Vec::new(),
                });
            }
            report.summary = crate::report::Summary::of(&report.runs);
            assert!(report.validate().is_ok());
            for expectation in [Expectation::Pass, Expectation::Observed] {
                assert!(render(
                    &report,
                    &id,
                    &Options {
                        expectation,
                        ..Default::default()
                    }
                )
                .is_err());
            }
        }
    }

    #[test]
    fn adapter_names_are_rust_string_data_in_the_unfinished_constructor() {
        let mut report =
            Report::from_json(include_str!("../tests/fixtures/example_report.json")).unwrap();
        report.adapter.name = "quoted\"{adapter}\nname".into();
        let source = render(&report, &report.runs[0].id, &Options::default()).unwrap();
        assert!(source.contains(
            r#"todo!("{}", "construct the `quoted\"{adapter}\nname` adapter, e.g. Box::new(my_crate::MyAdapter::new())")"#
        ));
    }
}
