# 01 · Reproduction: escrow_q326 against Token-2022 (TransferFeeConfig)

Date: 2026-10-21. Target: `targets/escrow`, an unmodified copy of
`reference/turbin3_q3_26/vault_and_escrow_q326/escrow_q326`. The program was not changed. Only a
test file was added: `targets/escrow/programs/escrowq32026/tests/token2022_fees.rs`.

## Summary

The program's README says it "is not tied to one token program" because it uses
`transfer_checked` through `TokenInterface`. That holds for SPL Token. It does not hold for
Token-2022. Four findings, all observed in LiteSVM with the Token-2022 program that LiteSVM ships:

| # | Finding | Mints | Observed result | Severity (my judgment) |
|---|---|---|---|---|
| F1 | `refund` never works for a Token-2022 mint, fee or no fee. After expiry the deposit is locked for good: take, update and refund all fail. | any Token-2022 mint_a | `ConstraintAssociated` (Anchor 2009) on `maker_ata_a` | High (permanent lock after expiry) |
| F2 | `take` always fails when mint_a has a transfer fee. Both transfers go through, then closing the vault fails because the fee withheld during `make` is still sitting in the vault. | fee-bearing mint_a | Token-2022 `AccountHasWithheldTransferFees` (35 = 0x23) | Medium (blocked until anyone harvests; harvest is permissionless) |
| F3 | The amounts are silently wrong. The escrow never records the deposit. The taker gets 96.04% of the nominal deposit, because the fee is taken twice on the A leg. The maker gets 98% of `receive`. `max_receive` limits only the gross amount. If the issuer changes the fee after `make`, the maker gets even less and nothing in the program catches it. | fee-bearing mint_a or mint_b | numbers below | Medium/Low (economic) |
| F4 | One escrow cannot hold an SPL Token mint and a Token-2022 mint together. | mixed | `ConstraintMintTokenProgram` (2022) or `IncorrectProgramId` | Informational |

The task asked whether Token-2022 refuses to close an account that holds withheld transfer fees.
The logs confirm it does, both through the escrow's `take` and when calling Token-2022 directly.
The exact line is
`Program log: Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again`,
followed by `custom program error: 0x23`.

For `refund` the same close failure could not be reached with the program as written, because F1
rejects the call first. I confirmed it in a separate patched copy (fix A below,
`research/fixcheck/`): once the refund ATA constraint is fixed, refund fails at `CloseAccount` with
0x23, exactly like take.

## Setup

- Toolchain: solana-cli 3.1.10 and `cargo-build-sbf` 3.1.10 (platform-tools v1.52, rustc 1.89.0
  for SBF). Host cargo/rustc 1.98.1. The global toolchain was not changed on purpose, but see
  "Side effects" below.
- Program: Anchor `anchor-lang` / `anchor-spl` 1.1.2, built with `nice -n 15 cargo build-sbf`
  (`CARGO_BUILD_JOBS=4`).
- Test runtime: `litesvm` 0.10.0 and `litesvm-token` 0.10.0, unchanged from the original
  dev-dependencies. LiteSVM 0.10.0 bundles Token-2022 as `spl_token_2022-10.0.0.so`
  (`litesvm-0.10.0/src/programs/mod.rs:19`, read in the local cargo registry).
- Instruction builders: `spl-token-2022-interface` 2.1.0. It was already in `Cargo.lock` through
  anchor-spl and is used through anchor-spl's re-export `anchor_spl::token_2022::spl_token_2022`.
  **No dependency was added. `Cargo.toml` and `Cargo.lock` are byte-identical to the reference**
  (checked with `diff -rq`; the only difference is the new test file).
  - Why `litesvm-token`'s `token-2022` feature was not used: turning it on swaps
    `litesvm_token::spl_token` (and `TOKEN_ID`) from `spl_token_interface` to
    `spl_token_2022_interface` (`litesvm-token-0.10.0/src/lib.rs`). Cargo applies features to the
    whole test build, so the existing `tests/mod.rs` would quietly start running against Token-2022
    while still calling it `TOKEN_PROGRAM_ID`. The new file builds Token-2022 mints and ATAs with its
    own small helpers instead.
- Baseline: `cargo test` on the untouched copy passes 4/4 (`test_id`, `test_make_and_refund`,
  `test_make_and_take`, `test_take_creates_missing_atas`). Log:
  `research/logs/00_baseline_cargo_test.log`.
