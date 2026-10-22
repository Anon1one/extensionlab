# 04 · ExtensionLab architecture and scaffold

Date: 2026-10-21, updated 2026-10-22. Status: the runner is implemented (section 7) and the escrow
adapter in `adapters/escrow` reproduces F1 and F2 automatically (golden test).
Section 15 lists what was verified and how. Everything not listed there is design and is marked
unverified where it makes a factual claim.

## 1. What ExtensionLab is

Executable compatibility tests for Solana token integrations. A developer building an escrow,
payment flow, vault or other protocol writes one adapter for their program. ExtensionLab runs the
program's real flows (deposit, withdraw, refund, ...) on LiteSVM against a matrix of controlled
token configurations: classic SPL Token, plain Token-2022, and Token-2022 with one extension at a
time. For every run it reports:

- pass, rejected, fail or error;
- the failing transaction: LiteSVM signature, error and program logs;
- expected versus actual balance for every tracked account, including withheld transfer fees;
- which invariant broke (conservation, credited == received, refundability, closeability);
- a best-effort name for the broken assumption;
- a regression test stub that can be committed and run in CI.

Non-goals for now: auditing program logic unrelated to tokens, mainnet or devnet execution,
fuzzing amounts (it could come later as a matrix dimension), and judging whether rejecting an
extension is the right product decision. ExtensionLab reports what happens. The adapter author
states what should happen.

## 2. What the reference code tells us

**The escrow** (`reference/turbin3_q3_26/vault_and_escrow_q326/escrow_q326`, Anchor 1.1.2):

- Four instructions with one-byte custom discriminators: `make = 0`, `take = 1`, `refund = 2`,
  `update = 3`. Anchor 1.1.2 turns `#[instruction(discriminator = 0)]` into `&[0]`
  (`anchor-syn-1.1.2/src/lib.rs`, `impl Parse for Overrides`). An adapter can therefore build
  instruction data as one byte plus the Borsh-encoded arguments, without depending on the program
  crate or on `anchor-lang`.
- One `token_program: Interface<TokenInterface>` account per instruction. Both mints are
  constrained to it (`mint::token_program = token_program`), so both mints must belong to the same
  token program. This is why the matrix fills the slots a case does not vary with the
  extension-free config of the **same** program (section 6).
- The vault is the escrow PDA's associated token account for `mint_a`. `take` and `refund` move
  `self.vault.amount`, and the escrow state stores no deposit amount. So the protocol cannot
  notice if the vault holds less than was deposited (TransferFee) or if someone else moved tokens
  out of it (PermanentDelegate). This is the main reason for the expected-balance and
  refundability invariants.
- The tests use LiteSVM 0.10.0 and `litesvm-token` 0.10.0. They load the program from
  `target/deploy/escrowq32026.so`, build instructions from Anchor's generated `accounts::*` and
  `instruction::*`, and only exercise classic SPL Token.

**The Token-2022 tests** (`reference/turbin3_q3_26/token22-q326/programs/t22/tests`):

- Small `setup`, `send` and `send_expecting_failure` helpers. A failing transaction is turned into
  its program logs (`e.meta.logs`), and tests assert on log substrings. ExtensionLab keeps this
  pattern: every failed transaction keeps its logs, and diagnosis matches log substrings.
- Mints with extensions are built from raw instructions: `create_account` with the size from
  `ExtensionType::try_calculate_account_len`, each extension initializer, then `initialize_mint2`.
  The mint factory follows the same order.
- One test notes that the TLV order of extensions does not follow declaration order. The factory
  tests compare extension sets, not lists.
- These tests use LiteSVM 0.16 and `spl-token-2022-interface` 3.x. The escrow uses 0.10 and 2.x.
  ExtensionLab pins the escrow's versions (section 13), because the protocol under test decides the
  runtime.

**The reproduction** (`research/01_repro_escrow_fee_token.md`, a separate task) ran the unmodified
escrow against Token-2022 in LiteSVM 0.10.0 and found, among other things, that `refund` fails for
every Token-2022 mint (Anchor `ConstraintAssociated`, 2009, because `refund.rs` omits
`associated_token::token_program`) and that `take` with a fee-bearing `mint_a` fails at
`CloseAccount` (Token-2022 error 35, `AccountHasWithheldTransferFees`). Those two observations are
what the example report in section 10 is built from. They also show the workflow ExtensionLab
automates: a configured token, the protocol's real flows, the failing transaction, balances, the
broken assumption, and a test that stays red until the fix lands.

## 3. Architecture

```
                 extensionlab run --adapter escrow --matrix default
                                      |
                         extensionlab-cli (clap, Registry of adapters)
                                      |
             +------------------------+-------------------------+
             |                 extensionlab-core                |
             |                                                  |
             |  matrix::load + matrix::expand --> Vec<Case>     |
             |              |                                   |
             |  Runner::run_case (per case, fresh LiteSVM)      |   implemented
             |    1 adapter.load_programs                       |
             |    2 mint::create_mint per slot                  |   implemented
             |    3 actors + mint::create_actor_token_account   |   implemented
             |    4 adapter.setup                               |
             |    5 snapshot -> step -> snapshot ...            |
             |    6 invariants::*                               |   implemented (pure)
             |    7 report::classify, diagnosis::diagnose       |   implemented (pure)
             |              |                                   |
             |  Report (serde JSON) --> render_markdown         |   implemented
             |                      --> exit_code(FailOn)       |   implemented
             |                      --> regression::render      |   implemented
             +--------------------------------------------------+
```

Design choices:

1. **The harness sends every transaction.** An adapter returns instructions and names the actors
   that sign. The harness owns the keypairs, signs, sends, and records signature, compute units,
   fee, error and logs the same way for every protocol. Adapters only read state.
2. **One fresh VM per case.** Cases never share state. `LiteSVM` is `Clone` in 0.10.0
   (`#[derive(Clone)] pub struct LiteSVM`), so a later version can fork after an entry flow and
   try several exits from the same state without replaying setup.
3. **The harness plays the token issuer.** One issuer key is mint authority, freeze authority,
   fee authorities, permanent delegate and hook authority. This makes issuer actions such as a
   permanent-delegate drain, a freeze or a fee change part of the scenario.
4. **Adapters are Rust, compiled into the binary that runs them.** The published `extensionlab`
   binary has no adapters. A protocol repository adds a five-line binary that registers its
   adapter and calls `extensionlab_cli::main_with(registry)`. The protocol's own Rust toolchain
   and dependency versions then apply, and nothing is loaded at runtime.
5. **Pure checks, separate from execution.** Invariants, classification, diagnosis, reports and
   regression stubs are pure functions over plain data, unit-tested without a VM. Only
   `run_case` touches LiteSVM.

## 4. The Adapter trait

`crates/extensionlab-core/src/adapter.rs`. The trait is object safe (`Box<dyn Adapter>`).

| Method | Required | Purpose |
|---|---|---|
| `name`, `description` | name | Run ids, reports, `--adapter` |
| `mint_slots() -> Vec<String>` | yes | Token slots the protocol uses, e.g. `mint_a`, `mint_b` |
| `actors() -> Vec<ActorSpec>` | yes | Wallets to create: lamports, and which slot ATAs to create and fund |
| `flows() -> Vec<FlowSpec>` | yes | Named flows with a kind: `entry`, `exit` or `other` |
| `scenarios()` | default | Step sequences. The default pairs every entry with every exit and adds the drain and pause scenarios |
| `load_programs(&mut LiteSVM)` | yes | `add_program_from_file`. Token, Token-2022, ATA and Memo are already loaded by LiteSVM |
| `setup(&World) -> Vec<FlowTx>` | default | Protocol initialization before the first step |
| `build_flow(name, &World) -> Vec<FlowTx>` | yes | Instructions for a flow against the current state |
| `tracked_accounts(&World)` | yes | Token accounts to snapshot, with a `custody` flag for PDA-held accounts |
| `expected_deltas(flow, &World)` | yes | What the author assumes the flow does to each balance |
| `credits(&World) -> Vec<Credit>` | default | What the protocol's own state says it holds, and in which account |
| `closable_accounts(&World)` | default | Tracked accounts that must be gone after a successful exit |
| `support_policy(slot, &MintConfig)` | default | `unspecified`, `must_support` or `must_reject` |

`World` holds the `LiteSVM`, the issuer, actor keypairs by name, provisioned mints by slot, and
address labels for reports. Its accessors (`actor`, `pubkey`, `mint`, `ata`, `label`) are
implemented.

`expected_deltas` encodes the developer's assumption, not Token-2022's behavior. A nominal escrow
adapter says "make moves `deposit` from `maker_ata_a` to `vault`". With a fee mint that assumption
is wrong, and the report shows the difference. An author who wants fee-aware expectations can
compute them from `world.mint(slot)?.config.transfer_fee_for(amount)`; the check then passes when
the protocol handles fees the way the author intends.

### Escrow adapter sketch (not compiled, not in the repository)

```rust
const PROGRAM_ID: Pubkey = pubkey!("9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t");
const EXPIRATION: i64 = 17_780_206_209;

pub struct EscrowAdapter { so_path: PathBuf, seed: u64, deposit: u64, receive: u64 }

impl Adapter for EscrowAdapter {
    fn name(&self) -> &str { "escrow" }
    fn mint_slots(&self) -> Vec<String> { vec!["mint_a".into(), "mint_b".into()] }
    fn actors(&self) -> Vec<ActorSpec> {
        let funded = |slot: &str| vec![ActorTokenAccount { slot: slot.into(), initial_balance: 100_000_000 }];
        vec![
            ActorSpec { name: "maker".into(), lamports: 1_000_000_000, token_accounts: funded("mint_a") },
            ActorSpec { name: "taker".into(), lamports: 1_000_000_000, token_accounts: funded("mint_b") },
        ]
    }
    fn flows(&self) -> Vec<FlowSpec> { /* make: Entry, take: Exit, refund: Exit, update: Other */ }
    fn load_programs(&self, svm: &mut LiteSVM) -> Result<()> {
        svm.add_program_from_file(PROGRAM_ID, &self.so_path).map_err(|e| Error::Adapter(e.to_string()))
    }
    fn build_flow(&mut self, flow: &str, w: &World) -> Result<Vec<FlowTx>> {
        let (maker, a, b) = (w.pubkey("maker")?, w.mint("mint_a")?, w.mint("mint_b")?);
        let escrow = Pubkey::find_program_address(
            &[b"escrow", maker.as_ref(), &self.seed.to_le_bytes()], &PROGRAM_ID).0;
        let vault = a.ata(&escrow);
        match flow {
            "make" => {
                // discriminator 0, then Borsh(seed, deposit, receive, expiration)
                let mut data = vec![0u8];
                for v in [self.seed, self.deposit, self.receive] { data.extend(v.to_le_bytes()); }
                data.extend(EXPIRATION.to_le_bytes());
                let accounts = vec![
                    AccountMeta::new(maker, true),
                    AccountMeta::new_readonly(a.address, false),
                    AccountMeta::new_readonly(b.address, false),
                    AccountMeta::new(a.ata(&maker), false),
                    AccountMeta::new(escrow, false),
                    AccountMeta::new(vault, false),
                    AccountMeta::new_readonly(spl_associated_token_account_interface::program::id(), false),
                    AccountMeta::new_readonly(a.program_id(), false),
                    AccountMeta::new_readonly(solana_system_interface::program::id(), false),
                ];
                Ok(vec![FlowTx { payer: "maker".into(), signers: vec![], extra_signers: vec![],
                                 instructions: vec![Instruction { program_id: PROGRAM_ID, accounts, data }] }])
            }
            "take" | "refund" | "update" => todo!(),
            other => Err(Error::Adapter(format!("unknown flow {other}"))),
        }
    }
    fn tracked_accounts(&self, w: &World) -> Result<Vec<TrackedAccount>> {
        // maker_ata_a, maker_ata_b, taker_ata_a, taker_ata_b, and vault with custody = true
    }
    fn expected_deltas(&self, flow: &str, _w: &World) -> Result<Vec<ExpectedDelta>> {
        let (d, r) = (self.deposit as i128, self.receive as i128);
        Ok(match flow {
            "make"   => vec![delta("maker_ata_a", -d), delta("vault", d)],
            "take"   => vec![delta("vault", -d), delta("taker_ata_a", d), delta("taker_ata_b", -r), delta("maker_ata_b", r)],
            "refund" => vec![delta("vault", -d), delta("maker_ata_a", d)],
            _ => vec![],
        })
    }
    fn closable_accounts(&self, _w: &World) -> Result<Vec<String>> { Ok(vec!["vault".into()]) }
}
```

