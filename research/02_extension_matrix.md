# Token-2022 Extension Risk Matrix for Integrators

This matrix is for teams building escrows, payment flows, vaults and other protocols that take custody of user tokens or move them. For each Token-2022 extension it records:

- what the extension changes;
- which integration assumption it breaks, with the error you will see;
- how to build the extension in a LiteSVM test;
- how a protocol should handle it, and how to detect it on-chain;
- how severe it is for an escrow or payment protocol, and its priority for ExtensionLab's first scenario set.

A machine-readable copy is in [`02_extension_matrix.json`](02_extension_matrix.json), with one object per extension.

## How the claims were checked

Every behavioral claim carries one of these tags:

| Tag | Meaning |
|---|---|
| **[probe]** | Observed by running it. A host-side LiteSVM probe executed the transaction and printed the result, on 2026-10-21 (Token-2022 11.0.0). Re-run on 2026-10-22 on Token-2022 10.0.0; see [Re-run on LiteSVM 0.10.0](#re-run-on-litesvm-0100-token-2022-1000). |
| **[src]** | Read in the Token-2022 source but not executed. Upstream `main` at commit `3d94b2fa6bf4369f4eb98ea69c188726b05ad64c`, or `spl-token-2022-interface` 3.1.1. |
| **[doc]** | Stated in official documentation (solana-program.com or solana.com). Not executed. |
| **[secondary]** | Stated by a third-party security write-up. Not executed. |
| **[unverified]** | Our own inference or expectation. Not checked against code or docs. |

**Probe environment.** The values in this document come from the first run unless a note says "on 10.0.0".
- First run, 2026-10-21:
  - `litesvm` 0.16.0, whose `LiteSVM::new()` loads the bundled Token-2022 program ELF `spl_token_2022-11.0.0.so`. The version was read from `litesvm-0.16.0/src/programs/mod.rs`.
  - `spl-token-2022-interface` 3.1.1 built the instructions and unpacked state.
  - Two other programs that LiteSVM loads by default were also used: SPL Memo at `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr` (`spl_memo-4.0.0.so`), and the Associated Token Account program (`spl_associated_token_account-1.1.1.so`).
- Re-run, 2026-10-22, on the stack ExtensionLab itself uses:
  - `litesvm` 0.10.0, which loads `spl_token_2022-10.0.0.so`, SPL Memo 3.0.0 at `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr` and `spl_associated_token_account-1.1.1.so`. The versions were read from `litesvm-0.10.0/src/programs/mod.rs`.
  - `spl-token-2022-interface` 2.1.0 builds the instructions and unpacks state.

**Probe files**
- The probe is `crates/extensionlab-core/tests/extension_probe.rs`, with one test per extension. `cargo test -p extensionlab-core --test extension_probe -- --nocapture --test-threads=1` prints one row per check (claim id, what this document recorded, what the run observed). The test fails if Token-2022's behavior changes.
- Output of the 10.0.0 run: `research/logs/02_extension_probe_litesvm-0.10.0.log`.
- The first-run probe was a separate Cargo project in the gitignored `target/ext-probe/`. Its source and raw output are kept as `research/logs/02_probe_litesvm-0.16.0_main.rs.txt` and `research/logs/02_probe_litesvm-0.16.0_out.txt`. Its dependencies were pinned to `litesvm` 0.16.0, `spl-token-2022-interface` 3.1.1, `spl-token-metadata-interface` 1.0.1 and `spl-token-group-interface` 0.7.2. The 11.0.0 results can only be reproduced from that copy.

No Solana program was built for the probe. As a result, any behavior that needs a calling program, such as a CPI, is tagged [src] or [doc], never [probe].

**Version caveat**
- The latest upstream program release is `program@v11.1.0`, published 2026-09-23 ([releases](https://github.com/solana-program/token-2022/releases)).
- LiteSVM 0.16.0 bundles 11.0.0. LiteSVM 0.10.0, which ExtensionLab uses, bundles 10.0.0.
- The version deployed on mainnet was not checked.
- ExtensionLab should pin the Token-2022 ELF it tests against and print that version in every report.

## Re-run on LiteSVM 0.10.0 (Token-2022 10.0.0)

**Of 127 checks, 122 gave the same result, 2 gave a different result, and 3 were not probed.**

On 2026-10-22 the probe was ported into the repository and run on Token-2022 10.0.0, the build ExtensionLab tests against.
- Each check repeats one observation of the first run, with the same transactions and the same instruction order, so the `ix N` indices compare directly.
- The checks cover every [probe] tag in this document. The summary table and findings 1 to 4 restate per-extension results, so the same checks cover them.
- A check passes only if 10.0.0 gives exactly the recorded outcome: success, the failing instruction and its error, or the resulting balances, sizes and extension lists. Where 10.0.0 differs, the test asserts what 10.0.0 does.

**Differences**

| Check | Claim in this document | 11.0.0 (first run) | 10.0.0 (re-run) |
|---|---|---|---|
| CT.4 | NonTransferable section: NonTransferable with ConfidentialTransferMint but no ConfidentialMintBurn is rejected | `InvalidExtensionCombination` (0x33) at `InitializeMint2` | Accepted; the mint is created. `check_for_invalid_mint_extension_combinations` in interface 2.1.0 has no rule for this pair either; 3.1.1 adds it [src]. |
| PB.1 | PermissionedBurn section: a mint with the extension can be created (202 bytes) | Succeeds | `InvalidInstruction` (0xc) at the extension's initializer. 10.0.0 has no `PermissionedBurnExtension` instruction (46). Interface 2.1.0 has no PermissionedBurn module, so the probe encodes the instruction by hand, the way 3.1.1 does. |

**Not probed on this stack**

| Check | Claim | Reason |
|---|---|---|
| SIZE.m17 | A PermissionedBurn mint is 202 bytes | Interface 2.1.0 has no `PermissionedBurn` extension type to size, and 10.0.0 cannot create the mint (PB.1). |
| PB.2 | The owner's standard `BurnChecked` fails with `InvalidInstruction` (0xc) | No PermissionedBurn mint can exist on 10.0.0. |
| PB.3 | `BurnChecked` co-signed by the burn authority succeeds | Same reason. |

The JSON copy also tags the PYUSD mainnet read (finding 1) as probe evidence. That was an RPC read, not a LiteSVM run. The test makes no network calls, so the read was not repeated and is not one of the 127 checks.

**Stack differences that did not change a result**
- SPL Memo at `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr` is 3.0.0 here and was 4.0.0 in the first run. The memo checks (MEMO.1 to MEMO.7) and the checks that use the memo program as a stand-in hook (HOOK.6 to HOOK.9) gave the same results.
- Interfaces 2.1.0 and 3.1.1 list the same 69 `TokenError` codes in the same order. They compute the same size for every extension type that both have (the SIZE checks).
- Log text: on failure, the two runs logged the same set of distinct `Program log: Error: ...` and `Program ... failed: ...` lines. This was compared once by hand. The test does not assert it; it matches error codes. CPI Guard is a separate case: the `TokenError` messages in interface 2.1.0 add "via CPI" to the text the 10.0.0 program logs (see `adapters/escrow/tests/golden.rs`). The one CPI Guard result here, CPIG.4 (0x2f), logged no message in either run.
- Setup:
  - The first run used one keypair as fee payer and as every authority. The port uses a separate issuer key.
  - The port builds mints with `extensionlab_core::mint` where it supports the extension. Those mints always have a freeze authority.
  - IO.5 and IO.6 use a hook program id where the first run used `None`. Account sizing does not depend on it.
  - For MD.2 the first run reported `InsufficientFundsForRent { account_index: 1 }`, and index 1 was the mint. The index depends on the signer layout, so the port records the account instead.

## Severity and priority scale

**Severity** applies to an escrow or payment protocol:

| Severity | Meaning |
|---|---|
| **high** | A third party (issuer, authority or hook) can move, destroy or indefinitely lock tokens in custody. Or the protocol's accounting silently stops matching the vault balance. |
| **med** | A standard flow reverts, and the protocol can fix it with code changes. Or the protocol gains a liveness dependency on a user or issuer action. Or amount math is wrong when UI amounts are used. |
| **low** | The extension fails loudly at listing or setup, or is cosmetic for custody flows. |

**Priority** is for ExtensionLab:

| Priority | Meaning |
|---|---|
| **P0** | In the first scenario set. |
| **P1** | Second wave. |
| **P2** | Later, or documented only. |

## Summary table

| Extension | Lives on | Assumptions it breaks | Error(s) seen or expected | Default handling | Severity | Priority |
|---|---|---|---|---|---|---|
| TransferFeeConfig | mint (+ `TransferFeeAmount` on every account) | deposit credit, withdraw, refund, close account, amount math, account size, legacy `Transfer` | `InsufficientFunds` 0x1 [probe], `AccountHasWithheldTransferFees` 0x23 [probe], `MintRequiredForTransfer` 0x1f [probe], `FeeMismatch` 0x20 [probe], `InvalidAccountData` [probe] | support, special-case | high | P0 |
| TransferHook | mint (+ `TransferHookAccount` on every account) | CPI transfers, withdraw, refund, liveness (the hook can be switched on later) | `MissingAccount` [probe], `UnsupportedProgramId` [probe], `InvalidAccountData` [probe], hook's own error | special-case: allowlist hooks, forward extra accounts | high | P0 |
| PermanentDelegate | mint | PDA-owned vault custody, accounting equals balance | none; the delegate's transfer and burn succeed [probe] | reject (or allowlist trusted issuers) | high | P0 |
| Pausable | mint (+ `PausableAccount` on every account) | withdraw, refund, deadline logic, liveness | `MintPaused` 0x43 [probe] | special-case: treat as retryable | high | P0 |
| DefaultAccountState (Frozen) | mint (sets the initial state of accounts) | vault creation plus first deposit, refund to new accounts | `AccountFrozen` 0x11 [probe], `MintCannotFreeze` 0x10 at setup [probe] | special-case: check `is_frozen` | med | P0 |
| MemoTransfer (required memo) | account (owner toggles; can be added later) | withdraw, refund, payouts to user accounts; deposits into a memo vault | `NoMemo` 0x24 [probe] | special-case: memo CPI before each outbound transfer | med | P0 |
| CpiGuard | account (owner toggles) | CPI deposit signed by the user, CPI approve, CPI close, SetAuthority | `CpiGuardTransferBlocked` 0x2a [src], `CpiGuardApproveBlocked` 0x2d [src], `CpiGuardCloseAccountBlocked` 0x2c [src], `CpiGuardBurnBlocked` 0x2b [src], `CpiGuardOwnerChangeBlocked` 0x2f [probe] | special-case: delegate flow or top-level transfer | med | P0 |
| MintCloseAuthority | mint | cached mint validation, mint resurrection, stale vault layout | `MintHasSupply` 0x1c [probe], `InvalidState` 0xd [probe], `InvalidAccountData` [probe] | special-case: revalidate the mint every time, or reject | med | P1 |
| InterestBearingConfig | mint | UI amount vs raw amount, pricing | none (raw amounts unchanged) [probe]; `InvalidExtensionCombination` 0x33 with ScaledUiAmount [probe] | support: raw-only accounting | med | P1 |
| ScaledUiAmount | mint | UI amount vs raw amount, pricing, scheduled multiplier | none (raw amounts unchanged) [probe] | support: raw-only accounting | med | P1 |
| NonTransferable | mint (+ `NonTransferableAccount` and `ImmutableOwner` on accounts) | deposit (any transfer) | `NonTransferable` 0x25 [probe], `NonTransferableNeedsImmutableOwnership` 0x26 [probe] | reject at listing | low | P1 |
| ImmutableOwner | account (always present on Token-2022 ATAs) | escrows that move account ownership with `SetAuthority` | `ImmutableOwner` 0x22 [probe] | support: use PDA-owned vaults | low | P1 |
| ConfidentialTransfer (+ ConfidentialTransferFee) | mint + account (`ConfidentialTransferAccount`) | refunds to accounts that disabled public credits, closing confidential accounts, crediting confidential deposits | `NonConfidentialTransfersDisabled` 0x31 [src], `ConfidentialTransferAccountHasBalance` 0x17 [src], `InvalidExtensionCombination` 0x33 [probe] | support the public path only; do not configure vaults | med | P2 |
| MetadataPointer / TokenMetadata | mint | token identity by metadata, fixed mint size | `InsufficientFundsForRent` at setup when underfunded [probe] | support (ignore); never trust metadata for identity | low | P2 |
| GroupPointer / TokenGroup / GroupMemberPointer / TokenGroupMember | mint | none for custody flows | `SizeExceedsMaxSize` (group interface error 3406457177) [probe] | support (ignore) | low | P2 |
| PermissionedBurn (extra, not in the brief) | mint | protocols that burn (redemption, burn-on-withdraw) | `InvalidInstruction` 0xc [probe]; on 10.0.0 the extension does not exist, and its initializer fails with `InvalidInstruction` 0xc [probe] | special-case, or reject for burn-based designs | low (escrow) | P2 |

## Findings that cut across extensions

### 1. Having an extension is not the same as it being active

Several extensions are dormant switches that an authority can turn on after a protocol has accepted the mint:

- **TransferHook.** A mint whose hook `program_id` was `None` transferred normally. After the hook authority called `Update`, the same `transfer_checked` failed with `MissingAccount` [probe].
- **TransferFeeConfig.** The fee authority can raise the fee. The new fee applied two epochs later: 990 tokens were credited for a 1,000-token transfer at epoch +0 and +1, and 950 at epoch +2 [probe].
- **Pausable** can be paused [probe].
- **DefaultAccountState** can be changed by the freeze authority [probe].
- **PermanentDelegate** can be reassigned, or set to `None`, by the current delegate through `SetAuthority(PermanentDelegate)` [src].
- **InterestBearingConfig and ScaledUiAmount.** The rate and the multiplier can change; a negative rate was accepted [probe].

So detection must read three things: whether the extension exists, its current configuration, and whether an authority exists that can change it.

**Real example.** The PayPal USD (PYUSD) mint `2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo` was read from the public mainnet RPC on 2026-10-21, around slot 459,697,278 ([explorer](https://explorer.solana.com/address/2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo)). It carries:
- a `PermanentDelegate` that is set;
- a `TransferFeeConfig` at 0 bps, with its authority set;
- a `TransferHook` with no program id, but with its authority set;
- a `MintCloseAuthority` that is set;
- `ConfidentialTransferMint` and `ConfidentialTransferFeeConfig`;
- a freeze authority.

That one production stablecoin mint therefore holds four latent switches covered by this matrix.

### 2. Account size depends on the mint

Some mint extensions force extensions onto every token account at `InitializeAccount` [src] [probe]:

| Mint extension | Forces on each account |
|---|---|
| TransferFeeConfig | `TransferFeeAmount` |
| NonTransferable | `NonTransferableAccount` and `ImmutableOwner` |
| TransferHook | `TransferHookAccount` |
| Pausable | `PausableAccount` |

A program that allocates a hardcoded 165 bytes for a vault fails with `InvalidAccountData` [probe]. Size the account in one of three ways:
- call `GetAccountDataSize`;
- use `ExtensionType::try_calculate_account_len::<Account>` with the mint's required extensions;
- create the vault through the ATA program.

For example, for a mint carrying TransferFee, Pausable and TransferHook:
- `GetAccountDataSize` returned 187 bytes, or 191 when `ImmutableOwner` was added [probe].
- The ATA program created a 191-byte account with `[ImmutableOwner, TransferFeeAmount, PausableAccount, TransferHookAccount]` [probe].

### 3. The legacy `Transfer` instruction fails on common extensions

A plain `Transfer` (without the mint) on an account that carries `TransferFeeAmount`, `TransferHookAccount` or `PausableAccount` fails with `MintRequiredForTransfer` (0x1f) [probe] [src]. Always use `transfer_checked`, or `transfer_checked_with_fee`.

### 4. Mint resurrection is still a live issue on v11.0.0

A mint with `MintCloseAuthority` and zero supply can be closed. Its old token accounts remain, and anyone holding the mint keypair can re-create a different mint at the same address [probe]. Details are in the MintCloseAuthority section. In short:
- a re-created mint with `DefaultAccountState(Frozen)` leaves old accounts unfrozen, and they transfer without ever being thawed [probe];
- a vault created before the resurrection can become unable to receive fee-bearing deposits [probe].

### 5. Errors arrive as custom codes

Token-2022 returns `InstructionError(i, Custom(n))`, and only some errors also log a message. ExtensionLab should map `n` to `TokenError` names. The table below lists every code this matrix relies on. Codes come from the enum order in `spl-token-2022-interface` 3.1.1 ([docs.rs](https://docs.rs/spl-token-2022-interface/3.1.1/spl_token_2022_interface/error/enum.TokenError.html)). Interface 2.1.0 has the same order. Where the Evidence column says probe, the code was also seen in a run.

| Code | Name | Evidence |
|---|---|---|
| 0x1 | InsufficientFunds | probe |
| 0xc | InvalidInstruction | probe (PermissionedBurn; on 10.0.0, from its initializer) |
| 0xd | InvalidState | probe (resurrected fee mint) |
| 0x10 | MintCannotFreeze | probe |
| 0x11 | AccountFrozen | probe |
| 0x17 | ConfidentialTransferAccountHasBalance | src |
| 0x1c | MintHasSupply | probe |
| 0x1f | MintRequiredForTransfer | probe |
| 0x20 | FeeMismatch | probe |
| 0x22 | ImmutableOwner | probe |
| 0x23 | AccountHasWithheldTransferFees | probe |
| 0x24 | NoMemo | probe |
| 0x25 | NonTransferable | probe |
| 0x26 | NonTransferableNeedsImmutableOwnership | probe |
| 0x29 | CpiGuardSettingsLocked | src |
| 0x2a | CpiGuardTransferBlocked | src |
| 0x2b | CpiGuardBurnBlocked | src |
| 0x2c | CpiGuardCloseAccountBlocked | src |
| 0x2d | CpiGuardApproveBlocked | src (plus the owner's earlier LiteSVM test, not re-run here) |
| 0x2e | CpiGuardSetAuthorityBlocked | src |
| 0x2f | CpiGuardOwnerChangeBlocked | probe |
| 0x31 | NonConfidentialTransfersDisabled | src |
| 0x33 | InvalidExtensionCombination | probe |
| 0x43 | MintPaused | probe |

Some failures are not Token-2022 custom errors:
- `MissingAccount`, `UnsupportedProgramId` and `InvalidAccountData` are runtime or program errors.
- `InsufficientFundsForRent` is a transaction-level error.

### 6. General detection pattern for on-chain programs

1. Check that the mint and token accounts are owned by SPL Token or Token-2022. The owner's reference test `validation_rejects_a_forged_mint_owned_by_another_program` shows why: `StateWithExtensions::unpack` validates only the byte layout.
2. Unpack the mint with `StateWithExtensions::<Mint>::unpack`, walk `get_extension_types()`, and compare each type against an allowlist.
3. For allowed extensions, read the live configuration and the authority fields.
4. Do this on every instruction that moves tokens, not only when a pool or escrow is created. Cached validation goes stale because of finding 1 (authorities can change configuration) and finding 4 (resurrection).

The owner's `handle_assert_supported_mint` in `reference/turbin3_q3_26/token22-q326/programs/t22/src/instructions/mint.rs` implements steps 1 and 2. The official on-chain guide also says not to store mint information that can go stale, and to read it from the mint instead ([on-chain guide](https://www.solana-program.com/docs/token-2022/onchain)). This is a paraphrase, not a quote.

**Anchor caveat [unverified].** According to the owner's reference code comments in `lib.rs`, Anchor's `InterfaceAccount<Mint>` throws away the extension data and keeps only the base struct. So extension checks need the raw account bytes. This was not checked against Anchor's source here.

## Per-extension notes

### TransferFeeConfig

**What it does.** The mint charges a fee in basis points, capped at `maximum_fee`, on every transfer. The fee is withheld in the recipient's account, where the recipient cannot spend it ([docs](https://www.solana-program.com/docs/token-2022/extensions#transfer-fees)).

**Where it lives.** On the mint (`TransferFeeConfig`, type 1). It forces `TransferFeeAmount` (type 2) onto every token account.

**What breaks**
- **Deposit credit.** The protocol credits `amount`, but the vault receives `amount - fee`. Depositing 10,000 at 100 bps left 9,900 in the vault and 100 withheld [probe].
- **Withdraw.** Paying out the credited 10,000 failed with `InsufficientFunds` (0x1) [probe]. In a pooled vault, this hits the last withdrawer. Paying out the 9,900 actually held delivered only 9,801; the other 99 was withheld on the user's own account [probe].
- **Refund.** A full round trip loses two fees, one on the way in and one on the way out (10,000 to 9,900 to 9,801) [probe].
- **Amount math.** The fee is `ceil(amount * bps / 10_000)`, capped at `maximum_fee` [src]. A transfer of 1 unit at 1% credited 0 and withheld 1 [probe], so dust-sized deposits credit nothing.
- **Fee schedule changes.** `SetTransferFee` takes effect at epoch +2 [probe]. Use `get_epoch_fee(current_epoch)` and also look at `newer_transfer_fee` for a pending change [src].
- **Close account.** A vault at zero balance that still holds withheld fees fails `CloseAccount` with `AccountHasWithheldTransferFees` (0x23) [probe]. `HarvestWithheldTokensToMint` needs no authority; only the fee payer signed [probe]. After a harvest, the close succeeded [probe].
- **CPI transfers.**
  - Legacy `Transfer` fails with `MintRequiredForTransfer` (0x1f) [probe].
  - `TransferCheckedWithFee` with the wrong expected fee fails with `FeeMismatch` (0x20) [probe].
  - A 165-byte account fails `InitializeAccount3` with `InvalidAccountData` [probe].

**Building it in LiteSVM**
- **Mint:**
  1. `create_account(space = 278)`
  2. `initialize_transfer_fee_config(fee_authority, withdraw_authority, bps, max_fee)`
  3. `initialize_mint2`
- **Token accounts:** 178 bytes, or let `GetAccountDataSize` or the ATA program size them.
- **Fee changes:** call `set_transfer_fee`, then advance `Clock.epoch` by 2 with `svm.set_sysvar::<Clock>`.
- **Local reference:** the owner's `create_mint_with_fee` and `create_remittance_mint` in `reference/turbin3_q3_26/token22-q326/programs/t22/src/instructions/{mint,remittance}.rs` build the same thing through CPI.

**How to handle it.** Support it as a special case:
- Credit the measured vault balance change (post minus pre) rather than the instruction amount, or compute `calculate_epoch_fee(epoch, amount)` yourself.
- Use `transfer_checked_with_fee` with the expected fee, so a fee change fails fast instead of mis-crediting.
- For flows that must deliver an exact net amount, use `calculate_inverse_epoch_fee`.
- Harvest before closing vaults.
- Optionally reject mints whose fee, or pending newer fee, is above a set tolerance.
- The on-chain guide says to subtract the fee from the amount used in calculations ([on-chain guide](https://www.solana-program.com/docs/token-2022/onchain)).

**How to detect it.** `mint.get_extension::<TransferFeeConfig>()`, then check:
- `get_epoch_fee(epoch)`;
- `newer_transfer_fee.epoch`;
- `transfer_fee_config_authority`, since `Some` means the fee can change.

**Severity:** high. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#transfer-fees
- https://www.solana-program.com/docs/token-2022/onchain
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### TransferHook

**What it does.** The mint names a program that Token-2022 calls through CPI (`Execute`) on every `transfer_checked`. The call happens "after all other transfer logic, so the accounts reflect the end state of the transfer" ([docs](https://www.solana-program.com/docs/token-2022/extensions#transfer-hook)).

**Where it lives.** On the mint (`TransferHook`, type 14). It forces `TransferHookAccount` (type 15) onto every token account, which holds the `transferring` flag.

**What breaks**
- **CPI transfers.** The caller must pass three things in the transfer's account list:
  - the hook program;
  - its validation account, a PDA with seeds `"extra-account-metas"` plus the mint, under the hook program ([extra accounts](https://www.solana-program.com/docs/transfer-hook-interface/configuring-extra-accounts));
  - every extra account the validation account lists.

  When those are missing:
  - With no extra accounts, the transfer failed with `MissingAccount` [probe].
  - When the hook id pointed to a non-existent program and that account was passed, it failed with `UnsupportedProgramId` [probe].
  - When a stand-in program was passed with a validation PDA that did not exist, it failed with `InvalidAccountData` [probe].
- **Withdraw and refund liveness.** The hook can reject any transfer, for example with a recipient blocklist, so a withdraw or refund can revert at any time. The hook authority can also turn the hook on, or change it, after the protocol has accepted the mint [probe, as described in finding 1].
- **Privileges.** "All accounts from the initial transfer are converted to read-only accounts", and the sender's signer privilege does not extend to the hook ([solana.com](https://solana.com/docs/tokens/extensions/transfer-hook)) [doc]. The spec lists source, mint, destination and authority as read-only, non-signer accounts ([spec](https://www.solana-program.com/docs/transfer-hook-interface/specification)) [doc].
- **Self-transfers.** A transfer from an account to itself does not invoke the hook: it succeeded with no extra accounts after the hook was switched on [probe] [doc].
- **Legacy `Transfer`** fails with `MintRequiredForTransfer` (0x1f) [probe].
- **Transaction size and compute.** Extra accounts raise the transaction's account count and compute use [unverified; not measured].

**Building it in LiteSVM**
- **Mint:**
  1. `create_account(space = 234)`
  2. `transfer_hook::instruction::initialize(authority, Some(hook_program_id))`
  3. `initialize_mint2`
- **Token accounts:** 171 bytes.
- **What the probe did not cover.** Realistic tests need real hook programs loaded with `svm.add_program`, for example always-allow, always-deny, deny-recipient, and require-an-extra-writable-account. Each needs its validation account created by the hook program's `InitializeExtraAccountMetaList`. Building those hook programs is a separate task. None were built here.

**How to handle it.** Treat it as a special case:
- Allowlist the hook program ids you accept, and reject the rest.
- Reject mints whose hook authority is `Some`, unless the issuer is trusted.
- In CPI paths, use `spl_token_2022::onchain::invoke_transfer_checked`, which resolves the hook accounts from the remaining accounts passed in ([onchain.rs](https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/onchain.rs)) [src].
- Have clients resolve the extra accounts off-chain, with `getTransferCheckedWithTransferHookInstructionAsync` or `createTransferCheckedWithTransferHookInstruction` ([solana.com](https://solana.com/docs/tokens/extensions/transfer-hook)).
- Design withdrawals so that one reverting transfer cannot lock other users: prefer pull-based claims over batched pushes [unverified design advice].

**How to detect it.** `mint.get_extension::<TransferHook>()`, then read `program_id` (an `Option`) and `authority` (an `Option`).

**Severity:** high. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#transfer-hook
- https://www.solana-program.com/docs/transfer-hook-interface/specification
- https://www.solana-program.com/docs/transfer-hook-interface/configuring-extra-accounts
- https://solana.com/docs/tokens/extensions/transfer-hook
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs

### PermanentDelegate

**What it does.** A delegate set on the mint can transfer or burn tokens from any token account of that mint, with no signature from the account owner ([docs](https://www.solana-program.com/docs/token-2022/extensions#permanent-delegate)). "Token account owners cannot revoke the permanent delegate from their token accounts" ([solana.com](https://solana.com/docs/tokens/extensions/permanent-delegate)).

**Where it lives.** On the mint (`PermanentDelegate`, type 12).

**What breaks**
- **PDA-owned vault custody.** The delegate transferred 600 and burned 400 out of a vault without the vault owner signing, leaving it at 0 [probe]. Protocol liabilities stay the same while the vault balance drops, so every later withdrawal fails with `InsufficientFunds`.
- **CPI Guard does not stop it.** "The permanent delegate may always burn or transfer tokens, bypassing CPI Guard" ([docs](https://www.solana-program.com/docs/token-2022/extensions#cpi-guard)).
- **The delegate can change.** It can be reassigned, or set to `None`, by the current delegate through `SetAuthority(PermanentDelegate)` [src].
- **A warning is logged.** Token-2022 logs `Warning: Mint has a permanent delegate, so tokens in this account may be seized at any time` on `InitializeAccount` [probe].
- **Confidential balances are out of reach.** The extension "does not work with tokens stored in the confidential balance" ([docs](https://www.solana-program.com/docs/token-2022/extensions#permanent-delegate)).

**Building it in LiteSVM**
1. `create_account(space = 202)`
2. `initialize_permanent_delegate(delegate)`
3. `initialize_mint2`

The delegate then signs `transfer_checked` or `burn_checked` as the authority. Local reference: the owner's test `a_permanent_delegate_moves_tokens_without_consent` in `tests/authority.rs`.

**How to handle it**
- Reject by default.
- If a trusted issuer must be supported (PYUSD carries this extension, as noted above), then:
  - reconcile liabilities against the actual vault balance;
  - define what happens on a shortfall, for example pro-rata losses;
  - make withdrawals degrade gracefully when the vault holds less than recorded.

**How to detect it.** `mint.get_extension::<PermanentDelegate>()` with `delegate.is_some()`.

**Severity:** high. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#permanent-delegate
- https://solana.com/docs/tokens/extensions/permanent-delegate
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### Pausable

**What it does.** The pause authority can stop transfers, mints and burns for the whole mint ([docs](https://www.solana-program.com/docs/token-2022/extensions#pausable), [solana.com](https://solana.com/docs/tokens/extensions/pausable)).

**Where it lives.** On the mint (`Pausable`, type 26). It forces `PausableAccount` (type 27) onto every token account.

**What breaks**
- **Blocked while paused.** `transfer_checked`, `mint_to` and `burn_checked` all failed with `MintPaused` (0x43) [probe]. The legacy `Transfer` failed with `MintRequiredForTransfer` (0x1f) [probe].
- **Allowed while paused.** `Approve` and closing an empty account both succeeded [probe].
- **Resume.** After `resume`, withdrawals worked again [probe].
- **Deadline logic.** Escrows with refund-after-timeout, liquidations or expiries can miss their deadlines during a pause [unverified; follows from the above].

**Building it in LiteSVM**
- **Mint:**
  1. `create_account(space = 203)`
  2. `pausable::instruction::initialize(authority)`
  3. `initialize_mint2`
- **Token accounts:** 170 bytes.
- **Pausing:** the authority signs `pause` and `resume`.

**How to handle it.** Treat it as a special case:
- Treat `MintPaused` as retryable.
- Do not penalize users for deadlines missed while the mint was paused.
- Make sure no partial state is committed when the token CPI fails. A failed CPI reverts the whole transaction, so this mostly concerns flows that span several transactions.

**How to detect it.** `mint.get_extension::<PausableConfig>()`, then read `paused` and `authority`.

**Severity:** high. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#pausable
- https://solana.com/docs/tokens/extensions/pausable
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs

### DefaultAccountState (Frozen)

**What it does.** New token accounts start in the `Frozen` state. Only the freeze authority can thaw them ([docs](https://www.solana-program.com/docs/token-2022/extensions#default-account-state)).

**Where it lives.** On the mint (`DefaultAccountState`, type 6). It sets the starting state of each account.

**What breaks**
- **New vaults and accounts start frozen.** A newly created vault was frozen [probe]. Depositing into it failed with `AccountFrozen` (0x11), and so did `mint_to` [probe]. Any "create vault and deposit" transaction reverts.
- **Refunds into new accounts.** A refund into a freshly created user account fails the same way [unverified; same code path].
- **A freeze authority is required.** Setting a frozen default without a freeze authority failed at `InitializeMint2` with `MintCannotFreeze` (0x10) [probe]. Any such mint can therefore also freeze the protocol's vault after deposits; a later freeze made the withdrawal fail with `AccountFrozen` [probe].
- **Updating the default.** The freeze authority switched the default to `Initialized`, and only accounts created after that were unfrozen [probe].

**Building it in LiteSVM**
1. `create_account(space = 171)`
2. `initialize_default_account_state(Frozen)`
3. `initialize_mint2` with `freeze_authority = Some(..)`

Then `thaw_account` signed by the freeze authority. Local reference: the owner's `create_remittance_mint` and `test_remittance_lifecycle_tasks_1_to_4`.

**How to handle it.** Treat it as a special case:
- Check `account.base.is_frozen()` right after creating a vault, and return a clear error.
- Document that the issuer must thaw protocol vaults.
- Or reject such mints.

**How to detect it.** `mint.get_extension::<DefaultAccountState>()` with `state == Frozen`, plus `mint.base.freeze_authority`.

**Severity:** med. Taken alone, the extension causes onboarding failures. The freeze authority it requires is a separate risk that classic SPL tokens also carry. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#default-account-state
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### MemoTransfer (required memo)

**What it does.** Incoming transfers to an account with this flag "must have an accompanying memo instruction right before the transfer instruction". This "also works in CPI contexts, as long as a CPI is performed to log the memo before invoking the transfer" ([docs](https://www.solana-program.com/docs/token-2022/extensions#required-memo-on-transfer)).

**Where it lives.** On the token account (`MemoTransfer`, type 8). The account owner turns it on or off.

**What breaks**
- **Deposits into a memo-required vault.** Without a memo, the deposit failed with `NoMemo` (0x24). It also failed when the memo came after the transfer. It succeeded with the memo directly before [probe].
- **Outbound transfers.** Transfers out of a memo-required account need no memo [probe].
- **Withdraw, refund and payout to users.** A user can reallocate their own account (it grew from 165 to 171 bytes) and enable required memos at any time [probe]. After that, a protocol withdrawal to that user without a memo failed with `NoMemo`, and with a memo directly before it succeeded [probe].
- **Batched payouts.** A single recipient can revert a whole batch this way [unverified; follows from the above].
- **CPI rule.** In a CPI, the memo must be the previously processed sibling instruction. Token-2022 accepts the SPL Memo program ids listed in `memo_transfer/mod.rs` [src].

**Building it in LiteSVM**
1. Create the account with 171 bytes, or call `reallocate(&[MemoTransfer])` on an existing one.
2. `initialize_account3`
3. `enable_required_transfer_memos`, signed by the owner.

The memo instruction goes to `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`, which LiteSVM loads by default. It takes no accounts and UTF-8 data.

**How to handle it.** Treat it as a special case. Before every outbound CPI transfer, CPI the memo program, either always or whenever `memo_required(&destination)` is true. Protocol vaults should not enable required memos.

**How to detect it.** `account.get_extension::<MemoTransfer>()`, then read `require_incoming_transfer_memos`.

**Severity:** med. **Priority:** P0.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#required-memo-on-transfer
- https://www.solana-program.com/docs/token-2022/extensions#reallocate
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/extension/memo_transfer/mod.rs

### CpiGuard

**What it does.** When the owner enables it, the account restricts what a program can do through CPI ([docs](https://www.solana-program.com/docs/token-2022/extensions#cpi-guard)):
- transfer and burn are allowed only when the signer is a delegate;
- `Approve` is prohibited;
- `CloseAccount` must send the lamports to the owner;
- setting a close authority is prohibited unless it is being unset;
- changing the owner is always prohibited, even outside CPI.

**Where it lives.** On the token account (`CpiGuard`, type 11). The owner turns it on and off, and only at the top level; doing it inside a CPI fails with `CpiGuardSettingsLocked` (0x29) [src].

**What breaks**
- **The standard CPI deposit.** In the usual pattern, the user signs the outer transaction and the program CPIs `transfer_checked` with the user as authority. That fails with `CpiGuardTransferBlocked` (0x2a) [src: `process_transfer` blocks the case where the authority is the owner, the guard is on, and the call is in a CPI]. Not executed here, because that needs a calling program.
- **Approve through CPI** fails with `CpiGuardApproveBlocked` (0x2d) [src]. The owner's test `cpi_guard_blocks_delegation_issued_through_a_cpi` covers this; it was not re-run for this task.
- **Closing a user account through CPI** with rent sent to the protocol fails with `CpiGuardCloseAccountBlocked` (0x2c) [src].
- **Burn by the owner through CPI** fails with `CpiGuardBurnBlocked` (0x2b) [src].
- **At the top level**, the owner's transfer and `Approve` succeeded [probe]. `SetAuthority(AccountOwner)` failed with `CpiGuardOwnerChangeBlocked` (0x2f) [probe].
- **What the guard still allows.** In CPI, transfers signed by a delegate are allowed [src], and so is the permanent delegate [doc].

**Building it in LiteSVM**
1. Create the account with 171 bytes.
2. `initialize_account3`
3. `enable_cpi_guard`, signed by the owner.

The TLV entry exists only after `enable_cpi_guard`: the extension list was `[]` before and `[CpiGuard]` after [probe], which matches the owner's test note. CPI scenarios run the user's own program, which ExtensionLab loads anyway.

**How to handle it.** Treat it as a special case. Support one of two flows:
- a delegate flow, where the user signs a top-level `Approve` in the same transaction and the program then transfers as the delegate;
- a top-level user transfer followed by a program instruction that verifies the vault balance change.

**How to detect it.** `account.get_extension::<CpiGuard>()`, then read `lock_cpi`.

**Severity:** med. **Priority:** P0. It is cheap to construct and hits the most common deposit pattern.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#cpi-guard
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### MintCloseAuthority

**What it does.** Lets the close authority close the mint once its supply is 0 ([docs](https://www.solana-program.com/docs/token-2022/extensions#mint-close-authority)).

**Where it lives.** On the mint (`MintCloseAuthority`, type 3).

**What breaks**
- **Closing the mint.** With supply 100, the close failed with `MintHasSupply` (0x1c). At supply 0 it succeeded. The mint account disappeared while its token accounts remained [probe].
- **Resurrection.** The mint was re-created at the same address with different settings, decimals 6 to 0 and a 10% fee added [probe]. Old zero-balance accounts accepted `mint_to` under the new mint [probe]. The docs warn that closing and re-initializing a mint can leave existing token accounts without extensions such as transfer fees ([docs](https://www.solana-program.com/docs/token-2022/extensions#mint-close-authority)). This is a paraphrase, not a quote. How old accounts behaved under each re-created configuration [probe]:

  | Re-created mint adds | Old account behavior |
  |---|---|
  | DefaultAccountState (Frozen) | Stayed unfrozen and transferred out without ever being thawed. This bypasses the frozen-by-default control. |
  | NonTransferable | `mint_to` was blocked with `NonTransferableNeedsImmutableOwnership` (0x26). |
  | TransferFeeConfig | Fee-bearing transfers into the old account failed with `InvalidState` (0xd). Transfers out were charged normally (900 credited, 100 withheld). |
  | TransferHook | Transfers out of the old account failed with `InvalidAccountData`. |

- **Stale protocol state.** Cached validation becomes wrong: decimals, the extension allowlist checked at pool creation, and the vault layout. A vault created before the resurrection can become unable to receive deposits [probe, using the `InvalidState` case].

**Building it in LiteSVM**
1. `create_account(space = 202)`
2. `initialize_mint_close_authority(Some(auth))`
3. `initialize_mint2`
4. Later: `close_account(mint, destination, auth)`.

To resurrect, run `create_account` again with the same mint keypair and initialize a different set of extensions. Local reference: the owner's `create_mint_with_fee`.

**How to handle it.** Treat it as a special case:
- Revalidate the mint (decimals and extension allowlist) on every instruction.
- Check that the vault's account extensions still match the mint's required account extensions.
- Optionally reject mints with a close authority. The on-chain guide suggests this for pools ([on-chain guide](https://www.solana-program.com/docs/token-2022/onchain)). Rejecting the extension alone is not enough, because a re-created mint need not carry it ([Neodyme](https://neodyme.io/en/blog/token-2022)) [secondary].

**How to detect it.** `mint.get_extension::<MintCloseAuthority>()` with `close_authority.is_some()`. Past resurrection cannot be read from the current mint alone [unverified].

**Severity:** med. **Priority:** P1.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#mint-close-authority
- https://www.solana-program.com/docs/token-2022/onchain
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### InterestBearingConfig

**What it does.** The UI amount accrues continuously compounded interest at a rate in basis points, based on the network timestamp. "No new tokens are ever created" ([docs](https://www.solana-program.com/docs/token-2022/extensions#interest-bearing-tokens)).

**Where it lives.** On the mint (`InterestBearingConfig`, type 10).

**What breaks**
- **UI amount vs raw amount.** At 5%, the raw balance stayed at 1,000,000,000 (6 decimals). The UI amount went from `1000` to `1051.236557` after one simulated year [probe].
- **Converting UI input.** `UiAmountToAmount("1000")` returned 951,260,678 raw units after that year [probe]. A protocol that stores UI amounts, or prices from them, drifts.
- **Rate changes.** The rate authority set a negative rate, -1000 bps [probe].
- **Invalid combination.** The extension cannot be combined with ScaledUiAmount; that fails with `InvalidExtensionCombination` (0x33) [probe].

**Building it in LiteSVM**
1. `create_account(space = 222)`
2. `interest_bearing_mint::instruction::initialize(rate_authority, rate_bps)`
3. `initialize_mint2`

Move `Clock.unix_timestamp` forward to simulate time. `amount_to_ui_amount` returns a UTF-8 string in the transaction's return data.

**How to handle it.** Support it, with all accounting in raw `u64` units. Convert to UI amounts only at the display edge.

**How to detect it.** `mint.get_extension::<InterestBearingConfig>()`, then read `rate_authority` and the current rate.

**Severity:** med, or low if the protocol never uses UI amounts. **Priority:** P1.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#interest-bearing-tokens
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs

### ScaledUiAmount

**What it does.** "The UI amount returns the raw amount of tokens multiplied by the current multiplier". The multiplier authority can schedule a new multiplier for a future timestamp ([docs](https://www.solana-program.com/docs/token-2022/extensions#scaled-ui-amount)).

**Where it lives.** On the mint (`ScaledUiAmount`, type 25).

**What breaks**
- **UI amount vs raw amount.** A raw balance of 1,000,000 stayed unchanged. The UI amount was `1` before the scheduled time and `2` after it [probe]. After the change, `UiAmountToAmount("1")` returned 500,000 [probe].
- **Reading the stored field.** Code that reads only the stored `multiplier` field is wrong once the effective time passes. The active value is `new_multiplier` from `new_multiplier_effective_timestamp` onward ([solana.com](https://solana.com/docs/tokens/extensions/scaled-ui-amount)) [doc].
- **Rounding.** Conversions use floating point and "are not guaranteed to round-trip exactly" [doc].

**Building it in LiteSVM**
1. `create_account(space = 226)`
2. `scaled_ui_amount::instruction::initialize(authority, multiplier_f64)`
3. `initialize_mint2`

Then `update_multiplier(new, effective_ts)`, and move `Clock.unix_timestamp` forward.

**How to handle it.** Support it with raw-only accounting. The official guidance: "dApps and service providers should use the raw amount and non-scaled prices for all calculations." When converting user input to raw amounts, round down ([integration guide](https://solana.com/docs/tokens/extensions/scaled-ui-amount/integration-guide)).

**How to detect it.** `mint.get_extension::<ScaledUiAmountConfig>()`.

**Severity:** med. **Priority:** P1.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#scaled-ui-amount
- https://solana.com/docs/tokens/extensions/scaled-ui-amount
- https://solana.com/docs/tokens/extensions/scaled-ui-amount/integration-guide

### NonTransferable

**What it does.** The tokens are "soul-bound": they cannot be moved to another account. The owner can still burn them and close the account ([docs](https://www.solana-program.com/docs/token-2022/extensions#non-transferable-tokens)).

**Where it lives.** On the mint (`NonTransferable`, type 9). It forces `NonTransferableAccount` (type 13) and `ImmutableOwner` onto every token account, making it 174 bytes.

**What breaks**
- **Every transfer, including deposits.** A deposit failed with `NonTransferable` (0x25) [probe].
- **Burn and close still work.** The owner's burn and the close that followed both succeeded [probe].
- **Account size.** A 165-byte account failed with `InvalidAccountData` [probe].
- **Minting into accounts without the required extensions** fails with `NonTransferableNeedsImmutableOwnership` (0x26) [probe, seen in the resurrection case].
- **Invalid combination.** NonTransferable with ConfidentialTransferMint but no ConfidentialMintBurn fails with `InvalidExtensionCombination` [probe]. On 10.0.0 the combination is accepted, and the mint is created [probe]. The rule is in the interface's `check_for_invalid_mint_extension_combinations` in 3.1.1, not in 2.1.0 [src].

**Building it in LiteSVM**
1. `create_account(space = 170)`
2. `initialize_non_transferable_mint`
3. `initialize_mint2`

**How to handle it.** Reject it when listing the token; an escrow needs transfers.

**How to detect it.** `mint.get_extension::<NonTransferable>()`, or `NonTransferableAccount` on the account.

**Severity:** low. **Priority:** P1.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#non-transferable-tokens
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs

### ImmutableOwner

**What it does.** "Makes it impossible to reassign ownership of an account". "The Associated Token Account program always uses this extension when creating accounts" ([docs](https://www.solana-program.com/docs/token-2022/extensions#immutable-owner)).

**Where it lives.** On the token account (`ImmutableOwner`, type 7).

**What breaks**
- **Escrows that hand over account ownership.** Some escrows reassign the initializer's token account to a PDA with `SetAuthority(AccountOwner)`. On an ImmutableOwner account, and on a Token-2022 ATA, that failed with `ImmutableOwner` (0x22) [probe]. Without the extension, the same call succeeded [probe].
- **Token-2022 ATAs always carry it.** The ATA's extensions were `[ImmutableOwner]` [probe].

**Building it in LiteSVM**
1. `create_account(space = 170)`
2. `initialize_immutable_owner`
3. `initialize_account3`

Alternatively, create an ATA through the ATA program: instruction data `[0]` creates it, and `[1]` creates it idempotently.

**How to handle it.** Support it. Move tokens into program-owned or PDA-owned vault accounts instead of reassigning account ownership.

**How to detect it.** `account.get_extension::<ImmutableOwner>()`.

**Severity:** low. **Priority:** P1.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#immutable-owner
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs

### ConfidentialTransfer (and ConfidentialTransferFee)

**What it does.** Balances and transfer amounts can be encrypted with ElGamal and moved with zero-knowledge proofs. Each token account must first be configured by its owner with a proof ([solana.com](https://solana.com/docs/tokens/extensions/confidential-transfer)).

**Where it lives.**
- On the mint: `ConfidentialTransferMint` (type 4), plus `ConfidentialTransferFeeConfig` (type 16) when a transfer fee is also present.
- On the account: `ConfidentialTransferAccount` (type 5, 465 bytes) and `ConfidentialTransferFeeAmount` (type 17).

**What breaks**
- **Nothing, on the public path.** Public `transfer_checked` between unconfigured accounts of a confidential-transfer mint worked unchanged [probe].
- **Refunds and payouts.** A recipient can turn off public credits with `DisableNonConfidentialCredits` [doc]. A public transfer to that account then fails with `NonConfidentialTransfersDisabled` (0x31) [src]. Not executed.
- **Deposit credit.** Confidential transfers land in an encrypted pending balance. Applying it needs the account owner's AES key; the program cannot compute the ciphertext. See the comments on `apply_pending_balance` in the owner's `lib.rs`. A program-owned vault configured this way cannot use its own funds without off-chain keys [unverified for PDAs].
- **Close account.** A non-zero confidential balance blocks `CloseAccount` with `ConfidentialTransferAccountHasBalance` (0x17) [src]. `EmptyAccount` is needed first [doc].
- **Pending credit limit.** An attacker can exceed `maximum_pending_balance_credit_counter` and block incoming transfers to a configured account ([Neodyme](https://neodyme.io/en/blog/token-2022)) [secondary].
- **Invalid combination.** ConfidentialTransferMint with TransferFeeConfig but no ConfidentialTransferFeeConfig fails with `InvalidExtensionCombination` (0x33) [probe].

**Building it in LiteSVM**
- **Mint:**
  1. `create_account(space = 235)`
  2. `confidential_transfer::instruction::initialize_mint(authority, auto_approve, auditor)`
  3. `initialize_mint2`
- **Full lifecycle:** follow the owner's `tests/confidential.rs`:
  - `ConfigureAccount` with a pubkey-validity proof placed at an instruction offset;
  - context-state accounts for the equality, ciphertext-validity and range proofs;
  - a compute-unit limit of 400,000.

**How to handle it.** Support the public path only:
- Never configure protocol vaults for confidential transfers.
- Treat `NonConfidentialTransfersDisabled` on payouts as a failure for that one recipient, which favors pull-based claims.

**How to detect it.**
- Mint: `get_extension::<ConfidentialTransferMint>()`.
- Destination account: `get_extension::<ConfidentialTransferAccount>()`, then check that non-confidential credits are allowed.

**Severity:** med. **Priority:** P2, because proofs are heavy to build and the public path is unchanged.

**Sources:**
- https://solana.com/docs/tokens/extensions/confidential-transfer
- https://www.solana-program.com/docs/confidential-balances
- https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- https://neodyme.io/en/blog/token-2022 [secondary]

### MetadataPointer and TokenMetadata

**What they do.** The pointer names the account that holds the token's metadata, which can be the mint itself. TokenMetadata stores name, symbol, URI and extra fields inside the mint as a variable-length TLV entry ([docs](https://www.solana-program.com/docs/token-2022/extensions#metadata-pointer)).

**Where they live.** On the mint (`MetadataPointer`, type 18; `TokenMetadata`, type 19).

**What breaks**
- **Transfers are unaffected** [probe].
- **Mint size is variable.** The mint grew from 234 to 364 bytes when metadata was initialized [probe]. Token-2022 reallocates the account but does not pay for it. If the account's lamports do not cover the new size, the transaction fails with `InsufficientFundsForRent` [probe].
- **The pointer can name any address.** A pointer to an arbitrary foreign address was accepted [probe]. "A client must check that the mint and the metadata both point to each other" [doc].
- **Metadata can change.** The update authority can change name, symbol and URI [unverified; not probed].

**Building it in LiteSVM**
1. `create_account(space = 234, lamports = rent(234 + metadata TLV))`, where the TLV is 4 + 32 + 32 + (4 + name) + (4 + symbol) + (4 + uri) + 4 bytes.
2. `metadata_pointer::instruction::initialize(authority, Some(mint))`
3. `initialize_mint2`
4. `spl_token_metadata_interface::instruction::initialize(program_id = Token-2022, metadata = mint, update_authority, mint, mint_authority, name, symbol, uri)`, signed by the mint authority.

Local reference: the owner's `create_remittance_mint` sets a pointer to the mint itself.

**How to handle it.** Support it by ignoring it for custody. Never identify a token by its metadata; identify it by the mint address.

**How to detect it.** `get_extension::<MetadataPointer>()` and `get_variable_len_extension::<TokenMetadata>()`.

**Severity:** low. **Priority:** P2.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#metadata-pointer
- https://www.solana-program.com/docs/token-2022/extensions#metadata
- https://neodyme.io/en/blog/token-2022 [secondary]

### GroupPointer, TokenGroup, GroupMemberPointer and TokenGroupMember

**What they do.** They configure collections: a group mint, and member mints that point to it ([docs](https://www.solana-program.com/docs/token-2022/extensions#group-pointer)).

**Where they live.** On the mint (types 20 to 23).

**What breaks**
- **Nothing on the transfer path.** `process_transfer` has no group checks [src]. Transfers on group mints were not run in the probe.
- **Group size limit.** With `max_size = 2`, the third member failed with `SizeExceedsMaxSize` (token-group interface error 3406457177) [probe].
- **Mint size.** The group mint grew from 234 to 318 bytes when the group was initialized [probe].

**Building it in LiteSVM**
- **Group mint:**
  1. `group_pointer::instruction::initialize(authority, Some(mint))` before `initialize_mint2`.
  2. After it: `spl_token_group_interface::instruction::initialize_group(program_id = Token-2022, group = mint, mint, mint_authority, update_authority, max_size)`.
- **Member mint:**
  1. `group_member_pointer::instruction::initialize` before `initialize_mint2`.
  2. After it: `initialize_member(Token-2022, member, member_mint, member_mint_authority, group, group_update_authority)`.
- Fund extra lamports for the growth.

**How to handle it.** Support it by ignoring it for escrows.

**Severity:** low. **Priority:** P2.

**Sources:**
- https://www.solana-program.com/docs/token-2022/extensions#group-pointer
- https://www.solana-program.com/docs/token-2022/extensions#group
- https://www.solana-program.com/docs/token-2022/extensions#member-pointer
- https://www.solana-program.com/docs/token-2022/extensions#member

### PermissionedBurn (extra)

**What it does.** Once a burn authority is set, every burn also needs that authority's signature. Standard `Burn` and `BurnChecked` are rejected ([docs](https://www.solana-program.com/docs/token-2022/extensions#permissioned-burn)).

**Where it lives.** On the mint (`PermissionedBurn`, type 28).

**What breaks**
- The owner's standard `BurnChecked` failed with `InvalidInstruction` (0xc) [probe].
- The permissioned `BurnChecked` with the burn authority co-signing succeeded [probe].
- On 10.0.0 the extension does not exist. Its initializer, instruction 46, fails with `InvalidInstruction` (0xc), so no such mint can be created [probe]. The two results above were therefore not re-run on 10.0.0. Interface 2.1.0 has no PermissionedBurn module either.

**Building it in LiteSVM**
1. `create_account(space = 202)`
2. `permissioned_burn::instruction::initialize(authority)`
3. `initialize_mint2`

**How to handle it.** Reject it for burn-based designs; no change is needed for pure escrows.

**Severity:** low. **Priority:** P2.

## Account size reference

Sizes were computed with `try_calculate_account_len` and checked against accounts created in the probe [probe].

**Mint with a single extension.** A bare mint is 82 bytes. Adding any extension pads it to 165 bytes, adds a 1-byte account type, and then adds 4 bytes plus the extension's length for each extension.

| Mint extension | Mint size (bytes) |
|---|---|
| TransferFeeConfig | 278 |
| MintCloseAuthority | 202 |
| ConfidentialTransferMint | 235 |
| DefaultAccountState | 171 |
| NonTransferable | 170 |
| InterestBearingConfig | 222 |
| PermanentDelegate | 202 |
| TransferHook | 234 |
| ConfidentialTransferFeeConfig | 299 |
| MetadataPointer | 234 |
| GroupPointer | 234 |
| TokenGroup | 250 |
| GroupMemberPointer | 234 |
| TokenGroupMember | 242 |
| ScaledUiAmount | 226 |
| Pausable | 203 |
| PermissionedBurn | 202 (not in interface 2.1.0 or Token-2022 10.0.0) |
| TokenMetadata | variable; Token-2022 reallocates it at initialization |

**Token account with a single extension.** A bare token account is 165 bytes; the same formula applies.

| Account extension | Account size (bytes) |
|---|---|
| TransferFeeAmount | 178 |
| ImmutableOwner | 170 |
| MemoTransfer | 171 |
| CpiGuard | 171 |
| NonTransferableAccount | 170 |
| TransferHookAccount | 171 |
| PausableAccount | 170 |
| ConfidentialTransferAccount | 465 |
| ConfidentialTransferFeeAmount | 234 |

**Ordering rules.** All mint extensions except TokenMetadata, TokenGroup and TokenGroupMember must be initialized after `create_account` and before `InitializeMint2`. Breaking this rule fails with `InvalidAccountData` in each of these cases [probe]:
- initializing the extension after `InitializeMint2`;
- running the extension's initializer on an 82-byte account;
- reserving space for an extension but never initializing it.

## First scenario set for ExtensionLab (proposed)

**P0.** Each scenario runs the user's real deposit, withdraw and refund flow against a controlled mint and reports the failing instruction and its logs, expected vs actual balances, the broken assumption, and a regression test.

| ID | Setup | Expected failure or broken assumption | Needs |
|---|---|---|---|
| TF-01 | 100 bps fee mint; deposit, then full withdraw | Credited 10,000 but vault holds 9,900; last withdrawal fails with `InsufficientFunds` | nothing new |
| TF-02 | Fee mint; empty the vault, then close it | `AccountHasWithheldTransferFees`, until a harvest | nothing new |
| TF-03 | Fee raised mid-escrow; move the clock 2 epochs | Payout differs from quote; `FeeMismatch` if `transfer_checked_with_fee` is used | Clock epoch warp |
| HOOK-01 | Mint with an always-allow hook; protocol CPI without extra accounts | `MissingAccount` | a hook `.so` (to build) |
| HOOK-02 | Mint whose hook is `None` at deposit and switched on before withdraw | Withdrawal fails | a hook `.so` (to build) |
| HOOK-03 | Deny-all or deny-recipient hook | Withdraw or refund reverts with the hook's error | a hook `.so` (to build) |
| PD-01 | Mint with a permanent delegate; delegate drains the vault after deposits | Liabilities exceed vault balance; every later withdrawal fails | nothing new |
| PAUSE-01 | Pausable mint paused during the refund window | `MintPaused`; timeout logic misfires | nothing new |
| DAS-01 | Frozen-by-default mint; program creates the vault and deposits | `AccountFrozen` | nothing new |
| DAS-02 | Freeze authority freezes the vault after deposits | Withdrawal fails with `AccountFrozen` | nothing new |
| MEMO-01 | User enables required memos on their own account; protocol refunds | `NoMemo` | nothing new |
| CPIG-01 | User's source account has CPI Guard on; protocol CPI deposit signed by the user | `CpiGuardTransferBlocked` | the user's program |

**P1**
- MCA-01: resurrected mint (frozen-default bypass, stale vault).
- IO-01: escrow that uses `SetAuthority(AccountOwner)`.
- NT-01: a non-transferable mint should be rejected when listed.
- UI-01: interest-bearing or scaled UI amount, comparing UI-based and raw-based accounting.

**P2**
- CT-01: recipient with public credits disabled.
- MD-01: spoofed metadata pointer.
- GRP-01: group size limit.
- PB-01: burn-based redemption.

**Combination scenarios to add after P0 [unverified priority].**
- TransferFee plus TransferHook plus Pausable on one mint, as on real stablecoin-style mints.
- The PYUSD-like "all switches dormant" mint, then flipping one switch at a time.

## Local references (owner code, read-only)

Paths are relative to the repo root. The code uses Anchor 1.2.0, Solana 3.1.10, `litesvm` 0.16.0 and `spl-token-2022-interface` 3.1.

- `reference/turbin3_q3_26/token22-q326/programs/t22/src/instructions/mint.rs`:
  - manual three-phase mint creation (`create_account`, then the extension initializers, then `initialize_mint2`);
  - `try_calculate_account_len` sizing;
  - extension allowlist validation;
  - reading the live epoch fee.
- `reference/turbin3_q3_26/token22-q326/programs/t22/src/instructions/remittance.rs`:
  - a mint combining TransferFee, MetadataPointer, DefaultAccountState(Frozen) and MintCloseAuthority;
  - a seven-extension mint that adds ConfidentialTransfer, ConfidentialTransferFee and PermanentDelegate;
  - `transfer_checked_with_fee` through CPI.
- `reference/turbin3_q3_26/token22-q326/programs/t22/tests/test_initialize.rs`: mint sizes (270 bytes for MintCloseAuthority plus MetadataPointer), TLV order, and the forged-mint owner check.
- `reference/turbin3_q3_26/token22-q326/programs/t22/tests/authority.rs`: CPI Guard through a CPI, permanent delegate seizure, and the frozen-by-default remittance lifecycle.
- `reference/turbin3_q3_26/token22-q326/programs/t22/tests/confidential.rs`: the full confidential-transfer lifecycle in LiteSVM, with proofs.

## Sources

**Official documentation**
- Token-2022 extensions guide: https://www.solana-program.com/docs/token-2022/extensions
- Token-2022 on-chain program guide: https://www.solana-program.com/docs/token-2022/onchain
- Transfer hook interface specification: https://www.solana-program.com/docs/transfer-hook-interface/specification
- Transfer hook extra accounts: https://www.solana-program.com/docs/transfer-hook-interface/configuring-extra-accounts
- Confidential balances: https://www.solana-program.com/docs/confidential-balances
- solana.com transfer hook: https://solana.com/docs/tokens/extensions/transfer-hook
- solana.com permanent delegate: https://solana.com/docs/tokens/extensions/permanent-delegate
- solana.com pausable: https://solana.com/docs/tokens/extensions/pausable
- solana.com scaled UI amount: https://solana.com/docs/tokens/extensions/scaled-ui-amount
- solana.com scaled UI amount integration guide: https://solana.com/docs/tokens/extensions/scaled-ui-amount/integration-guide
- solana.com confidential transfer: https://solana.com/docs/tokens/extensions/confidential-transfer

**Source code**
- Token-2022 processor (commit `3d94b2f`): https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/processor.rs
- On-chain transfer helpers: https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/onchain.rs
- Memo check: https://github.com/solana-program/token-2022/blob/3d94b2fa6bf4369f4eb98ea69c188726b05ad64c/program/src/extension/memo_transfer/mod.rs
- Token-2022 releases: https://github.com/solana-program/token-2022/releases
- `TokenError` enum, interface 3.1.1: https://docs.rs/spl-token-2022-interface/3.1.1/spl_token_2022_interface/error/enum.TokenError.html

**Other**
- Neodyme Token-2022 security guide [secondary]: https://neodyme.io/en/blog/token-2022
- PYUSD mint, read on mainnet on 2026-10-21: https://explorer.solana.com/address/2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo
