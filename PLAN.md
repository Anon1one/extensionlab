# ExtensionLab plan

Written 2026-10-21. It draws on `research/01` to `research/04` and on test runs repeated on
2026-10-21. Anything not run or read first-hand is marked **unverified**, **predicted** or
**unconfirmed**.

## 1. Verdict: does the fee-token escrow reproduction give ExtensionLab a real hook?

**Yes, as a demo. Not yet as proof of a product.**

What was re-run on 2026-10-21:

- `cd targets/escrow && cargo test` passed all **20 tests**: 1 unit test, the 3 original SPL Token
  tests and the 16 Token-2022 tests in `programs/escrowq32026/tests/token2022_fees.rs`.
- Five of those tests (`repro_*`) assert a failure. Each asserts an instruction index, an error code
  and a log line:
  - **F1:** `refund` fails for every Token-2022 mint with Anchor `ConstraintAssociated` (2009). The
    cause is in `refund.rs`: `maker_ata_a` and `vault` omit `associated_token::token_program`. After
    expiry, `take` and `update` both return `EscrowExpired` (6000), so the deposit is locked for good.
  - **F2:** `take` with a fee-bearing `mint_a` fails at `CloseAccount` with Token-2022 error 35
    (`AccountHasWithheldTransferFees`, 0x23). The fee withheld during `make` stays in the vault.
- The other tests assert exact amounts:
  - With a 200 bps fee, the vault holds 9,800,000 of a 10,000,000 deposit, with 200,000 withheld.
  - After a harvest, the taker receives 9,604,000.
  - An issuer fee change after `make` cuts the maker's net to 6,650,000.
- `cargo check` and `cargo test --workspace` at the repository root pass, with 37 tests. Both are
  host-only.

Not re-run in this pass: the patched copy in `research/fixcheck/`. Its saved outputs
(`run_fix_A_only.txt`, `run_fix_A_B.txt`) show that fix A turns 3 `repro_` tests red and fixes A+B
turn 5 red, which is the intended regression-test behavior.

Why this is a hook:

1. It is a concrete, reproducible failure. The program's own tests pass, and its README says it is
   "not tied to one token program".
2. It tells a strong story. Both exits break at once for Token-2022, and an expired escrow cannot be
   recovered by any instruction.
3. The full output ExtensionLab promises was produced by hand once: failing transaction and logs,
   expected vs actual balances including withheld fees, the broken assumption, and tests that flip
   when the fix lands.

Why it is not yet enough:

1. **ExtensionLab did not find these bugs. A hand-written test file did.** That file has
   1,062 lines. The product claim holds only if an adapter much smaller than that file reproduces
   F1 and F2 automatically.
   - Update (2026-10-22): it does. The runner is implemented, and the escrow adapter (341 non-blank, non-comment lines
     across both variants) reproduces F1 (2009) and F2 (0x23) with the right diagnoses; see
     `adapters/escrow/tests/golden.rs`. The adapter was written by someone who already knew the
     escrow, so this is not yet the third-party adapter-cost test (task 7).
2. **The target is the author's own learning project.** Its `Anchor.toml` configures localnet only.
   Nobody else's funds are involved, and outside readers may discount it.
3. **F1 and F2 are not new knowledge.**
   - F1 is a missing Anchor constraint, a program-id problem rather than an extension behavior.
   - F2 is documented Token-2022 behavior:
     https://www.solana-program.com/docs/token-2022/extensions#transfer-fees and
     https://neodyme.io/en/blog/token-2022 (secondary).
   - So the value is in automation and packaging, not in discovery.
4. **All results come from one Token-2022 build.** That is the 10.0.0 binary bundled in
   LiteSVM 0.10.0. Whether mainnet behaves the same is **unverified**.

The verdict becomes a product hook when all three of these are true:

- (a) the runner plus an escrow adapter of roughly 200 lines or fewer reproduces F1 and F2;
- (b) the same matrix confirms at least one **predicted** break in a third-party repository (first
  candidate: the `solana-foundation/program-examples` escrow);
- (c) a generated regression test runs in CI.

## 2. MVP definition

The MVP is done when all of the following hold:

1. `extensionlab run --adapter escrow --matrix default` runs on LiteSVM against
   `targets/escrow` and writes `report.json` and `report.md`. They contain the F1 and F2 runs with:
   - the failing transaction, its error and logs;
   - per-account balances (amount and withheld), expected vs actual;
   - the violated invariants and the diagnosis rule.
2. A golden test asserts the statuses and diagnoses of those runs, plus the vault balance
   (9,800,000 + 200,000 withheld).
3. `extensionlab regress` produces a test that compiles in the target repository and is red until
   fix A (for F1) or fix B (for F2) is applied. This is checked once against `research/fixcheck`.
4. One third-party program is run through its own adapter, with no change to its program code. The
   result is published as a case study, whatever it is (pass, fail or rejected).
5. The repository has a license, a CI workflow running `cargo test --workspace`, and no absolute
   local paths in docs.

Out of the MVP:

- transfer-hook programs;
- confidential transfers;
- devnet or mainnet runs;
- the composite GitHub Action;
- a crates.io release.

## 3. First scenario set (prioritized)

IDs come from `research/02_extension_matrix.md`, except T22-BASE, which is added here because
F1 is the strongest finding so far. "Factory" means support in `crates/extensionlab-core/src/mint.rs`
today. The factory currently has these configurations:

- SPL Token;
- plain Token-2022;
- TransferFee;
- DefaultAccountState(Frozen);
- PermanentDelegate;
- TransferHook (program id only);
- NonTransferable;
- MemoTransfer on actor accounts.

| # | ID | Scenario | Factory | Evidence against a real protocol |
|---|---|---|---|---|
| 1 | T22-BASE | Plain Token-2022 mint through every flow (program-id and ATA-derivation assumptions) | ready | **Observed** (escrow F1) |
| 2 | TF-02 | Empty a custody account that holds withheld fees, then close it | ready | **Observed** (escrow F2) |
| 3 | TF-01 | Amount credited or recorded vs amount received | ready | **Observed** (escrow: vault 9,800,000; taker 9,604,000) |
| 4 | TF-03 | Issuer changes the fee between entry and exit, then 2-epoch warp | needs a `set_transfer_fee` plus epoch-warp step | **Observed** in a hand-written escrow test (maker net 6,650,000) |
| 5 | PD-01 | Permanent delegate drains custody after deposit | ready (`drain_custody` step is defined, not executed) | Top-level only (research/02 probe); against a protocol **unverified** |
| 6 | DAS-01 | Frozen-by-default mint; the protocol creates the vault and deposits | ready | **Predicted** `AccountFrozen` (0x11) at escrow `make` |
| 7 | DAS-02 | Issuer freezes custody after deposit | ready (`freeze_custody` step is defined) | **Predicted** |
| 8 | CPIG-01 | User's source account has CPI Guard; protocol does a CPI deposit signed by the user | ready (matrix `all`) | **Observed** (22 Oct, golden test): `make` fails with 0x2a when the maker's account has CPI Guard, because `make` CPIs `transfer_checked` with the maker as authority; with the guard on the taker's account, `take` fails the same way after the deposit |
| 9 | MEMO-01 | Recipient enables required memos; protocol pays out | ready | **Predicted**. On the escrow, F1 masks the refund path until fix A, so run it on the take payout and on the fix-A copy |
| 10 | PAUSE-01 | Issuer pauses during the exit or refund window | ready (matrix `all`, `{entry}_pause_{exit}`) | **Observed** (22 Oct, golden test): Token-2022 10.0.0 supports Pausable; `take` after a pause fails with 0x43, `refund` after a pause fails on F1 first |
| 11 | HOOK-02 | Hook program `None` at deposit, then switched on before withdraw | needs a hook program | Top-level only (research/02 probe, LiteSVM 0.16 / Token-2022 11.0.0) |
| 12 | HOOK-01, HOOK-03 | Allow-all hook with no extra accounts forwarded; deny hook | needs hook programs | **Unverified** |

Low-cost additions to run alongside the list above:

- **NT-01** (non-transferable mint should be rejected at entry). The factory is ready.
- **F4** (mixed SPL Token and Token-2022 mints). Observed on the escrow; the matrix does not
  support it yet.
