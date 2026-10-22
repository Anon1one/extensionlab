# ExtensionLab

Executable compatibility tests for Solana token integrations.

ExtensionLab is for developers who build escrows, payment flows, vaults and other programs that
take custody of tokens. The goal is to run your real deposit, withdraw and refund flows against
controlled Token-2022 configurations, such as transfer fees, frozen-by-default accounts, permanent
delegates, required memos and transfer hooks. For each case you get:

- the failing transaction and its logs;
- expected vs actual balances, including withheld transfer fees;
- the assumption that broke;
- a regression test you can run in CI.

It runs locally on [LiteSVM](https://github.com/LiteSVM/litesvm), using the real SPL Token,
Token-2022 and Associated Token Account programs that LiteSVM bundles.

## Status (2026-10-22): alpha 0.3.0, four targets and a CI compatibility gate

ExtensionLab runs cases against the escrow in `targets/escrow` and three unmodified, pinned
outside targets: a Solana Foundation escrow example, Sablier Merkle Instant airdrops and Train HTLC.
Reports can be compared between revisions to catch new failures and lost coverage. This is
local compatibility testing, not a security audit or a claim of mainnet equivalence.

| Part | State |
|---|---|
| `crates/extensionlab-core` | Mint factory for SPL Token and Token-2022 with TransferFee, DefaultAccountState(Frozen), PermanentDelegate, TransferHook (program id only), NonTransferable, Pausable, and MemoTransfer and CpiGuard on actor accounts, tested against LiteSVM's bundled programs. Scenario matrix and case expansion, invariant checks, run classification, the broken-assumption catalog, JSON and Markdown reports, CI exit codes, and regression-stub generation. **The runner:** `Runner::run_case` executes a case in a fresh LiteSVM with seeded keypairs (mints, actor accounts, flows, issuer actions: drain, freeze, harvest, set transfer fee, pause, resume, pause all; clock and epoch steps), snapshots the tracked accounts, checks the invariants, and classifies and diagnoses the run. Harness problems become `error` runs. |
| `crates/extensionlab-cli` | `init`, `matrix`, `report`, `regress`, `adapters` and `run`. `init` creates an isolated adapter crate; the protocol wires its real flows into it. |
| `adapters/escrow` | Adapter for `targets/escrow` built from raw instruction bytes (no `anchor-lang` dependency), 341 non-blank lines without comments (after `cargo fmt`). Two variants: `escrow` (take creates the payout accounts) and `escrow-existing-atas` (they exist beforehand, which is what exercises required memos on payouts). The `extensionlab-escrow` binary. Golden tests check that the runner reproduces F1 and F2 below, plus harvest, expiry, freeze, CPI Guard, pause and multi-transaction behaviour. |
| `adapters/program-examples-escrow` | First third-party adapter: `solana-foundation/program-examples` at `de090195e4de698e150572c4f886c333323a64a5`. Raw instructions, no Anchor dependency, existing payout ATAs, 268 non-blank/non-comment adapter lines. Four tests cover the baseline, both fee-blocked exits, harvest recovery, delegate drain and missing-binary errors. |
| `adapters/sablier-merkle-instant` | Non-escrow campaign funding, one-leaf Merkle claim and creator clawback. Nominal and explicitly fee-aware client profiles use the same unmodified program; four golden tests check failures and successful settlement. |
| `adapters/train-htlc` | Measured-net token lock, redeem and expired sender refund. Four golden tests cover extension rejection, withheld-fee close failures and real harvest recovery. |
| `targets/escrow` | An Anchor escrow written by the repository author during Turbin3 Q3 2026. It is a learning project, and its `Anchor.toml` configures localnet only. It has one added test file that reproduces its Token-2022 failures by hand. The program code is unchanged. |
| `research/` | The reproduction write-up, an extension risk matrix, market and competitor notes, and the architecture. |
| Transfer-hook test programs | Not written yet. CI (`.github/workflows/ci.yml`) and the license files exist. |

See [PLAN.md](PLAN.md) for what comes next and what would make this direction wrong.

## First result: Token-2022 against an Anchor escrow

The escrow's README says it is "not tied to one token program" because it uses `transfer_checked`
through `TokenInterface`. Its SPL Token tests pass. Against Token-2022 in LiteSVM, the bundled
Token-2022 10.0.0 binary gives these results:

| | Finding | Observed result |
|---|---|---|
| F1 | `refund` fails for **every** Token-2022 mint, with or without a fee. `refund.rs` omits `associated_token::token_program`, so Anchor 1.1.2 derives the expected ATA with the legacy SPL Token program id. After expiry, `take` and `update` return `EscrowExpired`, so the deposit cannot be recovered. | Anchor `ConstraintAssociated` (2009) |
| F2 | `take` fails whenever `mint_a` has a transfer fee. The fee withheld in the vault during `make` blocks closing the vault. Anyone can unblock it by calling `HarvestWithheldTokensToMint` first. | Token-2022 `AccountHasWithheldTransferFees` (35, 0x23) |
| F3 | The amounts are silently wrong with a 200 bps fee and a 10,000,000 deposit. The escrow records no deposit amount, and `max_receive` bounds only the gross amount. | Vault 9,800,000 (+200,000 withheld). Taker receives 9,604,000 after a harvest. A fee change by the issuer after `make` cuts the maker's net from 6,860,000 to 6,650,000 |
| F4 | One escrow cannot mix an SPL Token mint and a Token-2022 mint. | `ConstraintMintTokenProgram` (2022) or `IncorrectProgramId` |

These are not new Token-2022 facts:

- F1 is a missing Anchor constraint.
- F2 is documented Token-2022 behavior
  ([Token-2022 extensions guide](https://www.solana-program.com/docs/token-2022/extensions#transfer-fees)).

What the reproduction shows is the workflow ExtensionLab is meant to automate. Two candidate fixes
were checked in a throwaway copy (`research/fixcheck/`):

- Fix A adds the missing constraint.
- Fix B harvests the withheld fees before closing the vault.

The `repro_` tests turn red when the fixes land, as a regression test should. Full write-up, logs and
fix tradeoffs: [research/01_repro_escrow_fee_token.md](research/01_repro_escrow_fee_token.md).

## Quickstart

Requirements:

- A stable Rust toolchain. Only Rust 1.98.1 on macOS has been tried; no minimum version is declared
  yet.
- For runs against compiled protocol targets, the Solana CLI, for `cargo build-sbf`. Version 3.1.10 was used.

### Host workspace (no Solana toolchain needed)

```sh
cargo test --workspace --locked # 101 tests, including 22 protocol golden tests.
                              # Program runs skip until their .so is built; target CI jobs require it.

cargo run -p extensionlab-cli -- matrix list
cargo run -p extensionlab-cli -- matrix show default

# The fixture below is a hand-written example report, not runner output. Two of its runs copy
# logs observed in the escrow reproduction; one is hypothetical.
cargo run -p extensionlab-cli -- report render crates/extensionlab-core/tests/fixtures/example_report.json
cargo run -p extensionlab-cli -- report exit-code crates/extensionlab-core/tests/fixtures/example_report.json   # prints 1
cargo run -p extensionlab-cli -- regress crates/extensionlab-core/tests/fixtures/example_report.json \
  --run 'transfer_fee_200bps@mint_a/make_take'
```

Exit codes:

| Code | Meaning |
|---|---|
| 0 | No failures |
| 1 | At least one failing run |
| 2 | Usage error |
| 3 | Harness error, empty matrix or no applicable cases |
| 4 | A baseline case failed (every slot on SPL Token without extensions) |

An issuer action with no eligible accounts or tokens is **not applicable**, never a pass.
Other steps still run, and actual failures take precedence over not-applicable status.

### Start an adapter in your own repository

From the ExtensionLab checkout:

```sh
cargo run --locked -p extensionlab-cli -- init --name my-protocol --out ../my-protocol-extensionlab
cd ../my-protocol-extensionlab
cargo run -- run --adapter my-protocol --matrix default
```

The new crate has its own Cargo workspace and path dependencies on this checkout. Wire its
`src/lib.rs` to the actual compiled program, actors, transactions, tracked accounts and expected
balances. Until then it returns harness errors (exit 3), with no passing compatibility runs.
Existing output directories are refused. Use `--extensionlab-path` to choose another SDK checkout.

### Compare revisions in CI

```sh
cargo run --locked -p extensionlab-cli -- report compare base/report.json head/report.json --out comparison
```

This writes `comparison.json` and `comparison.md`. Known failures remain visible; new failures,
worsening balance mismatches, removed cases or expectations, and lost invariant coverage fail
the gate. Case definitions and seeds must match. Token-runtime changes fail with exit 3 unless
explicitly acknowledged with `--allow-runtime-change`; protocol ELF changes are expected.
Use reports from trusted base/head builds using the same ExtensionLab version.

The root `action.yml` packages this for GitHub Actions after the caller builds its adapter and
target programs. It requires no API key or repository write permission:

```yaml
- uses: Anon1one/extensionlab@<reviewed-commit-sha>
  with:
    binary: target/debug/my-protocol-extensionlab
    adapter: my-protocol
    baseline-report: compatibility/base-report.json # optional, from the trusted base revision
    out: extensionlab-report                       # must be a fresh directory
```

Without a baseline report, the action gates on all current failures. It adds evidence to the
job summary; callers can upload the output directory with `actions/upload-artifact`.
It does not fetch a baseline automatically.

`run --token-2022-so <path> --token-2022-sha256 <hash>` uses an explicit runtime. Reports record
the actual Token, Token-2022 and ATA ELF hashes and the seed. Generated regressions preserve
the seed and check the Token-2022 hash; set `EXTENSIONLAB_TOKEN_2022_SO` when replaying a custom ELF.

To run the escrow against a matrix (needs the escrow binary, see below):

```sh
cargo run -p extensionlab-adapter-escrow -- run --adapter escrow --matrix default --out extensionlab-report
```

This writes `extensionlab-report/report.json` and `report.md`. On the default matrix it gives 32 runs:
9 pass, 4 rejected, 18 fail, 0 error, 1 not applicable (`--adapter escrow-existing-atas`: 8 pass,
4 rejected, 19 fail, 0 error, 1 not applicable;
the memo runs fail and the frozen-`mint_b` take passes). Both exit 1. Only SPL Token cases count as
the baseline (both pass), so the refund failing on plain Token-2022 (F1) is an ordinary failure, not a
broken baseline (PLAN.md, decision 1).

`--matrix all` adds an undeployed transfer hook, a Pausable mint (with a pause between entry and
exit) and CpiGuard on the actors' accounts: 48 runs, 11 pass, 8 rejected, 28 fail, 0 error,
1 not applicable, exit 1
for `--adapter escrow`. CPI Guard on the maker's account makes `make` fail with error 42, so the
deposit is refused; on the taker's account `take` fails the same way after the deposit. A pause
blocks `take` with error 67; `refund` fails on F1 before it reaches the pause.

### First third-party result: Solana Foundation escrow

The source at [`de090195e4de698e150572c4f886c333323a64a5`](https://github.com/solana-foundation/program-examples/tree/de090195e4de698e150572c4f886c333323a64a5/tokens/escrow/anchor)
was fetched with its MIT license and built without changing program code. The checked-in
`targets/locks/program-examples-escrow.Cargo.lock` pins its build dependencies (including Anchor
1.2.1, which satisfies its declared `1.0.2` dependency). The report records the program ELF hash.
These results were run locally on 22 October 2026 with the same LiteSVM/Token-2022 versions above:

| Matrix | Pass | Rejected | Fail | Harness error | Not applicable |
|---|---|---|---|---|---|
| `default` (32 runs) | 17 | 4 | 9 | 0 | 2 |
| `all` (48 runs) | 24 | 8 | 14 | 0 | 2 |

- SPL Token and plain Token-2022 both pass take and refund. The original target's F1 does not
  occur here: this example names the token program in its refund ATA constraints.
- A 200 bps fee on `mint_a` blocks **both** take and refund with `AccountHasWithheldTransferFees`
  (35). The rolled-back exit leaves 9,800,000 spendable plus 200,000 withheld in the vault.
- A permissionless harvest unblocks both exits. Take delivers 9,604,000 to the taker; refund
  restores the maker to 99,604,000. Nominal expectations still fail on the fee shortfall, while
  refundability and closeability hold.
- A permanent delegate can drain the real `mint_a` custody balance before either exit. The
  transfers succeed, but the invariants detect the shortfall.
- The two not-applicable cases put the delegate on `mint_b`, which has no custody account.
  Their drain action sends no transaction; they are not evidence of drain resistance.

Reproduce with Solana `cargo-build-sbf` 3.1.10 and its platform-tools v1.52:

```sh
scripts/fetch_targets.sh program-examples
(cd targets/program-examples/tokens/escrow/anchor && \
  RUSTUP_TOOLCHAIN=stable CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf --tools-version v1.52 -- --locked)
cargo run -p extensionlab-adapter-program-examples-escrow --locked -- \
  run --adapter program-examples-escrow --matrix default --out extensionlab-report/program-examples
# Exit 1 means the expected compatibility failures, not a harness error.
```

The fetch script refuses a different or modified existing checkout and supplies the pinned
lockfile. `EXTENSIONLAB_PROGRAM_EXAMPLES_ESCROW_SO` overrides the binary path. With
`EXTENSIONLAB_REQUIRE_TARGET_BINARIES=1`, missing binaries fail the new adapter tests rather than
skipping. The workflow's separate `program-examples` job builds this target, runs the matrix,
exports an `--expect observed` regression and compiles/runs it. The hosted build, matrix, tests,
regression export and action passed in [run 39801608961](https://github.com/Anon1one/extensionlab/actions/runs/39801608961). The default
pass-required export also compiles and fails on the observed incompatibility.

The separate `exported-regression` job builds the original escrow and the existing candidate
fix, then runs `bash scripts/check_regression_fix.sh`. The exported plain Token-2022 refund test
was verified locally to fail on the original program and pass after fix A. This does not claim
that fix B resolves the nominal fee-shortfall expectations.

### Non-escrow result: Sablier Merkle Instant

The adapter runs `create_campaign` plus funding in one transaction, then a real one-leaf Merkle
`claim` or creator `clawback`, against
[`solsab@e060b26a6c736a3104a0b24ac069fd53633b3ceb`](https://github.com/sablier-labs/solsab/tree/e060b26a6c736a3104a0b24ac069fd53633b3ceb/programs/merkle_instant).
At this pin the claim fee is zero, so no oracle service is needed. Program source is unchanged;
its BUSL-1.1 license and the repository's other license files stay in the ignored source checkout.

| Default matrix | Pass | Rejected | Fail | Harness error |
|---|---|---|---|---|
| `sablier-merkle-instant` (18 runs) | 8 | 4 | 6 | 0 |
| `sablier-merkle-fee-aware` (18 runs) | 10 | 4 | 4 | 0 |

With a 200 bps fee, nominal funding of 10,000,000 leaves 9,800,000 spendable plus 200,000 withheld.
A 10,000,000 leaf claim or nominal clawback fails with `InsufficientFunds` (1). This demonstrates
an underfunded client flow; it does not establish a permanent fund lock.

The opt-in fee-aware client sends 10,204,082 to fund the same 10,000,000 campaign. Both exits
then pass their explicit net expectations: the claimant receives 9,800,000, or the creator ends
with 99,595,918 after clawback. This profile deliberately promises a net payout, not the leaf's
gross amount. Global nominal-mismatch classification remains `fail`. Permanent-delegate drains
and issuer pauses still fail; this is a positive control for the fee case, not universal support.

```sh
scripts/fetch_targets.sh solsab
(cd targets/solsab && \
  RUSTUP_TOOLCHAIN=stable CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf \
  --manifest-path programs/merkle_instant/Cargo.toml --tools-version v1.52 -- --locked)
cargo run --locked -p extensionlab-adapter-sablier-merkle-instant -- \
  run --adapter sablier-merkle-instant --out extensionlab-report/sablier
cargo run --locked -p extensionlab-adapter-sablier-merkle-instant -- \
  run --adapter sablier-merkle-fee-aware --out extensionlab-report/sablier-fee-aware
```

### Comparison target: Train HTLC

The adapter uses unchanged
[`contracts@3dbda6949e35e9095493333a6fff7dfe4ccde1aa`](https://github.com/TrainProtocol/contracts/tree/3dbda6949e35e9095493333a6fff7dfe4ccde1aa/chains/solana/programs/train-htlc)
(MIT): a token lock, recipient redeem, or sender refund after the actual 60-second timelock.
It reads the real lock's recorded amount, which correctly measures the net deposit.

| Matrix | Pass | Rejected | Fail | Harness error |
|---|---|---|---|---|
| `default` (22 runs) | 10 | 8 | 4 | 0 |
| `all` (30 runs) | 12 | 12 | 6 | 0 |

Plain SPL Token and Token-2022 settle; permanent-delegate and hook mints are rejected before
custody. The earlier research prediction that a failed close could be tolerated was wrong:
with 200 bps fees, both exits revert at `CloseAccount` (35), and the attempted catch branch never
logs. A callee error immediately fails the transaction, as the
[Solana CPI API documents](https://docs.rs/solana-cpi/latest/solana_cpi/fn.invoke.html).

A real permissionless harvest before the exit makes both fee-bearing flows pass their net
expectations: redeem delivers 9,604,000, refund restores the sender to 99,604,000, and the vault
closes. The adapter includes these recovery scenarios. Train is therefore a comparison target,
not the universally passing negative control originally predicted.

```sh
scripts/fetch_targets.sh train
(cd targets/train/chains/solana && \
  RUSTUP_TOOLCHAIN=stable CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf \
  --manifest-path programs/train-htlc/Cargo.toml --tools-version v1.52 -- --locked)
cargo run --locked -p extensionlab-adapter-train-htlc -- \
  run --adapter train-htlc --matrix all --out extensionlab-report/train
```

The new `protocol-solsab` and `protocol-train` CI jobs fetch the pinned sources, build with their
upstream lockfiles, require real binaries, run golden tests, compile passing recovery regressions,
and exercise the reusable action against a same-build reference. Use
`EXTENSIONLAB_SABLIER_MERKLE_SO` or `EXTENSIONLAB_TRAIN_HTLC_SO` to select another ELF; reports
record its actual hash and label the source revision as expected rather than verified provenance.
These are local, pinned-runtime results; deployed behavior and external adoption remain unverified.

### Escrow reproduction

```sh
cd targets/escrow
cargo build-sbf     # builds target/deploy/escrowq32026.so; the tests load it from there
cargo test          # 20 tests: 1 unit, 3 SPL Token, 16 Token-2022
cargo test --test token2022_fees -- --nocapture --test-threads=1   # with program logs
```

Notes:

- The `.so` is not committed, so `cargo test` fails until `cargo build-sbf` has run.
- `cargo build-sbf` also generates a program keypair under `target/deploy/`, which is gitignored. It
  may download platform-tools on first use.
- Compute-unit figures printed by the tests vary from run to run, because the tests use random
  keypairs. The asserted balances and error codes do not vary.

## Layout

```
crates/extensionlab-core/   adapter trait, mint factory, matrix, invariants, diagnosis, report, regression, runner
crates/extensionlab-cli/    `extensionlab` binary; protocol repos build their own binary with `main_with(registry)`
adapters/escrow/            escrow adapter, `extensionlab-escrow` binary, golden test for F1 and F2
adapters/program-examples-escrow/  pinned outside-target adapter and its CLI/golden tests
adapters/sablier-merkle-instant/  airdrop adapter, nominal and fee-aware client profiles
adapters/train-htlc/         measured-net HTLC adapter and harvest recovery cases
scripts/fetch_targets.sh    fetch pinned outside targets with their licenses and dependency locks
targets/locks/              dependency lockfiles for fetched targets
targets/escrow/             protocol under test (separate Anchor workspace, not a Cargo workspace member)
research/                   write-ups and logs behind every claim in this README
```

## How it is meant to work

The design is in [research/04_architecture.md](research/04_architecture.md). In short:

- **Adapter.** A protocol writes an `Adapter` in Rust. It declares the token slots, the actors and
  the named flows, builds each flow's instructions, and states the balance changes it expects.
- **Run.** The runner creates mints and accounts through the real token programs. It plays the
  token issuer, which covers fee changes, freezes and permanent-delegate transfers. It runs each
  case in a fresh LiteSVM, sends every transaction itself, and snapshots balances after each step.
- **Checks.** Pure functions check:
  - conservation;
  - expected balances;
  - credited vs received amounts;
  - refundability;
  - closeability.

  They then classify the run and name the broken assumption, which is a heuristic and is labeled
  as one in every report.

The scenario catalog is drafted in
[research/02_extension_matrix.md](research/02_extension_matrix.md) (JSON copy alongside). It has 16
extension entries with error codes, LiteSVM setup recipes and P0/P1/P2 priorities. Its
`[probe]` evidence was first produced with LiteSVM 0.16.0 and Token-2022 11.0.0. The probe is now
`crates/extensionlab-core/tests/extension_probe.rs` and runs on the workspace's LiteSVM 0.10.0
(Token-2022 10.0.0): of its 127 checks, 122 give the same result, 2 differ and 3 cannot run there
(research/02, "Re-run on LiteSVM 0.10.0").

## Roadmap

From [PLAN.md](PLAN.md). Item 1 (runner, escrow adapter, golden test) is done.

2. Cover the cheap P0 scenarios against the escrow. Done for the permanent-delegate drain, the
   frozen-by-default vault, a freeze after deposit, a required memo on payout, CPI Guard on the
   depositor's account and a pause (default and `all` matrices, golden tests). A fee change
   mid-flight is supported by the runner (`set_transfer_fee`, `advance_epoch`) but no escrow
   scenario uses it yet.
3. Done locally (22 Oct): the default and `all` matrices were run against the pinned
   `solana-foundation/program-examples` escrow; actual results are above. The source is unchanged.
   External maintainer feedback and adoption remain unverified.
4. Build transfer-hook test programs (allow, deny, extra account) and add the hook scenarios.
5. Done: reusable CI action, report comparisons, pinned runtime hashes, adapter scaffolding and
   a non-escrow airdrop target. Fee-aware Sablier funding and Train harvest recovery supply
   passing fee cases. External developer trials remain the next product-validation step.

## Caveats

- All results so far come from the Token-2022 10.0.0 binary bundled in LiteSVM 0.10.0. Whether
  mainnet or devnet behave the same has not been checked.
- Severity labels in `research/` are the authors' judgment, not an audit rating.
- `research/02` and `research/03` tag each claim with its evidence (`[probe]`, `[src]`, `[doc]`,
  `[predicted]`, `[unverified]` and similar). Only `[probe]` claims, the mint factory tests and the
  reproduction in `research/01` were observed by running code.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this repository by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