The account order follows the `Make` struct in `make.rs`. The adapter could instead use Anchor's
generated `escrowq32026::accounts::Make` and `instruction::Make`, as the reproduction in
research/01 does. That pulls `anchor-lang` 1.1.2 into the adapter build. Research/01 shows it
works next to LiteSVM 0.10.0 in one test crate.

## 5. Mint factory

`crates/extensionlab-core/src/mint.rs`. A `MintConfig` is `{label, program, decimals,
extensions, actor_account_extensions}`. `validate()` rejects empty labels and labels containing
`/` or `@`, SPL Token configs with extensions, duplicate extensions, fees above 10,000 bps, and
every combination Token-2022's own `ExtensionType::check_for_invalid_mint_extension_combinations`
rejects.

| Config | What the harness sets | What it probes in a protocol | Factory status |
|---|---|---|---|
| `spl_token` | Classic mint, 82 bytes | Baseline | Verified (test) |
| `token_2022` | Token-2022 mint, no extensions | Program-id assumptions, ATA derivation | Verified (test) |
| `transfer_fee_200bps` | `TransferFeeConfig` 200 bps, no cap; fee and withdraw authority = issuer | Received < sent; withheld fees block `CloseAccount` | Verified (test): 10,000 sent at 250 bps arrives as 9,750 with 250 withheld; closing an account with withheld fees fails with error 35 |
| `transfer_fee_0bps` | `TransferFeeConfig` at 0 bps | Account size assumptions (165-byte accounts) with no fee effect | Mint creation verified |
| `default_frozen` | `DefaultAccountState(Frozen)`, freeze authority = issuer. The harness thaws actor accounts, never protocol accounts | Protocol-created accounts start frozen | Verified (test): an ATA created without thaw is frozen and `MintTo` fails with error 17, "Account is frozen" |
| `permanent_delegate` | `PermanentDelegate` = issuer | Custody can be moved without the PDA | Mint creation verified. The drain is `IssuerAction::DrainCustody` in the runner |
| `memo_required` | Actor ATAs reallocated for `MemoTransfer` and required memos enabled (owner-signed) | Program transfers to user accounts without a memo CPI | Verified (test): a transfer into such an account without a memo fails with error 36 |
| `non_transferable` | `NonTransferable` | Deposits of soul-bound tokens | Verified (test): transfers fail with error 37 |
| `pausable` (matrix `all` only) | `Pausable`, pause authority = issuer, created unpaused | Exits while the issuer has paused the mint | Verified (test): while paused a transfer fails ("paused on this mint"), after resume it works; escrow `take` after a pause fails (golden test) |
| `cpi_guard` (matrix `all` only) | Actor ATAs reallocated for `CpiGuard` and the guard enabled (owner-signed) | Programs that move user tokens by CPI with the user as signer | Verified (test): a top-level transfer still works; escrow `make` with a guarded maker account fails with error 42 (golden test) |
| `transfer_hook_undeployed` (matrix `all` only) | `TransferHook` with program id `extensionlab-undeployed-hook-id!` (32 ASCII bytes), no program deployed | Only that a protocol fails atomically | Verified (test): the id is recorded on the mint. Behavior on transfer unverified |

LiteSVM 0.10.0 loads SPL Token 3.5.0, Token-2022 10.0.0, ATA 1.1.1 and Memo 1.0.0 and 3.0.0 by
default (`litesvm-0.10.0/src/programs/mod.rs`). Every "verified" entry above was run against those
binaries through instructions the harness sent itself, not through a protocol's CPI.

Extension semantics, from the Token-2022 documentation: a transfer fee is withheld in the
recipient account; an account that holds withheld fees cannot be closed until someone harvests
them, which anyone can do; `DefaultAccountState` can force new accounts to be frozen; a permanent
delegate has unlimited delegate rights over every account of the mint; required memos need a memo
instruction (or a memo CPI) right before the transfer; non-transferable tokens cannot move but can
be burned and closed [S1][S2]. A transfer of a hook-enabled mint must also carry the hook program,
the extra-account-metas PDA (seeds `"extra-account-metas"`, mint, hook program id) and any extra
accounts it lists [S3]. A protocol whose CPI passes only source, mint, destination and authority
does not forward them. What Token-2022 10.0.0 does in that case has not been checked here.

## 6. Scenario matrix

`crates/extensionlab-core/src/matrix.rs`.

- **Step**: `flow` (adapter flow by name), `issuer` (`drain_custody`, `freeze_custody{slot}`,
  `harvest_withheld{slot}`, `set_transfer_fee{slot, basis_points, maximum_fee?}`, `pause{slot}`,
  `resume{slot}`, `pause_all`), `advance_clock{seconds}`, or `advance_epoch{epochs}`. A new
  transfer fee takes effect two epochs after it is set; refundability allows the fee in effect at
  each transfer. An issuer action on a slot whose mint lacks the extension sends nothing.
  `pause_all`, like `drain_custody`, takes no slot: it pauses every slot's mint that has
  `pausable`.
- **Scenario**: a name, steps, and an optional `requires_extension`. The defaults derived from the
  adapter's flows are `{entry}_{exit}` for every entry/exit pair, plus `{entry}_drain_{exit}`,
  which runs only when some slot has `permanent_delegate`, and `{entry}_pause_{exit}` (with
  `pause_all` in between), which runs only when some slot has `pausable`. Adapters can return
  more, for example `make, advance_clock(expiry), refund` or a fee change between `make` and
  `take`.
- **Matrix**: a name, a list of `MintConfig`, and a slot strategy. `one_slot_at_a_time` varies one
  slot and gives the others the extension-free config of the same program. `all_slots` gives every
  slot the same config.
- **Case**: `{id, slots, varied_slot, scenario}`. Ids are `{config}@{slot}/{scenario}` when one
  slot is varied and `{config}/{scenario}` otherwise, e.g.
  `transfer_fee_200bps@mint_a/make_take`. A case whose slots are all extension-free SPL Token
  mints is a **baseline** case. Plain Token-2022 is extension-free but not baseline.

Presets: `baseline` (2 configs), `default` (8), `all` (11: `default` plus
`transfer_hook_undeployed`, `pausable` and `cpi_guard`). For the escrow's flows (make as entry;
take and refund as exits; update as other), `default` expands to 32 cases: 2 neutral configs x 2
scenarios, 6 extension configs x 2 slots x 2 scenarios, and `permanent_delegate` x 2 slots x 2
drain scenarios. `all` expands to 48: those 32, the 3 extra configs x 2 slots x 2 scenarios, and
`pausable` x 2 slots x 2 pause scenarios. Unit tests assert both counts. `--matrix` also accepts
a path to a JSON `Matrix`; `extensionlab matrix show default` prints the format.

Not covered yet: mixed-program cases (an SPL Token `mint_a` with a Token-2022 `mint_b`). Research/01
finding F4 shows they matter. See open question 8.

## 7. Run lifecycle (`Runner::run_case`)

1. `LiteSVM::new()`, then `adapter.load_programs`.
2. Derive issuer, actor and mint keypairs from `(seed, case id, name)` so addresses are the same
   on every run and reports diff cleanly. Airdrop SOL.
3. For each slot, `mint::create_mint` with the case's config.
4. For each actor token account, `mint::create_actor_token_account` and `mint::mint_tokens`.
5. `adapter.setup`. If it fails, the run is `error`: the protocol never reached a flow.
6. `adapter.tracked_accounts` plus the issuer's own token accounts (the drain destination), then
   the initial snapshot: `amount`, `withheld`, `exists`, `lamports` per account; `supply` and
   withheld pool per mint.
7. For each step: build it (flow) or perform it (issuer action, clock), send each transaction,
   record a `TxRecord`, snapshot. Stop at the first failed step and mark the rest `skipped`.
   Successful transactions drop their logs unless `keep_success_logs` is set.
8. Invariants (section 8), then `report::classify` and `diagnosis::diagnose`.
9. Per-case harness errors become `error` runs instead of aborting the matrix.

## 8. Invariants and run status

`crates/extensionlab-core/src/invariants.rs`. All are pure functions over `Snapshot`s and are
unit-tested.

| Invariant | Rule | Catches |
|---|---|---|
| `conservation` (per mint) | change in Σ(amount + withheld) over tracked accounts + change in the mint's withheld pool == change in supply. This is "sum in == sum out + fees" in delta form | Tokens moving to or from accounts the adapter does not track |
| `expected_balances` | For every account with an expected delta, cumulative over the scenario's flow steps: `actual_after == before + Σ delta` | Received < sent (fees), funds stuck after a failed exit, drained custody |
| `credited_equals_received` | Per backing account: change in credited amount == change in balance, and credits ≤ balance (solvency) | Ledgers that record the sent amount instead of the received amount. Skipped when the adapter exposes no credits (the escrow stores none) |
| `refundability` | After a successful entry, the exit flow succeeds and returns the deposit minus the fees the config predicts | Funds locked in custody; payouts short by more than fees explain |
| `closeability` | After a successful exit, every `closable_accounts` label no longer exists | Vaults left open (withheld fees, dust, rent) |

Expected balances are cumulative over the flow steps that succeeded, plus the failed flow step when
an earlier step succeeded. A scenario whose first step failed was cleanly rejected and expects no
change, whether it is a round trip or a swap. A failed exit after a successful entry shows exactly
what stayed stuck. Skipped steps never count. (The first draft counted skipped steps too; that made
a rejected `make` in `make_take` a failure, because the take's deltas did not cancel the make's.)

`report::classify(steps, invariants, policy)`:

1. Any violated invariant: `fail`.
2. A flow step failed and no transaction of the run was committed (steps without transactions,
   such as a clock step, do not count): `rejected`, or `fail` under `must_support`.
3. Any other failed step: `fail`. Something failed after state had changed.
4. Every step succeeded: `pass`, or `fail` under `must_reject`.

`error` is set by the runner for harness failures.

## 9. Broken-assumption catalog

`crates/extensionlab-core/src/diagnosis.rs`. First match wins. A rule needs its scope (an
extension kind, or `token_2022` for program-level rules) to be present in some slot.

| Rule | Evidence | Evidence checked against real output? |
|---|---|---|
| `token_2022.ata_constraint_without_program` | log contains `ConstraintAssociated` | Yes, escrow refund logs in research/01. Anchor behavior confirmed in `anchor-syn-1.1.2/src/codegen/accounts/constraints.rs` |
| `transfer_fee.close_with_withheld` | log contains `withheld fee balance is zero` | Yes: harness test, and the escrow take logs in research/01 |
| `freeze_authority.custody_frozen` | scenario froze custody (`FreezeCustody`) and a log contains `Account is frozen` | Yes, golden test (escrow take after a freeze, 22 Oct) |
| `default_frozen.new_accounts_usable` | log contains `Account is frozen` | Harness test (top-level `MintTo`) |
| `memo_required.no_memo_needed` | log contains `No memo in previous instruction` | Harness test (top-level transfer) |
| `non_transferable.transferable` | log contains `Transfer is disabled for this mint` | Harness test (top-level transfer) |
| `cpi_guard.user_signed_cpi` | log contains `CPI Guard is enabled, and a program attempted to transfer user funds` (the program logs "... without using a delegate"; the `spl-token-2022-interface` 2.1.0 string adds "via CPI") | Yes, golden test (escrow `make` with a guarded maker account, `take` with a guarded taker account, 22 Oct). A top-level transfer from a guarded account works (harness test) |
| `pausable.paused_mint` | scenario paused a mint (`pause` or `pause_all`) and a log contains `paused on this mint` | Yes, golden test (escrow `take` after a pause, 22 Oct); harness test (top-level transfer) |
| `permanent_delegate.custody_exclusive` | scenario drained custody and refundability, credits, expected balances or conservation is violated | No |
| `transfer_fee.received_equals_sent` | credits, expected balances or refundability violated | No |
| `transfer_hook.plain_cpi` | any failed transaction | No |

The diagnosis is labeled heuristic in every report.

## 10. Report

`crates/extensionlab-core/src/report.rs`, schema version `0.1.0`. `extensionlab run` writes
`report.json` (serde JSON of `Report`) and `report.md`.

### Data model

```
Report
├─ schema_version, tool {name, version}
├─ environment {litesvm, programs[] {name, program_id, source, sha256?}}
├─ adapter {name, description}
├─ matrix {name, strategy, cases}
├─ summary {total, pass, rejected, fail, error}
└─ runs[] RunResult
   ├─ id, slots {slot -> MintConfig}, varied_slot?, baseline, scenario {name, steps[], requires_extension?}
   ├─ status: pass | rejected | fail | error
   ├─ steps[] {index, step, outcome: succeeded | failed | skipped,
   │           transactions[] {signature, compute_units, fee_lamports, error?, logs[]}}
   ├─ balances[] {label, address, slot, before, expected_after?, actual_after, withheld_after, exists_after}
   ├─ invariants[] {invariant, slot?, status: held | violated | skipped, detail}
   ├─ diagnosis? {rule, scope, assumption, evidence}
   └─ harness_error?
```

Addresses are base58 strings. `signature` is the transaction's first signature inside the LiteSVM
run. It identifies the transaction in the report only: it was never sent to a cluster and cannot
be looked up on an explorer. `Report::from_json` rejects a different major schema version.

### Example

`crates/extensionlab-core/tests/fixtures/example_report.json` is a **hand-written illustrative
example**, not runner output (it predates the runner). Runs 2 and 3 copy the program logs
and compute units observed in research/01 (deposit 10,000,000, receive 7,000,000, 200 bps fee with
no cap). Run 1 is the expected baseline outcome. Run 4 is hypothetical: it was never run against
the escrow, and its single log line is the text Token-2022 10.0.0 printed in the harness test for
a direct transfer. Signatures are placeholders; compute units of 0 mean not measured. The tests
parse this file, check that its summary, statuses and diagnoses are what `Summary::of`,
`classify` and `diagnose` compute, and render it, so this document and the types cannot drift
apart silently.

One run, shortened here: 6 of its 57 log lines, 3 of its 5 balance rows, and one of the two
conservation checks. The fixture has the full run.

```json
{
  "id": "transfer_fee_200bps@mint_a/make_take",
  "slots": {
    "mint_a": { "label": "transfer_fee_200bps", "program": "token_2022", "decimals": 6,
                "extensions": [ { "kind": "transfer_fee", "basis_points": 200, "maximum_fee": 18446744073709551615 } ] },
    "mint_b": { "label": "token_2022", "program": "token_2022", "decimals": 6 }
  },
  "varied_slot": "mint_a",
  "baseline": false,
  "scenario": { "name": "make_take", "steps": [ { "type": "flow", "name": "make" }, { "type": "flow", "name": "take" } ] },
  "status": "fail",
  "steps": [
    { "index": 0, "step": { "type": "flow", "name": "make" }, "outcome": "succeeded",
      "transactions": [ { "signature": "EXAMPLE-sig-5", "compute_units": 48444, "fee_lamports": 5000, "logs": [] } ] },
    { "index": 1, "step": { "type": "flow", "name": "take" }, "outcome": "failed",
      "transactions": [ { "signature": "EXAMPLE-sig-6", "compute_units": 72979, "fee_lamports": 5000,
        "error": "InstructionError(0, Custom(35))",
        "logs": [
          "Program 9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t invoke [1]",
          "Program log: Instruction: Take",
          "Program log: Instruction: CloseAccount",
          "Program log: Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again",
          "Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb failed: custom program error: 0x23",
          "Program 9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t failed: custom program error: 0x23"
        ] } ] }
  ],
  "balances": [
    { "label": "maker_ata_a", "address": "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi", "slot": "mint_a",
      "before": 100000000, "expected_after": 90000000, "actual_after": 90000000, "withheld_after": 0, "exists_after": true },
    { "label": "taker_ata_a", "address": "GgBaCs3NCBuZN12kCJgAW63ydqohFkHEdfdEXBPzLHq", "slot": "mint_a",
      "before": 0, "expected_after": 10000000, "actual_after": 0, "withheld_after": 0, "exists_after": false },
    { "label": "vault", "address": "8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR", "slot": "mint_a",
      "before": 0, "expected_after": 0, "actual_after": 9800000, "withheld_after": 200000, "exists_after": true }
  ],
  "invariants": [
    { "invariant": "conservation", "slot": "mint_a", "status": "held",
      "detail": "tracked accounts changed by 0, mint withheld pool by 0, supply by 0" },
    { "invariant": "expected_balances", "status": "violated",
      "detail": "maker_ata_b: expected 7000000, actual 0 (short 7000000); taker_ata_a: expected 10000000, actual 0 (short 10000000); taker_ata_b: expected 93000000, actual 100000000 (over 7000000); vault: expected 0, actual 9800000 (over 9800000)" },
    { "invariant": "credited_equals_received", "status": "skipped", "detail": "adapter exposes no credit readers" },
    { "invariant": "refundability", "status": "violated",
      "detail": "exit flow `take` failed after a successful entry; 10000000 base units stay in custody: InstructionError(0, Custom(35))" },
    { "invariant": "closeability", "status": "skipped", "detail": "exit flow did not succeed" }
  ],
  "diagnosis": {
    "rule": "transfer_fee.close_with_withheld",
    "scope": "transfer_fee",
    "assumption": "A custody token account can be closed once its `amount` has been transferred out. With TransferFee, fees withheld in the account block CloseAccount until someone harvests them to the mint.",
    "evidence": "Program log: Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again"
  }
}
```

Markdown (`extensionlab report render report.json`, first lines of the actual output for the
fixture):

```markdown
# ExtensionLab report: `escrow` x `default`

extensionlab 0.1.0 · LiteSVM 0.10.0 · report schema 0.1.0

| Total | Pass | Rejected | Fail | Error |
|---|---|---|---|---|
| 4 | 1 | 1 | 2 | 0 |

## Runs

| Run | Status | Broken invariants | Broken assumption |
|---|---|---|---|
| `spl_token/make_refund` | pass |  |  |
| `token_2022/make_refund` | FAIL | expected_balances, refundability | An Anchor `associated_token` constraint without ... |
| `transfer_fee_200bps@mint_a/make_take` | FAIL | expected_balances, refundability | A custody token account can be closed once ... |
| `non_transferable@mint_a/make_take` | rejected |  | Every mint can be transferred into custody. ... |
```

After the table, each `fail` or `error` run gets a section with its mints, diagnosis, steps, the
logs of the first failing transaction in a collapsed block, the balances table and the invariants
table. The output is meant for `$GITHUB_STEP_SUMMARY`.

## 11. CI

### Exit codes

| Code | Meaning | Source |
|---|---|---|
| 0 | No failures (rejections allowed under `--fail-on fail`) | `report::EXIT_OK` |
| 1 | At least one `fail` (or `rejected` under `--fail-on rejected`) | `EXIT_FAILURES` |
| 2 | Command-line usage error | clap's own usage-error code |
| 3 | Harness error: unknown adapter or matrix, unreadable report, or an `error` run | `EXIT_HARNESS` |
| 4 | A baseline case (all slots extension-free SPL Token) failed. The adapter or the protocol is broken before Token-2022 or any extension is involved, so the other results mean little | `EXIT_BASELINE` |

Precedence: 4, then 3, then 1, then 0. The example report exits 1: the escrow's `refund` fails for
plain Token-2022 (research/01, F1), but plain Token-2022 is not baseline, so that is an ordinary
failure. Until F1 is fixed, every Token-2022 refund case fails for the same reason, and the
diagnosis says so. `extensionlab report exit-code report.json`
prints and returns the code for an existing report.

### GitHub Action sketch (unverified; not committed as a workflow)

For a protocol repository with an `extensionlab` binary crate that registers its adapter:

```yaml
name: token-compat
on: [push, pull_request]
jobs:
  extensionlab:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install Solana CLI (provides cargo build-sbf)
        run: |
          sh -c "$(curl -sSfL https://release.anza.xyz/v3.1.10/install)"
          echo "$HOME/.local/share/solana/install/active_release/bin" >> "$GITHUB_PATH"
      - name: Build the program
        run: cargo build-sbf --manifest-path programs/escrowq32026/Cargo.toml
      - name: Run the matrix
        run: cargo run -p escrow-extensionlab -- run --adapter escrow --matrix default --out extensionlab-report
      - name: Job summary
        if: always()
        run: cat extensionlab-report/report.md >> "$GITHUB_STEP_SUMMARY"
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: extensionlab-report
          path: extensionlab-report/
```

The install command follows the Anza docs, which take a release tag in the URL [S4]. The
`$GITHUB_PATH` line assumes the installer's default location, which those docs do not state;
unverified. Markdown appended to `$GITHUB_STEP_SUMMARY` appears on the run summary page [S5].
The run step's exit code fails the job; the summary and upload steps still run (`if: always()`).
Later this can become a composite action published from this repository.

## 12. Regression test stubs

`extensionlab regress report.json --run <id> [--expect pass|observed] [--adapter-expr <expr>] [--out file]`
writes a Rust integration test that:

- embeds the exact `Case` (slot configs and scenario steps) as JSON;
- constructs the adapter from `--adapter-expr`, or a `todo!()` that names the adapter;
- runs `Runner::default().run_case(...)` and asserts `RunStatus::Pass` (`--expect pass`: red until
  fixed) or the observed status (`--expect observed`: pins a known incompatibility).

The test name is `extensionlab_<adapter>_<run id>` with non-alphanumerics collapsed to `_`. The
generated file needs `extensionlab-core` and `serde_json` as dev-dependencies. Without
`--adapter-expr` the test compiles and then fails at the `todo!()`.

## 13. Repository layout and dependency pins

```
extensionlab/
├─ Cargo.toml                 workspace: members crates/*; exclude reference, targets
├─ Cargo.lock                 seeded from the escrow's lockfile, so shared crates resolve to the same versions
├─ crates/
│  ├─ extensionlab-core/
│  │  ├─ src/{lib,adapter,mint,matrix,invariants,diagnosis,report,regression,runner,error}.rs
│  │  └─ tests/{mint_factory.rs, fixtures/example_report.json}
│  └─ extensionlab-cli/       binary `extensionlab`, lib `extensionlab_cli` (Registry, main_with)
├─ research/                  write-ups (this file)
├─ targets/                   protocols under test (separate Anchor workspaces; not members)
└─ reference/                 read-only copies of the owner's projects (git-ignored; not members)
```

Planned: `adapters/escrow` (the adapter in section 4, as a workspace member that loads
`targets/escrow/target/deploy/escrowq32026.so` and skips with a clear message when it is
missing), and `hooks/` for transfer-hook test programs.

Direct dependencies and the versions Cargo resolved: `litesvm` 0.10.0, `solana-instruction` 3.5.0,
`solana-keypair` 3.1.2, `solana-message` 3.1.0, `solana-pubkey` 3.0.0, `solana-signer` 3.0.1,
`solana-transaction` 3.1.0, `solana-system-interface` 2.0.0 (feature `bincode`),
`spl-token-2022-interface` 2.1.0, `spl-associated-token-account-interface` 2.0.0, `serde`
1.0.229, `serde_json` 1.0.151, `thiserror` 2.0.20, `clap` 4.6.7. These match the escrow's
`Cargo.lock` where both have them. `solana-pubkey` 3.0.0 and 4.3.0 both re-export
`solana_address::Address` 2.7.0, which is also what litesvm 0.10.0 uses, so there is one address
type across the build. `spl-token-2022-interface` builds base instructions for both token
programs (`check_spl_token_program_account` accepts both ids), so no separate `spl-token`
interface crate is needed. No `rust-version` is declared: only the host toolchain 1.98.1 was used.

## 14. MVP scope

### Weekend

Goal: `extensionlab run --adapter escrow --matrix default` reproduces research/01 F1 and F2 by
itself. (It exited 4 until the baseline gate became SPL Token only; it now exits 1.)

1. `Runner::run_case`: lifecycle steps 1 to 8 for flow steps and `drain_custody`; seeded keypairs;
   token-account snapshots; per-case harness errors as `error` runs.
2. Escrow adapter (`adapters/escrow`) with make, take and refund, built from raw instruction bytes.
3. `default` matrix, JSON and Markdown reports, exit codes, `regress`. All exist.
4. A golden test: run the escrow matrix and compare statuses and diagnoses with a checked-in
   expected list (only the observed F1 and F2 rows at first).

Out of scope for the weekend: TransferHook, credits, issuer fee changes and clock warps, the
GitHub Action, a second adapter.

### Four weeks

- **Week 1.** Harden the weekend runner. `advance_clock`, `harvest_withheld`, `freeze_custody`, and
  a fee change (`SetTransferFee` followed by a 2-epoch warp, since a new fee takes effect two
  epochs later [S2]). Adapter-defined scenarios for the escrow (expiry, fee change mid-flight).
  Fork after the entry flow (`LiteSVM: Clone`) to try every exit from one state.
- **Week 2.** Transfer-hook programs in `hooks/`: noop, always-fail, and one that needs an extra
  account. Built with `cargo build-sbf`, checked in with SHA-256 and a rebuild script, recorded in
  the report's `environment.programs`. Matrix `all` switches from the undeployed hook to these.
- **Week 3.** Two more adapters from the owner's projects (`vault_q326`, and `amm_q3_26` or the
  `nft-staking` program) to test whether the trait fits more than an escrow. Credits for
  ledger-based protocols. Closeability for non-token accounts. Mixed-program cases.