- **UNK-01** (a mint with an extension tag newer than the protocol's interface crate). Evidence is
  Anchor issue https://github.com/otter-sec/anchor/issues/5138. It needs a Token-2022 11.x binary,
  because 10.0.0 has no PermissionedBurn.

These are P1/P2 per research/02: MCA-01, IO-01, UI-01, CT-01, MD-01, GRP-01 and PB-01.

**Scenario expectations must be invariants, not error codes.** The research/02 table states expected
failures for a pooled vault. For example, TF-01 expects "last withdrawal fails with
`InsufficientFunds`". The escrow instead moves `vault.amount` and silently under-pays. The expected
outcome depends on the protocol, so the catalog should state the invariant that breaks, not the
error code.

## 4. Differentiation

All competitor facts below come from `research/03` (read 2026-10-21) and were not re-checked here.

| Tool | What it covers | What it lacks for this job |
|---|---|---|
| Trident (Ackee) | Fuzz API that can create mints with 15 mint and 3 account extensions | No preset catalog of hostile configurations, no diagnosis, no balance-vs-assumption report. Token-2022 ATA creation is broken ([#385](https://github.com/Ackee-Blockchain/trident/issues/385), open) |
| LiteSVM | Execution primitives; `litesvm-token` has a `token-2022` feature | No scenarios. Its helper under-allocated Token-2022 accounts until 2026-10-20 ([#440](https://github.com/LiteSVM/litesvm/issues/440)) |
| Surfpool | Mainnet forking and cheatcodes | Cheatcode ATAs lack required extensions ([#866](https://github.com/solana-foundation/surfpool/issues/866), open); no catalog |
| Mollusk, Bankrun, Crucible, Sec3 X-ray | Single-instruction harness; deprecated Node harness; generic fuzzing; static rules | None targets Token-2022 integration flows |
| GoPlus, RugCheck and other scanners | Score a **mint** for traders | Do not test whether **your program** survives that mint |
| In-house tooling (marginfi `SupportedExtension` helper, Kamino conditional allow-list, Raydium/Orca/Meteora allow-lists plus registries) | Large teams build their own | Small and mid-size teams have none of it |

ExtensionLab's position:

- a curated, versioned scenario catalog;
- runs the developer's real multi-step flows;
- reports the failing transaction, balances, invariant and assumption;
- emits a CI regression test.

It is built on LiteSVM, and every account is created through the real Token-2022 and ATA programs,
which avoids the LiteSVM #440 and Surfpool #866 class of false results. Treat Trident as a possible
export target, not a rival.

Main competitive risk: Trident or LiteSVM adding presets. Research/03 found no announced plans
(**unverified**).

## 5. Top 5 target repositories to test first

Every predicted break below comes from research/03 code reading and is **predicted**, not run.
Commits are the ones research/03 read. Licenses and build requirements (Anchor version,
platform-tools) are **unverified** for all five.

Before these, the golden target is `targets/escrow`. The runner must reproduce F1 and F2 there
first.

| # | Repository (commit) | Flow | Scenario expected to hit | Why it is in the top 5 |
|---|---|---|---|---|
| 1 | [solana-foundation/program-examples](https://github.com/solana-foundation/program-examples) `tokens/escrow/anchor` (`de09019`) | `make_offer` → `take_offer` / `refund_offer` | TF-02 (both exits revert at `close_account`), TF-01 | Canonical and widely copied. Same shape as the golden target. Best public demo |
| 2 | [sablier-labs/solsab](https://github.com/sablier-labs/solsab) (`e060b26`) | create → withdraw → cancel | TF-01 (stores `deposit_amount` from arguments), PD-01, PAUSE-01 | Custody over time; the shortfall lands on the last withdraw or cancel |
| 3 | [1inch/solana-fusion-protocol](https://github.com/1inch/solana-fusion-protocol) (`2aab60c`) | create → fill → cancel | TF-02 on the final-fill close, TF-01 | Known team; escrow-shaped |
| 4 | [solana-foundation/vault](https://github.com/solana-foundation/vault) `async_vault` (`c359962`) | create → deposit → claim → redeem | TF-03 (only the current-epoch fee is checked) | Tests the dormant-switch finding, not just static fees |
| 5 | [TrainProtocol/contracts](https://github.com/TrainProtocol/contracts) `train-htlc` (`3dbda69`) | lock → redeem → refund | **Negative control:** should survive TF-02 and reject PD and hook mints. MEMO-01 and TF-03 are still open | Shows that ExtensionLab does not just produce noise on careful code |

Vendoring third-party code into `targets/` means following each repository's license. A fetch
script pinned to a commit is safer than copying the code.

## 6. Hackathon and grant targets

| Target | Dates | Status | Recommendation |
|---|---|---|---|
| **Solana Foundation grants** | Rolling. About 1 week to review, about 3 weeks to a decision. Criteria: public good, open source, only possible on Solana, clear milestones ([solana.org/grants-funding](https://solana.org/grants-funding), fetched 2026-10-21) | Rolling; the page states no pause | **Primary.** Apply once the MVP items 1 to 4 exist (target: week 4). Proposed milestones: catalog v1, Anchor plus LiteSVM adapters, CI export, 3 public case studies |
| Solana Foundation India Grants (Superteam) | Rolling, up to 10,000 USDG, India-based applicants, "Developer Tooling" is a listed theme ([API record](https://superteam.fun/api/grants/solana-foundation-india-grants), fetched 2026-10-21) | **Unconfirmed.** The record shows `"status":"OPEN"` and also `"isPaused":true` | Secondary, if eligible. Check that it has reopened before applying. Research/03 reported it as open without noting the pause |
| Solana Hacker House London | 2026-11-01 to 2026-11-12, 10:00–20:00 ([solana.com Breakpoint guide](https://solana.com/news/breakpoint-guide-part1), fetched 2026-10-21) | Confirmed. Not a competition | Optional. In-person building and integrator feedback; needs travel to London |
| Breakpoint 2026 | 2026-11-15 to 2026-11-17, Olympia, London (same source) | Confirmed. Not a competition | Distribution only |
| Next Colosseum hackathon | None announced (research/03, plus a web search on 2026-10-21) | **Unconfirmed** | Watch. An April 2027 event is research/03's inference only |
| Colosseum Eternal | Rolling sprints | Paused, per research/03 (not re-checked) | Not available |

No hackathon with a confirmed open deadline was found. The "weekend cut" below is therefore the
minimum demo for a grant application or the Hacker House, not an entry to a specific event.

## 7. Four-week plan (2026-10-22 to 2026-11-18)

### Week 1 (Oct 22–28): runner and golden target

- Implement `Runner::run_case` for flow steps:
  - seeded keypairs;
  - token-account snapshots (amount, withheld, exists);
  - invariants, classification and diagnosis;
  - per-case harness errors become `error` runs.
- Write the escrow adapter from raw instruction bytes, with the golden test for F1 and F2.
- Decide the baseline gate (decision 1 below). Done: SPL Token only.
- Repository hygiene:
  - add a LICENSE (done 2026-10-22: proprietary, all rights reserved);
  - add the CI workflow;
  - remove absolute paths;
  - decide what happens to `research/fixcheck/escrow_patched`.

### Week 2 (Oct 29–Nov 4): cheap P0 scenarios and the first third-party target

- Add issuer steps (`set_transfer_fee` with an epoch warp, `pause` and `resume`, `advance_clock`)
  and factory support for Pausable and CpiGuard.
- Run on the escrow: TF-03, PD-01, DAS-01, DAS-02, MEMO-01, CPIG-01, PAUSE-01. Run them on the
  fix-A copy as well, so F1 does not mask the refund path.
- Adapter for the program-examples escrow. Run the default matrix and write the case study
  (`research/05_program_examples_escrow.md`) with actual results.

### Week 3 (Nov 5–11): breadth and hooks

- Two more targets: Sablier solsab, and Train HTLC as the negative control.
- Hook programs in `hooks/` (allow, deny, extra-account), built with `cargo build-sbf` 3.1.10 and
  recorded with a sha256. Then run HOOK-01, HOOK-02 and HOOK-03.
- Run a GitHub workflow once for real against `targets/escrow`.

### Week 4 (Nov 12–18): packaging and the application

- README and adapter-author guide.
- Promote the research/02 probe into repository tests.
- The case studies, published as they are.
- Assemble the Solana Foundation grant material: actual results, milestones, links. The author
  writes the application.
- Optional: check F2 on devnet. This needs a funded devnet key and is the author's call.

Breakpoint (Nov 15–17) overlaps this week.

### Weekend cut (2 days)

1. `run_case` for flow steps only: no issuer steps, no clock, no credits.
2. The escrow adapter with `make`, `take` and `refund`.
3. A matrix with `spl_token`, `token_2022` and `transfer_fee_200bps`.
4. JSON and Markdown report plus `regress`. Both already exist.
5. A golden test for F1 and F2.

Demo: run the matrix and get two FAIL runs with logs and balances. Apply fix A
(`research/fixcheck/fix_A_B.diff`) and re-run. The refund cases pass, and take with a fee mint still
fails until fix B.

Out of the weekend cut: hooks, a third-party target, CI.

## 8. Next 10 tasks

Status (2026-10-22): tasks 1 (runner), 2 (escrow adapter), 3 (golden test), 4 (baseline gate,
SPL Token only) and 5 (issuer steps, Pausable, CpiGuard) are done; 6 onward are open. Known gaps
found in review and not fixed yet:

- A drain scenario whose permanent-delegate slot has no custody account (for the escrow,
  `permanent_delegate@mint_b/make_drain_*`) drains nothing and reports `pass`; the drain step
  succeeds with no transaction.
- The escrow's `update` flow is not in any default scenario (it is neither entry nor exit).
- A step interrupted by a harness error is listed as `skipped`; `harness_error` explains it.
- `Adapter::expected_deltas` is called after the run, so an adapter that reads the fee from the
  world sees the fee at the final epoch, not the one in effect when the flow ran. Refundability
  does use the fee in effect at each transfer.

1. **Runner.** Implement `Runner::run_case` in `crates/extensionlab-core/src/runner.rs`, using
   `mint.rs`, `invariants.rs`, `report.rs` and `diagnosis.rs`. Derive keypairs from
   `(seed, case id, name)`. The current tests use `Keypair::new()`, so compute units vary between
   runs (see section 9).
2. **Escrow adapter.** Create `adapters/escrow/` (workspace member `extensionlab-adapter-escrow`)
   with `src/lib.rs`, following the sketch in `research/04_architecture.md` §4, and `src/main.rs`
   calling `extensionlab_cli::main_with`. Skip with a clear message when
   `targets/escrow/target/deploy/escrowq32026.so` is missing.
3. **Golden test.** Add `adapters/escrow/tests/golden.rs`. It asserts the status and diagnosis of
   `token_2022/make_refund` (2009) and `transfer_fee_200bps@mint_a/make_take` (0x23), and the vault
   balance 9,800,000 + 200,000.
4. **Baseline gate.** Implement the chosen rule in `crates/extensionlab-core/src/report.rs`
   (`exit_code`) and `matrix.rs` (`Case::is_baseline`). Done (2026-10-22): `Case::is_baseline`
   is true only when every slot is an extension-free SPL Token mint, so the escrow exits 1.
5. **Issuer steps and factory additions.**
   - Issuer steps in `crates/extensionlab-core/src/matrix.rs`: `SetTransferFee{slot,bps}`,
     `AdvanceEpoch{n}`, `Pause{slot}`, `Resume{slot}`.
   - Factory additions in `mint.rs`: `MintExtension::Pausable`, `AccountExtension::CpiGuard`.
   - Tests in `crates/extensionlab-core/tests/mint_factory.rs`.
   - Done (2026-10-22): `IssuerAction::{SetTransferFee, Pause, Resume}` and `Step::AdvanceEpoch`;
     refundability uses the fee in effect at each transfer (`mint::epoch_transfer_fee`); tests in
     `tests/mint_factory.rs`, `tests/runner_steps.rs` (program-free, runs in CI) and the escrow
     golden test `cpi_guard_and_pause_scenarios`. Token-2022 10.0.0 supports both extensions.
   - Done (2026-10-22): `pausable` and `cpi_guard` are in the `all` preset (not `default`), with
     `IssuerAction::PauseAll`, the default `{entry}_pause_{exit}` scenarios, and the diagnosis rules
     `cpi_guard.user_signed_cpi` and `pausable.paused_mint`. On the escrow, F1 masks the pause on
     `make_pause_refund`.
6. **Probe in the repository.** Move the research/02 probe (`target/ext-probe/src/main.rs`, which is
   gitignored today) into `crates/extensionlab-core/tests/extension_probe.rs`, so the matrix's
   `[probe]` evidence can be reproduced from the repository. Run it on LiteSVM 0.10.0 (Token-2022
   10.0.0) and record any difference from the 0.16.0 (11.0.0) results.
   Done (2026-10-22): `tests/extension_probe.rs`, 19 tests and 127 checks, host-only. On 10.0.0,
   122 are the same, 2 differ and 3 could not be run. CT.4: NonTransferable + ConfidentialTransfer
   without ConfidentialMintBurn is accepted. PB.1: PermissionedBurn does not exist, so its size
   and its two burn checks were not run. research/02 has a "Re-run on LiteSVM 0.10.0" section.
   The 0.16.0 probe source and output are kept in `research/logs/02_probe_litesvm-0.16.0_*`.
7. **Third-party target.** Add `scripts/fetch_targets.sh`, pinned to `program-examples@de09019`,
   plus `adapters/program-examples-escrow/`. Write the results to
   `research/05_program_examples_escrow.md`.
8. **Repository hygiene.**
   - `LICENSE`. Done (2026-10-22): proprietary, all rights reserved, at the author's request.
   - Done (2026-10-22): `.github/workflows/ci.yml` runs fmt, clippy and `cargo test --workspace`
     on the host (the golden tests skip without the `.so`), and the `/Users/...` paths in
     `research/` are replaced with repo-relative ones.
   - Delete `research/fixcheck/escrow_patched/` but keep `fix_A_B.diff` and its `fixcheck.rs`, or
     keep the whole copy. Author's choice.
9. **Hook programs.** Create `hooks/allow/`, `hooks/deny/` and `hooks/extra-account/`, each with an
   `InitializeExtraAccountMetaList`, and record sha256s in `environment.programs`. Switch the `all`
   matrix in `matrix.rs` from `transfer_hook_undeployed` to these.
10. **Token-2022 version pin.** Add a way to load a pinned Token-2022 binary instead of the one
    LiteSVM bundles, and record its sha256 in every report (`runner.rs`,
    `bundled_token_programs`). This answers research/04 open question 2 and the research/02
    version caveat.

### Decisions for the author

1. **Baseline gate.** Decided 2026-10-22: SPL Token only. Before, plain Token-2022 counted as a
   baseline, so the escrow exited 4 ("baseline broken"), which presented F1, a real Token-2022
   compatibility bug, as "nothing else means anything". Plain Token-2022 is now an ordinary case
   with program scope, and the escrow exits 1.
2. **Fee-explained mismatches.** Should a balance that differs exactly by the configured fee get a
   softer status, or must adapters write fee-aware expectations? Recommendation: keep `fail` and
   ship a helper for fee-aware expectations, so the default shows the shortfall.
3. **License**: decided 2026-10-22, proprietary (all rights reserved); the repository is private.
   Still open: whether the research write-ups (written by agents) ship as they are if it is ever
   made public.

## 9. Risks and what would make this direction wrong

**Risks**

- **Adapter cost.** If an adapter for a third-party program costs about as much as the 1,062-line
  hand-written test file, the product reduces to the catalog document (research/02).
  - Kill or pivot signal: the program-examples adapter takes more than a day, or more than about
    300 lines.
- **Low hit rate.** If three or more of the five targets show nothing beyond an expected fee
  shortfall, or reject Token-2022 cleanly, the pain is narrower than research/03 suggests.
  - In that case, pivot to the "which real mints does my allow-list accept" report
    (payment-channels #83) or to regression lock-in for audited protocols.
- **Production fidelity.** LiteSVM 0.10.0 bundles Token-2022 10.0.0. The research/02 probe ran on
  11.0.0 and the latest upstream release is 11.1.0. Mainnet's version is **unverified**.
  - A result that differs on mainnet would undermine trust. Mitigation: task 10.
- **Version lock.** The workspace pins the escrow's crate family (LiteSVM 0.10, solana-* 3.x,
  `spl-token-2022-interface` 2.1.0). Adapters that use a target's Anchor-generated types would pull
  in that target's `anchor-lang` and can conflict. Prefer raw instruction bytes or an IDL.
- **Building third-party programs.** `cargo build-sbf` may need other platform-tools versions.
  - On this machine, the first build with cargo-build-sbf 3.1.10 changed which platform-tools
    toolchain rustup links (`research/logs/00_build_sbf.log`).
  - Building many targets could repeat this. Use one pinned toolchain in CI.
- **Noise.** Nominal expectations make every fee mint "fail". Without clear diagnosis, users may
  ignore the report (decision 2).
- **Masking.** One failure can hide another. On the escrow, F1 hides MEMO-01 and TF-02 on refund.
  Matrices should also run on the patched copy.
- **Competition.** Trident or LiteSVM could add presets. The scenario catalog and diagnosis are the
  only moat, and it is thin.
- **Funding.** The Solana Foundation grant is discretionary. The India grant record shows
  `isPaused: true`.

**What would make this direction wrong**

- The third-party targets do not break, or their maintainers already reject Token-2022 mints. Then
  developers do not need this tool.
- Developers only meet these issues at audit time. Then the buyer is auditors, and the product
  should be an auditor's checklist runner, not a developer CI tool.
- LiteSVM's Token-2022 behavior differs from mainnet for the P0 scenarios. Then a local-first design
  is the wrong base, and forking real state (Surfpool-style) matters more.