- Fee parameters: `TransferFeeConfig` at 200 bps (2%) with `maximum_fee = u64::MAX`, so the cap
  never applies. 6 decimals. The fee authority and the withdraw-withheld authority are an "issuer"
  key that is neither the maker nor the taker.
- Amounts (base units): starting balance 100,000,000; `deposit` 10,000,000; `receive` 7,000,000.
- The fee formula is checked independently in the test: `fee = min(ceil(amount * bps / 10_000), max)`.

## Results (exact numbers, all asserted in `token2022_fees.rs`)

### (a) make with fee-bearing mint_a: `a_fee_mint_a_make_vault_holds_less_than_deposit`

| Quantity | Value |
|---|---|
| Maker debited | 10,000,000 |
| Vault `amount` | **9,800,000** |
| Vault `withheld_amount` (TransferFeeAmount) | **200,000** |
| Escrow state | `seed, maker, mint_a, mint_b, receive = 7,000,000, bump, expiration`. **No deposit field.** |
| make CUs | 48,444 |

`make` succeeds. The escrow never records how much was deposited. `take` and `refund` later move
whatever `vault.amount` holds at that moment.

### (b) take

**Fee-bearing mint_a: `repro_b_fee_mint_a_take_fails_closing_vault_with_withheld_fees`**

The transaction fails with `InstructionError(0, Custom(35))`. Both `TransferChecked` CPIs succeed,
then `CloseAccount` on the vault fails. The whole transaction is reverted: the vault still holds
9,800,000 plus 200,000 withheld, the taker's B balance is unchanged, and neither ATA that
`init_if_needed` would have created exists.

```
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb invoke [2]
Program log: Instruction: TransferChecked            <- taker_ata_b -> maker_ata_b
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb success
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb invoke [2]
Program log: Instruction: TransferChecked            <- vault -> taker_ata_a (9,800,000)
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb success
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb invoke [2]
Program log: Instruction: CloseAccount
Program log: Error: An account can only be closed if its withheld fee balance is zero, harvest fees to the mint and try again
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb consumed 1772 of 124293 compute units
Program TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb failed: custom program error: 0x23
Program 9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t consumed 77479 of 200000 compute units
Program 9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t failed: custom program error: 0x23
```

The same happens when both mints carry a fee (`repro_b_fee_both_mints_take_fails_closing_vault`).

**Fee-bearing mint_a after a permissionless harvest:
`b_fee_mint_a_take_succeeds_after_permissionless_harvest` and `..._with_harvest_prepended_in_same_tx`**

A stranger with no authority over the mint or the vault sends `HarvestWithheldTokensToMint(mint_a, [vault])`.
The vault's withheld amount drops to 0 and the mint's withheld amount becomes 200,000. After that,
`take` succeeds. Putting the harvest instruction in the same transaction as the take also works.

| Quantity | Value |
|---|---|
| Taker receives (amount) | **9,604,000** of a nominal 10,000,000 (96.04%) |
| Withheld in taker_ata_a | 196,000 (2% of 9,800,000) |
| Total lost to fees on the A leg | 396,000 (two fees: maker→vault, vault→taker) |
| Maker receives of mint_b (fee-free) | 7,000,000 |
| Vault / escrow | closed |
| take CUs (harvest in a separate tx) | 82,747 |

**Fee-bearing mint_b: `b_fee_mint_b_take_maker_receives_less_than_receive`**

Take succeeds. The fee is withheld in `maker_ata_b`, not in the vault, so the vault closes normally.

| Quantity | Value |
|---|---|
| Taker debited of mint_b | 7,000,000 |
| Maker credited of mint_b | **6,860,000** (+140,000 withheld in maker_ata_b) |
| Taker receives of mint_a (fee-free) | 10,000,000 |

### (c) refund: `repro_refund_rejected_for_any_token2022_mint`, `repro_c_fee_mint_a_refund_rejected_before_close`, `repro_expired_token2022_escrow_is_locked_forever`

Refund fails before its handler runs, for plain Token-2022 mints and fee-bearing ones alike, and
both before and after a harvest:

```
Program log: Instruction: Refund
Program log: AnchorError caused by account: maker_ata_a. Error Code: ConstraintAssociated. Error Number: 2009. Error Message: An associated constraint was violated.
Program log: Left:
Program log: 6LT5kEjun13ABtcS71HfinFXc5mUq5PAqc1PPrdnCmFn      <- the Token-2022 ATA that was passed
Program log: Right:
Program log: FaumBmq6HStBFYqqvQPkM2e1tCarNALQaHY25WBWBiUP      <- the address Anchor expected (derived with legacy SPL Token)
```

Cause: in `refund.rs`, `maker_ata_a` and `vault` declare `associated_token::mint` and
`associated_token::authority` but not `associated_token::token_program`. In Anchor 1.1.2, if that
field is missing, the expected address comes from `get_associated_token_address(wallet, mint)`,
which uses the legacy SPL Token program id (`anchor-syn-1.1.2/src/codegen/accounts/constraints.rs`,
lines 1299–1305, read in the local registry). A Token-2022 ATA is derived with the Token-2022
program id, so the addresses can never match. `make` and `take` set
`associated_token::token_program = token_program` and so are not affected.

Result after expiry: `take` returns `EscrowExpired` (6000, `take.rs:79`), `update(expiration)`
returns `EscrowExpired` (6000, `update.rs:38`), and `refund` returns 2009. No instruction can move
the deposit, so it is locked permanently. Before expiry the only way out is a take by a different
wallet. A self-take (taker = maker) is rejected with
`ConstraintDuplicateMutableAccount` (2040) on `maker_ata_b`
(`limit_maker_cannot_self_take_with_one_wallet`).

Close-with-withheld-fees in refund, observed in the patched copy (`research/fixcheck`, fix A only):
refund with a fee-bearing mint_a fails at `CloseAccount` with 0x23 and the same log line. If the
client harvests first, the refund succeeds and the maker ends up with 99,604,000. **A deal that
never happened cost the maker 396,000 (3.96%).**

### (d) update / max_receive with a fee-bearing mint_b

- `d_fee_mint_b_max_receive_bounds_gross_not_net`: a taker who sets `max_receive` to the net price
  (6,860,000) is rejected with `ReceiveAboveMax` (6004, `take.rs:84`). The check compares against
  the gross `receive`. After the maker lowers `receive` to 5,000,000 with `update`, the take
  succeeds, the taker pays 5,000,000 and the maker nets **4,900,000**.
- `d_fee_mint_b_gross_up_receive_gives_maker_exact_net`: the maker can net exactly 7,000,000 only
  by choosing `receive = 7,142,858` at make time. The fee is ceil(142,857.16) = 142,858. Because
  `update` can only lower `receive`, the maker cannot fix this later.
- `d_fee_mint_b_issuer_fee_change_after_make_cuts_maker_net`: after `make`, the issuer raises the
  fee to 500 bps with `SetTransferFee` and the test moves the clock forward 2 epochs. The take still
  passes with the original `max_receive`. The taker pays 7,000,000 and the maker nets
  **6,650,000** (350,000 withheld). Nothing in the escrow notices. `max_receive` protects the
  taker's gross amount. No equivalent protects the maker's net amount.

### Token-2022 rule on its own: `rule_token2022_refuses_to_close_account_with_withheld_fees`

This test does not involve the escrow. Alice sends 10,000,000 to Bob. Bob is credited 9,800,000
and 200,000 is withheld in his account. Bob sends the 9,800,000 back, which leaves him with
amount 0 and 200,000 withheld. `CloseAccount` on Bob's account then fails with 0x23 and the same
log line. A stranger harvests, and the close succeeds.

### F4, mixed programs: `limit_mixed_token_programs_rejected_at_make`

`make` binds both mints to one `token_program`. Legacy mint_a with Token-2022 mint_b:
- With `token_program` set to SPL Token, the vault `init` runs first and succeeds, then the check on
  mint_b fails: `Error Code: ConstraintMintTokenProgram. Error Number: 2022`. Anchor does not print
  the account name for this one.
- With `token_program` set to Token-2022, the vault `init` CPI fails first:
  `Program log: Error: IncorrectProgramId` from Token-2022 `GetAccountDataSize`, which surfaces as
  `InstructionError(0, IncorrectProgramId)`.

## Broken assumptions

1. **"Using TokenInterface makes the program Token-2022 compatible."** Every constraint also has
   to name the token program. One missing `associated_token::token_program` (refund) silently
   limits that instruction to legacy SPL Token, and nothing fails at compile time.
2. **"Amount sent = amount received."** With TransferFee the receiver is credited
   `amount - fee`. The vault holds less than `deposit`, the taker receives less than the vault
   held, and the maker receives less than `receive`.
3. **"A token account with amount 0 can be closed."** With TransferFee, the account must also
   hold no withheld fees. The fee is withheld in the destination account, which here is the vault.
4. **"The price is fixed when the deal is made."** The fee is a parameter of the mint, controlled
   by its issuer. It can change between `make` and `take` (it takes effect 2 epochs later), so the
   maker's net amount is not fixed.
5. **"If take is impossible, refund still works."** For Token-2022 both exits were broken at the
   same time. Only an outside harvest unblocks `take`, and nothing unblocks anything once the
   escrow has expired.

## Candidate fixes and tradeoffs

| Fix | What | Verified? | Tradeoffs |
|---|---|---|---|
| **A** Refund ATA derivation | Add `associated_token::token_program = token_program` to `maker_ata_a` and `vault` in `Refund`, and `mint::token_program = token_program` to `mint_a`. | **Yes**, in a patched copy (`research/fixcheck/fix_A_B.diff`). Plain Token-2022 refund and refund after expiry succeed. The three refund `repro_` tests turn red as expected. The legacy tests in `tests/mod.rs` still pass. | None worth noting. Must ship regardless of fee policy, because it fixes F1 for every Token-2022 mint. |
| **B** Harvest before close | Inside take/refund, after moving the tokens out: if `token_program` is Token-2022 and the vault's `TransferFeeAmount.withheld_amount > 0`, CPI `HarvestWithheldTokensToMint(mint_a, [vault])` and then close. This needs no signer. | **Yes** (feature `fix-b` in the patched copy). Take and refund with a fee-bearing mint_a succeed without a client-side harvest: the taker gets 9,604,000, the maker on refund gets 99,604,000. Plain mints skip the CPI. take CUs went from 82,747 (harvest in a separate tx) to 90,936. | `mint_a` must be passed **writable** in take/refund, so every take/refund for a popular mint write-locks that mint. That is the contention Token-2022 avoids by withholding fees at the destination. Adds about 8k CUs. Does not fix the amounts (F3). |
| **B′** Client-side harvest | Put `HarvestWithheldTokensToMint` before take/refund in the same transaction. | **Yes**, for take, in the unpatched program (`b_fee_mint_a_take_succeeds_with_harvest_prepended_in_same_tx`). | No redeploy, but every client and integrator has to know about it, and a naive client always fails. Does not help refund until fix A ships. |
| **C** Credit actual amounts | Measure what actually arrived (reload the vault after the deposit), store it in `Escrow` and emit it. Let the taker pass a `min_receive_a` (net) next to `max_receive`. Let the maker state `receive` as a net amount, have the program compute the gross from the current epoch fee, and transfer with `transfer_checked_with_fee`, which fails if the expected fee does not match. | **Unverified** (design only). | Changes the account layout (adds a field) and the instruction arguments, which breaks existing clients. Two fees on the A leg are still unavoidable as long as the vault holds custody. Pinning the fee turns the silent shortfall from a fee change into a clean failure, which is the point, but it can also block takes right after a fee change. |
| **D** Reject fee mints | In `make`, unpack `mint_a` and `mint_b` with `StateWithExtensions<Mint>` and refuse `TransferFeeConfig`, or keep an allowlist of extensions. | **Unverified** (design only). | Simplest and safest, but it excludes fee tokens. Other extensions (TransferHook, PermanentDelegate, DefaultAccountState, NonTransferable, MemoTransfer) need their own decision, so this turns into an extension policy, not a one-line check. |
| **E** Don't close a vault that still holds withheld fees | Skip `close_account` when `withheld_amount > 0`. | **Unverified**. | The vault's rent stays locked, and `make` with the same seed later fails because the vault ATA already exists (`init`). Not recommended. |

Suggested order: A (needed in any case) plus either D (if fee tokens are out of scope) or B + C
(if they are in scope).

## How this becomes ExtensionLab's demo

This reproduction already produces what ExtensionLab promises, for one real target:

1. **A controlled token configuration.** `create_mint_2022(MintKind::{Plain, Fee{bps,max}})`, an
   issuer key separate from the parties, plus "fee change mid-flight" (`SetTransferFee` plus an epoch
   warp) and "expired" (clock warp) as runtime perturbations.
2. **The developer's real flows.** make/take/refund/update built from the target's own Anchor
   `accounts::*` and `instruction::*` types, run on a deterministic LiteSVM.
3. **The failing transaction and its logs.** Every test prints the full program log. Failures are
   asserted by `(instruction index, error code)` plus a log needle.
4. **Expected vs actual balances.** `balance()` reads both `amount` and `withheld_amount`, so
   "where did the 2% go" has an answer in every table above.
5. **The broken assumption,** named in plain language (the list above).
6. **A regression test that runs in CI.** `cargo build-sbf && cargo test --test token2022_fees`.
   The `repro_` tests pass while the bug exists and fail once it is fixed. The patched-copy run
   shows this directly: with fix A, 3 `repro_` tests turn red; with fixes A+B, 5 do. The developer
   then swaps them for the post-fix assertions in `research/fixcheck/.../tests/fixcheck.rs`.

Proposed next steps (proposals, not built yet):
- Pull the harness (mint factory, ATA and balance helpers, `expect_custom_failure`, clock and epoch
  warps) out of the test file into a reusable crate, so a new target only writes its flow driver.
- Turn the configurations into a matrix: {Plain, TransferFee(bps, max), fee change mid-flight,
  mixed programs} × {make, take, refund, update}, and later TransferHook, PermanentDelegate,
  DefaultAccountState(Frozen), NonTransferable, MemoTransfer and CpiGuard.
- Produce a one-page report per run from the same assertions (the tables above are written by hand
  today).
- Demo script: "Here is a Turbin3 escrow whose SPL Token tests all pass. Run it against a 2%
  fee token: take reverts with 0x23, refund reverts with 2009, after expiry the funds are locked,
  and here is the 3-line fix (fix A) with the test that proves it."

## Reproduce

```
cd targets/escrow                                              # from the repository root
CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf
CARGO_BUILD_JOBS=4 nice -n 15 cargo test                       # 1 + 3 + 16 tests pass
CARGO_BUILD_JOBS=4 nice -n 15 cargo test --test token2022_fees -- --nocapture --test-threads=1   # with logs

# candidate fixes (throwaway copy, not part of the target)
cd ../../research/fixcheck/escrow_patched
CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf && CARGO_BUILD_JOBS=4 nice -n 15 cargo test --no-fail-fast                    # fix A only
CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf --features fix-b && CARGO_BUILD_JOBS=4 nice -n 15 cargo test --features fix-b --no-fail-fast   # fix A + B
```

Full logs: `research/logs/01_token2022_fees.log`. Patched-copy results:
`research/fixcheck/run_fix_A_only.txt` and `research/fixcheck/run_fix_A_B.txt`.

## Caveats and side effects

- **Toolchain side effect (unintended).** The first `cargo build-sbf` printed
  `info: uninstalling toolchain 1.95.0-sbpf-solana-v1.57` (`research/logs/00_build_sbf.log`).
  cargo-build-sbf 3.1.10 dropped the rustup link for platform-tools v1.57 and linked its default
  v1.52 (`1.89.0-sbpf-solana-v1.52`). The v1.57 files are still in `~/.cache/solana/v1.57`.
  Nothing else was changed. I did not restore the link, because that would itself be a toolchain
  change. My expectation is that a later `cargo build-sbf --tools-version v1.57` relinks it
  automatically, but I have not verified that.
- `cargo build-sbf` generated a new program keypair at `target/deploy/escrowq32026-keypair.json`
  (and a cloned copy under `research/fixcheck/escrow_patched/target/`). I have not read it. It is
  covered by the `target/` ignore rules. The tests load the `.so` under the declared id
  `9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t`, so the keypair plays no part.
- These results come from the Token-2022 build that LiteSVM bundles (10.0.0). I have not checked
  that mainnet or devnet run the same version or behave the same. A devnet run is the obvious next
  check.
- The severity labels are my own judgment, not a formal audit rating.
- `research/fixcheck/escrow_patched` is a throwaway verification copy, and its `target/` is about
  1.2 GB (an APFS clone). It can be deleted. The candidate fixes are also kept as
  `research/fixcheck/fix_A_B.diff`.

## Sources

- Token-2022 transfer fees: the fee is withheld in the recipient account, an account holding
  withheld tokens cannot be closed, `harvest_withheld_tokens_to_mint` is permissionless, and
  `transfer_checked` / `transfer_checked_with_fee` are required:
  https://www.solana-program.com/docs/token-2022/extensions#transfer-fees
- The withheld fee is tracked on the destination account, harvest is permissionless,
  `TransferChecked` withholds automatically, and `SetTransferFee` "takes effect starting two epochs
  later": https://solana.com/docs/tokens/extensions/transfer-fees
- `associated_token::token_program` is optional, and the default is not documented there:
  https://www.anchor-lang.com/docs/references/account-constraints. The legacy-program default was
  read in the local source `anchor-syn-1.1.2/src/codegen/accounts/constraints.rs:1299-1305`.
- Error numbers come from the logs above and from the local sources
  `spl-token-2022-interface-2.1.0/src/error.rs` and `anchor-lang-error-1.1.2/src/lib.rs`.