- **Week 4.** A composite GitHub Action and a sample workflow, run once for real. README and
  adapter-author guide. A decision on publishing to crates.io (the owner's call). Optionally check
  one finding against devnet's Token-2022.

## 15. What was verified

All commands ran from the repository root with `nice -n 15` and `CARGO_BUILD_JOBS=4`,
on the host only. No `cargo build-sbf`, no anchor CLI, no toolchain changes.

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | passes, no warnings |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo test --workspace` | 72 tests pass (22 Oct, after tasks 5 and 6): 26 unit tests in `extensionlab-core`, 14 in `tests/mint_factory.rs`, 1 in `tests/runner_steps.rs`, 19 in `tests/extension_probe.rs`, 2 in `extensionlab-cli`, 10 escrow golden tests in `adapters/escrow/tests/golden.rs` (21 Oct: 37, before the runner) |
| Mint factory against LiteSVM 0.10.0's bundled programs | every `all`-preset config creates a mint whose extension set and size match the config; actor ATAs are created, thawed or memo-enabled, funded and read back for every `default` config |
| Token-2022 10.0.0 behavior, top-level instructions only | fee withheld in destination (10,000 at 250 bps gives 9,750 plus 250 withheld); close with withheld fees fails with error 35 and the log line in section 9; `MintTo` into a default-frozen account fails with error 17; transfer into a memo-required account fails with error 36; transfer of a NonTransferable mint fails with error 37 |
| Example report | parses; summary, statuses and diagnoses match `Summary::of`, `classify` and `diagnose`; round-trips through JSON; renders to Markdown |
| CLI smoke run | `matrix list` and `matrix show` work; `run --adapter escrow` exits 3 in the adapter-free `extensionlab` binary, and runs the matrix in `extensionlab-escrow` (22 Oct: 32 runs, exit 1 after the SPL-only baseline gate); `run` with no arguments exits 2; `report exit-code` on the example prints and returns 1; `report render` and `regress` produce output |
| Generated regression stub | compiles in a scratch crate against `extensionlab-core` without warnings and fails at the `todo!()` as intended; with `--adapter-expr 'extensionlab_adapter_escrow::boxed()' --expect observed` it passes against the escrow (22 Oct) |
| Anchor discriminator and ATA derivation | read in `anchor-syn-1.1.2` sources (section 2, rule table) |

Verified on 22 Oct: reports are byte-identical for the same seed (golden test), and the runner's
results on the escrow match research/01 (F1 2009, F2 0x23, vault 9,800,000 + 200,000 withheld, taker
9,604,000 after a harvest).

Not verified: any extension behavior inside a protocol's CPI beyond the escrow; TransferHook
transfer behavior with a deployed hook; the GitHub Action; building on any Rust version other than
1.98.1.

## 16. Open questions

1. **Adapter distribution.** Compiled Rust adapters (current design) versus a declarative adapter
   driven by an Anchor IDL plus a small JSON of flows and expectations. The IDL route would cover
   Anchor programs without Rust code, but the expectations still need a human.
2. **Which Token-2022.** LiteSVM 0.10.0 bundles Token-2022 10.0.0. Is that what mainnet runs? An
   option to load a dumped mainnet binary would make results closer to production; the report
   already records each program's source.
3. **Hook programs in a public repo.** Commit `.so` files with hashes, or build them in CI? Committed
   binaries are convenient but opaque.
4. **Fee-explained mismatches.** Today a balance that differs only by the configured fee is a
   `fail` under nominal expectations. Should there be a softer status, or should adapters be
   expected to write fee-aware expectations?
5. **Default support policy.** Should some configs default to `must_reject` (permanent delegate,
   non-transferable), or should every policy be stated by the author?
6. **Baseline gate scope.** Decided 2026-10-22: SPL Token only. A protocol that fails with plain
   Token-2022 now exits 1 with a program-scope diagnosis, not 4.
7. **Non-token state.** Closeability and rent checks for program accounts such as the escrow PDA.
   Snapshots cover token accounts only.
8. **Mixed programs.** A strategy for slots on different token programs (research/01 F4).
9. **Determinism.** Seeded keypairs are planned. Whether LiteSVM's blockhash and slot are
   deterministic enough for byte-identical reports has not been checked.
10. **One issuer or several.** Splitting authorities would allow cases like "fee authority is not
    the mint authority". Does that find anything a single issuer misses?
11. **State injection.** LiteSVM's `set_account` could put protocol-owned accounts into states the
    harness cannot reach with instructions (for example memo-required on a PDA vault). Allow only
    reachable states, or allow injection with a label in the report?
12. **License and name.** The public repository needs a license (not added) and a check that the
    name is free.
13. **MSRV.** The owner's projects declare Rust 1.89.0. This workspace was only built with 1.98.1.

## Sources

- [S1] Token-2022 extensions guide (transfer fees and closing with withheld fees, default account
  state, permanent delegate, required memo including CPI, non-transferable):
  https://www.solana-program.com/docs/token-2022/extensions
- [S2] Transfer fees (fee withheld in the destination account, permissionless harvest,
  `SetTransferFee` takes effect two epochs later): https://solana.com/docs/tokens/extensions/transfer-fees
- [S3] Transfer hook (the transfer carries the ExtraAccountMetas account and the hook program; PDA
  seeds): https://solana.com/docs/tokens/extensions/transfer-hook
- [S4] Agave CLI install command with a release tag: https://docs.anza.xyz/cli/install
- [S5] Job summaries via `$GITHUB_STEP_SUMMARY`:
  https://github.blog/news-insights/product-news/supercharging-github-actions-with-job-summaries/
- LiteSVM: https://github.com/LiteSVM/litesvm. Version-specific facts (bundled programs, `Clone`,
  `TransactionMetadata` fields) were read in the local registry copy of `litesvm-0.10.0`.
- Token-2022 error codes and messages: local `spl-token-2022-interface-2.1.0/src/error.rs`, and the
  logs printed by the harness tests.
- Anchor: local `anchor-syn-1.1.2/src/lib.rs` (custom discriminators) and
  `src/codegen/accounts/constraints.rs` (associated-token address derivation).
- Escrow reproduction: `research/01_repro_escrow_fee_token.md` and
  `research/logs/01_token2022_fees.log` (separate task).
