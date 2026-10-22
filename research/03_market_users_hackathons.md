# ExtensionLab: market, competition, first users, hackathon and grant targets

Research date: 2026-10-21 (IST), with local execution corrections on 22 Oct. Nothing was executed against a chain. No issues were opened and nobody was contacted.

How to read the confidence labels:

- **[web]**: read on the primary page linked (official docs, the GitHub issue itself, the advisory itself).
- **[code]**: read in the repository source at the commit given. This means grep plus reading the relevant functions. It is not a full review.
- **[2nd]**: secondary source only (news site, aggregator, third-party guide).
- **[predicted]**: my inference from code plus documented Token-2022 behavior. ExtensionLab would have to run it to confirm.
- **[unverified]**: could not confirm.

---

## 0. Summary

1. **Nothing I found does exactly what ExtensionLab proposes.** I found no tool that runs a developer's own deposit, withdraw and refund flows against a ready-made catalogue of hostile Token-2022 configurations, then reports the broken assumption and exports a CI regression test. The closest is **Trident** (Ackee). Its fuzz API can create mints with 15 mint extensions and 3 account extensions, so a developer could build this by hand. It ships no scenario catalogue and no diagnosis, and its Token-2022 ATA bug is still open ([Trident docs source](https://github.com/Ackee-Blockchain/trident/blob/HEAD/documentation/docs/trident-api/token-2022.md), [Trident #385](https://github.com/Ackee-Blockchain/trident/issues/385)). [web]
2. **Large protocols build this in-house.** marginfi has a test helper with six extension presets. Kamino keeps a conditional extension allow-list and re-checks it on lending actions. Raydium, Orca and Meteora keep allow-lists plus manual review (registry PDA or Token Badge). Small and mid-size teams have none of this. [code], [web]
3. **The pain is current and documented in 2026.** Examples:
   - A Token-2022 transfer-fee bug in the Solana Foundation's Kora paymaster (GitHub Security Advisory, March 2026).
   - Jupiter quotes through Manifest that overstate output by the transfer fee (September 2026).
   - An Anchor `init` failure on mints that carry an extension newer than the pinned interface (October 2026).
   - Eleven Token-2022-related findings in a Cantina review of the Solana Foundation DvP program (May 2026).
   - Test tools themselves (LiteSVM, Surfpool) building Token-2022 accounts wrong (October 2026).

   All links are in section 2. [web]
4. **Product implication.** ExtensionLab must create every account through the real Token-2022 and Associated Token Account programs. It must not write raw account bytes. Two current tool bugs show that hand-built accounts give false results: [LiteSVM #440](https://github.com/LiteSVM/litesvm/issues/440) and [Surfpool #866](https://github.com/solana-foundation/surfpool/issues/866). [web]
5. **Hackathons and grants.**
   - Colosseum's fall 2026 event (Crypto World's Fair) closed submissions on 2026-10-12 and is in judging, so ExtensionLab cannot enter it.
   - No next Colosseum date is published.
   - The best open routes today are rolling: Solana Foundation grants (public goods, open source) and the Superteam-run Solana Foundation regional grants. The India grant gives up to $10,000 USDG and lists "Developer Tooling" as a theme.
   - Breakpoint 2026 runs 15–17 November in London, with a Hacker House on 1–12 November.

   Details are in section 4. [web]

---

## 1. Competitors and adjacent tools

### 1.1 Test and fuzz frameworks

| Tool | What it does | Runs protocol-specific flows against extension configs? | Gap ExtensionLab fills |
|---|---|---|---|
| **Trident** (Ackee Blockchain), 408 stars, latest release 0.12.0 (2025-11-27) | Rust fuzzing framework for Anchor programs. Developers write guided "flows" and invariants. Includes regression comparison of fuzz metrics. | **Partly.** Its Token-2022 API creates mints and accounts with extensions: TransferFeeConfig, MintCloseAuthority, InterestBearing, NonTransferable, PermanentDelegate, TransferHook, MetadataPointer, GroupPointer, GroupMemberPointer, ScaledUiAmount, Pausable, DefaultAccountState, TokenMetadata, TokenGroup and TokenGroupMember, plus the account extensions ImmutableOwner, MemoTransfer and CpiGuard. The developer must pick the configs and write the invariants. | No preset catalogue of breaking configs. No "expected vs actual balance plus broken assumption" report. Open bug: creating Token-2022 ATAs fails with `IncorrectProgramId` ([#385](https://github.com/Ackee-Blockchain/trident/issues/385), open since 2025-10-06). ExtensionLab could emit Trident flows rather than compete. Sources: [token-2022.md](https://github.com/Ackee-Blockchain/trident/blob/HEAD/documentation/docs/trident-api/token-2022.md), [repo](https://github.com/Ackee-Blockchain/trident). [web] |
| **LiteSVM**, 647 stars, active (pushed 2026-10-20) | In-process SVM for fast Rust/TS/Python tests. The `litesvm-token` crate has a `token-2022` feature. | **No.** It provides primitives only. | No extension scenarios. Its helper under-allocated Token-2022 accounts for mints with required account extensions until fixed on 2026-10-20 ([#440](https://github.com/LiteSVM/litesvm/issues/440)). LiteSVM is the natural execution backend for ExtensionLab. Sources: [Cargo.toml](https://github.com/LiteSVM/litesvm/blob/HEAD/crates/token/Cargo.toml), [Anchor testing docs](https://www.anchor-lang.com/docs/testing). [web] |
| **Mollusk** (Anza), 305 stars | Harness for testing single instructions. | No. A secondary source says it suits single-instruction unit tests without CPI or multi-step state ([ika docs](https://solana-pre-alpha.ika.xyz/testing/litesvm)) [2nd]. | Multi-step flows (deposit, then withdraw, then refund) are ExtensionLab's core, so Mollusk is not a fit as the main backend. [repo](https://github.com/anza-xyz/mollusk) |
| **Bankrun** | Node test framework. | No. | The README says deprecated in favor of LiteSVM ([README](https://github.com/kevinheavey/solana-bankrun)). [web] |
| **solana-program-test** | Legacy BanksClient harness. | No. | Not reviewed in depth. [unverified] |
| **Surfpool** (Solana Foundation), 615 stars | Local network with mainnet forking and "cheatcodes" to set balances. | **Indirectly.** You can fork real mints such as PYUSD and run flows. | Open bug: `surfnet_setTokenAccount` creates 165-byte Token-2022 accounts without the mint's required extensions. A balance override then "succeeds" while real transfers behave differently ([#866](https://github.com/solana-foundation/surfpool/issues/866), 2026-10-17). It has no scenario catalogue or diagnosis. [web] |
| **Crucible** (Asymmetric Research) via `anchor fuzz` | Coverage-guided, stateful invariant fuzzing (LibAFL/LiteSVM backend). | No Token-2022 mention in the docs. | ExtensionLab scenarios could be supplied as fuzz seeds. Sources: [Anchor fuzzing docs](https://www.anchor-lang.com/docs/testing/fuzzing), [repo](https://github.com/asymmetric-research/crucible). [web] |

### 1.2 Auditor tooling and knowledge

| Who | What exists | Runs your flows against extension configs? | Notes |
|---|---|---|---|
| **Sec3 X-ray** (open source) | LLVM-based static analyzer. | No. | Rule files in the repo tree are ArbitraryCPI, Break, CheckedDiv, CosplayAccountDetector, InsecurePDA, MaliciousSimulation, Overflow* and UntrustfulAccountDetector. None is named for Token-2022. I checked the file tree only, not the rule source ([repo](https://github.com/sec3-product/x-ray)). [web] |
| **Neodyme** | Blog post "SPL Token-2022: Don't shoot yourself in the foot with extensions" (2024-09-10). Also a Token-2022 audit (April 2024). | No. It is guidance. | Lists integration pitfalls: fee deducted from the recipient, accounts with `withheld_amount != 0` cannot be closed, hook mint validation, permanent delegate drain, CPI Guard requiring the delegate flow, memo-required reverts, default-frozen vaults, and mint close-and-reinit ([blog](https://neodyme.io/en/blog/token-2022/), [audit PDF](https://www.neodyme.io/reports/Token%202022%20-%202024.pdf)). [web] Good source for ExtensionLab's scenario list. |
| **Offside Labs** | "Token-2022 Security Best Practices", Part 1 (2024-09-19) and Part 2 (2024-11-04). | No. | Includes a worked vault example: a 20% fee leaves the vault 20 units short after one deposit and withdraw cycle. Also covers `calculate_fee` vs `calculate_inverse_fee` off-by-one, `maximum_fee` semantics, and fee updates taking effect after 2 epochs ([Part 2](https://blog.offside.io/p/token-2022-security-best-practices-part-2), [Part 1](https://blog.offside.io/p/token-2022-security-best-practices-part-1)). [web] |
| **OtterSec** | Audit firm, one of the firms that audited the Token Extensions program [2nd: solana.com FAQ heading, answer not rendered]. | No public Token-2022 integration tool found. | On 2026-10-21 the GitHub API resolved `coral-xyz/anchor` to `otter-sec/anchor`. I did not research what that means for maintainership. [unverified] |
| **Ackee** | Trident (above) and a Solana auditors' book chapter on Token-2022 ([book](https://ackee.xyz/solana/book/latest/chapter4/token-2022/)). | Partly, via Trident. | See Trident. |
| **Halborn, Zellic, Trail of Bits** | Audits of the Token-2022 program itself ([Halborn](https://www.halborn.com/blog/post/solana-token-ception-token-2022-bugfix-review), [Zellic finding](https://reports.zellic.io/publications/spl-token-2022/findings/low-confidential-transfer-extension-confidential-transfer-amounts-information-leak-via-transfer-fees), [Trail of Bits](https://trailofbits.com/library/token-2022-program/)). | No. | They audit the token program, not integrators. |

### 1.3 Token scanners (mint risk for traders)

| Tool | What it does | Relevance |
|---|---|---|
| **"SolStack scan"** | **Not found.** Two web searches returned nothing under that name. It may be a different spelling or an unindexed product. [unverified] | Ask the owner for a URL. |
| **GoPlus Solana token security API** | Returns fields such as `transfer_fee` (current and scheduled), `transfer_hook`, `transfer_hook_upgradable`, `none_transferable`, `default_account_state`, `freezable`, `closable` and `balance_mutable_authority` ([docs](https://docs.gopluslabs.io/reference/solanatokensecurityusingget)). [web] | It scores a **mint**. It does not test whether **your program** survives that mint. ExtensionLab could use the same field set to pick scenarios for a given real mint. |
| **RugCheck, Solscan, open-source scanners** (for example [awaping/solana-token-risk-scanner](https://github.com/awaping/solana-token-risk-scanner)) | Flag permanent delegate, hooks, fees, pausable and freeze authority for traders. Solscan displays extensions ([Solscan 2024](https://kb.solscan.io/solscan-2024-wrapped)). [2nd] | Same gap: they assess trader risk, not integration correctness. |

### 1.4 What wallets, DEXes, aggregators and lenders do today

| Protocol | Policy | Source |
|---|---|---|
| **Orca Whirlpools** | Token-2022 support since 2024-05-28. Supported without approval: TransferFee, InterestBearing, MemoTransfer, MetadataPointer, TokenMetadata, and ConfidentialTransfer (non-confidential transfers only). These need a **Token Badge**, granted case by case: PermanentDelegate, TransferHook, MintCloseAuthority, DefaultAccountState (must be Initialized), and freeze authority. Not supported: NonTransferable, Group/Member and all others. | [Orca docs](https://docs.orca.so/developers/architecture/token-extensions) [web] |
| **Raydium CPMM and CLMM** | `is_supported_mint` allow-list: TransferFeeConfig, MetadataPointer, TokenMetadata, InterestBearingConfig and ScaledUiAmount. It is checked only at pool creation. Hooks are excluded because they "could brick swaps". Permanent delegate is excluded because "a holder of the delegate can sweep any token account, including the pool vault". The hardcoded `MINT_WHITELIST` was deleted (CLMM 2026-07-24, CPMM 2026-09-09) and replaced by a per-mint registry PDA `[b"support_mint", mint]`. | [Raydium Token-2022 reference](https://docs.raydium.io/reference/token-2022-support) [web]. Some Raydium product pages contradict this ([search summary](https://docs.raydium.io/products/cpmm/overview.md)) [2nd]. |
| **Meteora DAMM v2** | Accepted without a badge: TransferFeeConfig, MetadataPointer, TokenMetadata, and TransferHook only when the hook program and authority are both unset. Everything else needs a badge. "DAMM v2 does not provide a general transfer-hook remaining-account surface", so an active hook can fail even with a badge. | [Meteora docs](https://docs.meteora.ag/core-products/damm-v2/token-2022-support) [web] |
| **Jupiter** | Limit Order V2 and DCA V2 support transfer-fee tokens. V1 of both did not. Spot shows a JupShield "Has Freeze Authority" warning. The page does not cover permanent delegate or hooks. | [Jupiter docs](https://docs.jup.ag/user-docs/trade/spot/risks-and-limitations) [web] |
| **Kamino klend** | In-code allow-list with conditions: transfer fee must be 0 in both `older` and `newer` fee; hook `program_id` must be unset; DefaultAccountState must be Initialized or Frozen; Pausable must not be paused. Re-checked across lending actions in `lending_checks.rs`. | [code] at `77e8185`, [repo](https://github.com/Kamino-Finance/klend) |
| **marginfi v2** | Test helper `SupportedExtension` with presets MintCloseAuthority, InterestBearing, PermanentDelegate, TransferHook, TransferFee and TransferFeeInactive (`test-utils/src/spl.rs`). | [code] at `aeafee7`, [repo](https://github.com/0dotxyz/marginfi-v2) |

### 1.5 Honest assessment

- **Does anything already do this?** Not as a product. A competent team can do it with Trident or LiteSVM plus hand-written extension setups, and marginfi did. The value ExtensionLab adds is packaging:
  - a curated, versioned catalogue of breaking configurations (including future or unknown extension tags);
  - "run your real flow" adapters;
  - diagnosis (failing transaction, logs, balance diff, the assumption that broke);
  - export of a regression test that runs in CI.
- **Main competitive risk:** Trident or LiteSVM adding presets. Mitigation: build on LiteSVM and treat Trident as an export target, not a rival.
- **Protocols with strict allow-lists** (Raydium, Orca, Meteora, Kamino) are less likely to need bug-finding. They might still want regression lock-in, or a "which real stablecoins does my allow-list accept?" report (see payment-channels #83 below).

---

## 2. Evidence of real pain (public, linked)

Newest first. All entries are [web] unless marked.

| Date | Where | What broke | Extension or cause | Link |
|---|---|---|---|---|
| 2026-10-19 | LiteSVM (test tool) | `CreateAccount` helper under-allocated Token-2022 accounts for TransferFee, TransferHook and NonTransferable mints, so ordinary test flows failed with `InvalidAccountData`. Fixed 2026-10-20. | Required account extensions | [LiteSVM #440](https://github.com/LiteSVM/litesvm/issues/440) |
| 2026-10-17 | Surfpool (test tool) | Cheatcode-created Token-2022 ATAs lacked `TransferFeeAmount` and `ImmutableOwner`. Inbound `transfer_checked` failed with error `0xd`, and `SetAuthority` succeeded where a real ATA rejects it. Open. | Required account extensions | [Surfpool #866](https://github.com/solana-foundation/surfpool/issues/866) |
| 2026-10-02 | Anchor | `#[account(init, token::mint = ..)]` fails with `InvalidAccountData` on mints carrying an extension newer than the interface pinned by `anchor-spl` 1.2.0 (PermissionedBurn, tag 28). Closed 2026-10-06. | Unknown or new extension tag | [otter-sec/anchor #5138](https://github.com/otter-sec/anchor/issues/5138) |
| 2026-09-24 | Jupiter Swap API and Manifest | The Manifest adapter's `quote()` ignored TransferFeeConfig, so quotes overstated `outAmount` by the fee. Consequences: reverts at tight slippage (`0x1771`), misrouting (−0.10% to −0.85% delivery on PreStocks mints), and integrators receiving about 1% less than quoted on 100 bps mints. The Manifest issue closed the same day; the Jupiter issue is open. | Transfer fee (off-chain quoting) | [jupiter-swap-api-client #65](https://github.com/jup-ag/jupiter-swap-api-client/issues/65), [manifest #735](https://github.com/Bonasa-Tech/manifest/issues/735) |
| 2026-08-17 | Solana Foundation payment-channels | A fixed allow-list rejects devnet USDG, devnet PYUSD and mainnet PYUSD, whose extensions include MintCloseAuthority, PermanentDelegate, TransferFeeConfig, ConfidentialTransfer*, and TransferHook. The error makes a valid mint look corrupted (`0x38`). Open. | Allow-list vs real stablecoins | [payment-channels #83](https://github.com/solana-foundation/payment-channels/issues/83) |
| 2026-05-21 to 2026-05-28 | Cantina review of the Solana Foundation DvP program | 11 Token-2022-related findings, including Medium "Closed unfunded mint can block funded-leg recovery", Medium "Memo-required ATAs can block DvP recovery", Low "NonTransferable mints can brick funded DvPs", Low "Destination-dependent TransferHook extras can block surplus settlement", and Informational "Mint authorities can freeze or claw back escrows". | MintCloseAuthority, MemoTransfer, NonTransferable, TransferHook, freeze and permanent delegate | [Cantina report](https://cantina.xyz/portfolio/fe870bb4-d96d-4902-8aec-9dfcfa2d6a79) |
| 2026-04-14 | Token-2022 program | Report that a self-transfer returns before the transfer hook runs, so hook-based compliance can be bypassed. Closed as completed 2026-04-29; I did not verify the resolution. | Transfer hook | [token-2022 #1109](https://github.com/solana-program/token-2022/issues/1109) |
| 2026-03-12 | Solana Foundation Kora (paymaster) | GHSA-725g-w329-g7qr: `verify_token_payment()` credited the full raw amount for TransferFeeConfig tokens, so the paymaster received less than credited. Affects all versions before 2.0.5; fixed in 2.0.5. | Transfer fee | [GitHub advisory](https://github.com/solana-foundation/kora/security/advisories/GHSA-725g-w329-g7qr), [GitLab mirror](https://advisories.gitlab.com/cargo/kora-lib/GHSA-725g-w329-g7qr/), [fix commit](https://github.com/solana-foundation/kora/commit/8cbd8217ee505e6b37c63ef835ff095cfa8ab318) |
| 2025-12-22 | Raydium CPMM | A launchpad builder asks for transfer-hook support. Raydium's allow-list blocks hook mints even when the hook is used only before migration. Open. | Transfer hook | [raydium-cp-swap #64](https://github.com/raydium-io/raydium-cp-swap/issues/64) |
| 2025-10-06 | Trident (test tool) | Cannot create Token-2022 ATAs (`IncorrectProgramId`). Open. | Token-2022 ATA | [Trident #385](https://github.com/Ackee-Blockchain/trident/issues/385) |
| 2025-10-05 | Orca TS SDK | Could not open positions or harvest in a pool where one token uses Token-2022 (account decode error). Closed 2025-11-01. | Token-2022 integration (SDK) | [whirlpools #1091](https://github.com/orca-so/whirlpools/issues/1091) |
| 2025-09-24 | Hummingbot Gateway | Meteora `/pool-info` threw `TokenInvalidAccountOwnerError` on Token-2022 pools. Closed 2025-10-30. | Token-2022 program ID | [gateway #519](https://github.com/hummingbot/gateway/issues/519) |
| 2025-05 | CodeHawks "SSSwap" First Flight | Valid Low finding: "Missing Token-2022 Transfer Fee Handling". `transfer_checked` with the full amount and no fee adjustment, so vault balances drift. | Transfer fee | [CodeHawks finding](https://codehawks.cyfrin.io/c/2025-05-ssswap/s/64) |
| 2025-03-29 | Jupiter Metis binary | Quotes for all tokens broke via Meteora right after Meteora added Token-2022 support. Closed 2025-04-02. | Token-2022 rollout regression | [metis-binary #89](https://github.com/jup-ag/metis-binary/issues/89) |
| 2024-11-14 | Jito Restaking (Immunefi audit competition) | Insight: the vault program hardcoded `spl_token::id()`, so Token-2022 vaults could not initialize or accept deposits. | Program ID assumption | [Immunefi #36787](https://reports.immunefi.com/jito-restaking/36787-sc-insight-the-vault-program-dont-support-token2022-transfer) |
| 2024-09-24 | Sherlock, WOOFi Solana deployment | Medium (sponsor disputed, won't fix): `#[account(address = token::ID)]` excludes Token-2022. | Program ID assumption | [Sherlock judging #52](https://github.com/sherlock-audit/2024-08-woofi-solana-deployment-judging/issues/52) |
| 2024-09-04 | Mainnet scam (reported by Cointelegraph) | The "RED" token used a permanent delegate to burn buyers' tokens 7 seconds after purchase. Reported by Jupiter Core Working Group member Slorg on 2024-09-03. | Permanent delegate | [Cointelegraph](https://cointelegraph.com/news/new-solana-scam-burn-tokens-from-inside-sol-wallets) |
| 2024-07-10 | SPL `invoke_transfer_checked` | Out-of-memory when a program does 2 hook transfers whose hooks need more than about 6 extra account metas. Open. | Transfer hook | [SPL #7004](https://github.com/solana-labs/solana-program-library/issues/7004) |
| 2022-10-31 | Token-2022 (pre-release) | Removing the freeze authority prevented disabling default-frozen accounts. Fixed. | DefaultAccountState | [SPL #3789](https://github.com/solana-labs/solana-program-library/issues/3789) |

More context:

- Training audit competitions keep recording the same class of bug, for example [Solana-Audit-Arena #245](https://github.com/Frankcastleauditor/Solana-Audit-Arena/issues/245) (permanent delegate drains vault custody) and [#260](https://github.com/Frankcastleauditor/Solana-Audit-Arena/issues/260) (transfer fee not anticipated). These are training findings, not production incidents. [web]
- Official Solana docs say a client that does not resolve hook extra accounts "can't send the token at all", and that a stale `ExtraAccountMetaList` fails "with an account-validation error, not a hook-logic error" ([transfer hook integration](https://solana.com/docs/tokens/extensions/transfer-hook-integration)). [web]

**Failure patterns ExtensionLab should cover first.** These are derived from the evidence above:

1. Transfer-fee accounting: recorded amount differs from the vault balance (Kora, SSSwap, Offside vault example).
2. Closing an escrow or vault account with `withheld_amount > 0` fails, which bricks take and refund paths. Neodyme documents this, and Train Protocol's code comments describe it as a real concern.
3. Transfer hook extra accounts missing, stale, or too many (SPL #7004, Cantina DvP).
4. Memo-required destination accounts (Cantina DvP).
5. Frozen default state, freeze, Pausable, and permanent delegate clawback of escrow (Cantina DvP, Neodyme).
6. Mint close and re-init (Cantina DvP, Neodyme).
7. Allow-lists that reject real stablecoins, or fail on unknown future extension tags (payment-channels #83, Anchor #5138).
8. Off-chain quote and SDK paths that ignore fees (Jupiter and Manifest).
9. Fee changes that take effect two epochs later (Offside).

---

## 3. First users: open-source programs that accept arbitrary or Token-2022 mints

How each was selected: it uses `TokenInterface` / `InterfaceAccount` or the Token-2022 program and appears active in 2026. "Activity" means GitHub stars plus the last commit on the default branch, checked 2026-10-21. "Likely to break" is **[predicted]** unless it says [code] or [observed local, 22 Oct]. Reproduction commands and pinned runtime details for observed results are in README.

| # | Repository | Type | Activity | Flow ExtensionLab would test | What the code does today, and what likely breaks |
|---|---|---|---|---|---|
| 1 | [solana-foundation/program-examples](https://github.com/solana-foundation/program-examples) `tokens/escrow/anchor` | Canonical escrow example | 1,436 stars; last commit 2026-09-28 (`de09019`) | `make_offer` → `take_offer` / `refund_offer` | [code] Deposits `token_a_offered_amount` with plain `transfer_checked`. Take and refund send `vault.amount`, then call `close_account`. [predicted] With a transfer-fee mint, the vault keeps withheld fees, so `close_account` fails and both take and refund revert until someone harvests. The taker also gets less than offered. A hook mint fails because no extra accounts are passed. A memo-required or default-frozen ATA also fails. **Best public demo target:** widely copied by learners, no outreach needed. |
| 2 | [sablier-labs/solsab](https://github.com/sablier-labs/solsab) | Token streams (lockup) and Merkle airdrops | 10 stars; last commit 2026-10-07 (`e060b26`) | `create_with_timestamps` → `withdraw` → `cancel`; `create_campaign` → `claim` → `clawback` | [code] Stores `deposit_amount` from instruction arguments. No fee or hook handling found by grep. [predicted] Fee mint: recorded deposit exceeds the vault balance, so the last withdraw or cancel refund comes up short. Hook mint: transfers fail. Permanent delegate: vault can be drained. Pausable: withdraws blocked. |
| 3 | [1inch/solana-fusion-protocol](https://github.com/1inch/solana-fusion-protocol) | Intent / limit-order escrow | 6 stars; last commit 2026-09-09 (`2aab60c`) | `create` → partial and full `fill` → `cancel` | [code] Deposits `order.src_amount`. Fill amounts are priced off `src_amount`. Closes the escrow ATA when it is drained. [predicted] Fee mint: escrow holds less than `src_amount`, so the final fill cannot complete as priced and `close_account` fails on withheld fees. Hook mint fails. Whether 1inch restricts mints off-chain is unverified. |
| 4 | [1inch/solana-crosschain-protocol](https://github.com/1inch/solana-crosschain-protocol) | Cross-chain escrow (src and dst) | 3 stars; last commit 2026-10-19 (`b82c282`) | create escrow → withdraw with secret → cancel / public cancel | [code] Uses `TokenInterface`; no fee or hook handling found by grep. [predicted] Same failure class as #3. |
| 5 | [TrainProtocol/contracts](https://github.com/TrainProtocol/contracts) `chains/solana/programs/train-htlc` | HTLC atomic swaps | 30 stars; last commit 2026-07-31 (`3dbda69`) | lock → redeem / expired refund | [code] Measures the amount actually received and rejects PermanentDelegate and TransferHook mints. Attempts to catch a failed vault close. [observed local, 22 Oct] That catch does not recover: a failed CloseAccount CPI aborts the transaction (35), rolling back both fee-bearing exits. Real pre-harvest makes both net-accounted exits pass. The earlier prediction of close-error tolerance was wrong. Fee changes between lock and redeem remain untested. |
| 6 | [tributary-so/tributary](https://github.com/tributary-so/tributary) | Subscriptions and delegated pull payments | 12 stars; last commit 2026-09-28 (`4b0a32e`) | `create_payment_policy` → `execute_payment` (delegate pull) | [code] A deny-list of 6 extensions (TransferHook, ConfidentialTransferMint, NonTransferable, PermanentDelegate, TransferFeeConfig, MintCloseAuthority), re-checked at execution. [predicted] Not covered: Pausable, DefaultAccountState, a memo-required merchant account, unknown future tags. Rejecting any TransferFeeConfig, even 0 bps, would exclude PYUSD-style mints, which carry TransferFeeConfig according to [payment-channels #83](https://github.com/solana-foundation/payment-channels/issues/83). |
| 7 | [solana-foundation/vault](https://github.com/solana-foundation/vault) (`async_vault`) | Vault factory with deposit and redemption queues | 20 stars; last commit 2026-10-01 (`c359962`) | `create_vault` → deposit request → claim → redemption request | [code] Requires the asset mint's fee at the **current epoch** to be 0, checked at create and deposit. The share mint rejects ConfidentialMintBurn. No checks for asset-mint hook, permanent delegate or Pausable were found by grep. [predicted] A fee scheduled in `newer_transfer_fee` passes now and applies to later redemptions; Kamino checks both older and newer fees. Hook, pause and permanent-delegate scenarios are untested. |
| 8 | [solana-foundation/payment-channels](https://github.com/solana-foundation/payment-channels) | Agent payment channels (x402/MPP) | 10 stars; last commit 2026-08-07 (`3ffa4d6`) | `open` → `top_up` → settle / close / `reclaim` | [code] Strict allow-list (MetadataPointer, TokenMetadata). [web] Real PYUSD and USDG are rejected with a misleading error (#83, open). Value: a "which real mints does my policy accept, and with what error" report, plus regression tests. |
| 9 | [solana-foundation/solana-private-channels](https://github.com/solana-foundation/solana-private-channels) (DvP and escrow programs) | Delivery-versus-payment escrow | 68 stars; last commit 2026-10-20 (`effd1fb`) | fund legs → settle → recovery / cancel | [web] Cantina found 11 Token-2022-related issues, 5 of them "Acknowledged" rather than fixed. [code] The escrow program tracks `ISSUER_ADDABLE_EXTENSIONS`. Value: turn the audit findings into CI regression tests. |
| 10 | [Squads-Protocol/v4](https://github.com/Squads-Protocol/v4) | Multisig treasury | 202 stars; last commit 2026-08-20 (`af94153`) | `spending_limit_use` (treasury payouts within a limit) | [code] `transfer_checked` with no remaining accounts for hooks. `memo` is an argument; I found no Memo instruction CPI. [predicted] Hook mints fail. Memo-required destinations fail. With a fee mint the recipient gets less than the amount counted against the limit. Arbitrary vault transactions are not affected. |
| 11 | [Squads-Protocol/smart-account-program](https://github.com/Squads-Protocol/smart-account-program) | Smart accounts with policy engine | 44 stars; last commit 2026-08-31 (`0e51082`) | spending-limit policy and internal fund transfer | [code] Pre- and post-balance tracking enforces spending limits. [predicted] Test fee mints (does the balance delta match the limit?) and a permanent-delegate drain (does a balance drop not initiated by the account consume the limit?). |
| 12 | [MeteoraAg/dynamic-bonding-curve](https://github.com/MeteoraAg/dynamic-bonding-curve) | Launchpad bonding curve | 99 stars; last commit 2026-10-16 (`91e696e`) | create config and pool → swap → curve complete → migrate | [code] Explicit TransferHook paths (`create_config_with_transfer_hook`, hook events, missing-remaining-account errors) and transfer-fee handling. [predicted] Test hooks that need several extra accounts across swap and migration, and fee changes mid-curve. Mature team, so lower bug odds. |
| 13 | [0dotxyz/marginfi-v2](https://github.com/0dotxyz/marginfi-v2) | Lending | 303 stars; last commit 2026-10-14 (`aeafee7`) | deposit → borrow → repay → withdraw → liquidate | [code] Has its own extension test presets (see 1.4). Value: benchmark ExtensionLab's catalogue against theirs. Possible source of scenario contributions. Unlikely to adopt early. |
| 14 | [Kamino-Finance/klend](https://github.com/Kamino-Finance/klend) | Lending | 183 stars; last commit 2026-10-19 (`77e8185`) | reserve init → deposit → borrow → withdraw | [code] A conditional allow-list re-checked across lending actions. [predicted] Value: prove it fails closed after the fee authority turns on a fee, the hook authority sets a program, or the issuer pauses. |
| 15 | [wormhole-foundation/native-token-transfers](https://github.com/wormhole-foundation/native-token-transfers) | Bridge (lock or burn) | 47 stars; last commit 2026-10-15 (`15d54ec`) | `transfer` (lock or burn) → `release_inbound` | [code] Comment: "we currently do not support tokens with fees". Measures custody before and after transfer, and supports transfer hooks (a `dummy-transfer-hook` program exists). [predicted] Test that fee mints revert cleanly, plus hook extra-meta resolution on inbound release. |

**Considered but excluded:**

- **[metaDAOproject/programs](https://github.com/metaDAOproject/programs)** (`56a23ae`). The v0.8 launchpad `settle_launch` uses `Account<Mint>` and `Program<Token>`, which means classic SPL Token only on that path. [code] Out of scope unless they move to Token-2022.
- **Streamflow and Jupiter Lock.** I could not find their on-chain program repos on 2026-10-21: `streamflow-finance/protocol` and `jup-ag/jup-lock` returned 404. Streamflow's [JS SDK](https://github.com/streamflow-finance/js-sdk) is public. [unverified] The programs may be closed source or hosted elsewhere.

**Suggested order** (no contact made):

1. Use the program-examples escrow as the public demo. Its predicted bricking with a transfer-fee mint is a clear, reproducible story once ExtensionLab runs it.
2. Solana Foundation repos (payment-channels, vault, DvP). They already track Token-2022 issues publicly.
3. Small active teams where a regression suite is cheap to show: Tributary, Train, Sablier, 1inch.

---

## 4. Hackathons and grants (status as of 2026-10-21)

| Program | Dates and deadline | Status today | Fit for ExtensionLab | Judging / evaluation criteria | Source |
|---|---|---|---|---|---|
| **Colosseum Crypto World's Fair** (first Colosseum event open to all chains) | Kickoff 2026-09-15. "Submissions due October 12, 2026". Winner date not stated. | **Closed, in judging.** | Too late. | The page lists no criteria. "The Colosseum team reviews all product submissions and determines the overall hackathon winners"; track judges review the ecosystem tracks. Prizes: $840,000 in prizes plus $2.5M seed funding; Grand Prize $30k; 20 × $15k; Public Good $5k; University $5k; Solana track 10 × $10k; accelerator $250k pre-seed. | [colosseum.com/worldsfair](https://colosseum.com/worldsfair), [Expanding the Arena](https://blog.colosseum.com/expanding-the-arena/) [web] |
| **Colosseum schedule conflict** | An early-2026 announcement listed a second 2026 hackathon for Sep 28 – Nov 2. | **Superseded.** The live site shows Crypto World's Fair (Sep 14 – Oct 12). | — | — | [Colosseum Codex, 2026 hackathons](https://blog.colosseum.com/2026-hackathons-updraft-course-offline-signer-cli/) (title only), [X post](https://x.com/venture_launch/status/2011382696942182786) [2nd] |
| **Next Colosseum hackathon** | **Not announced.** The blog index up to 2026-10-16 lists no future dates. Colosseum ran spring events in April–May 2025 (Breakout) and 2026-04-06 to 2026-05-11 (Frontier), so an April 2027 event is plausible. That is inference only. | Unknown | Best main target if it happens. Enter as a public-good or developer-tooling project. | Colosseum's guide: judged "just on the work completed during the hackathon"; prizes favor "teams who intend to build full-time and develop products with potentially viable business models", but "there will always be an award for the best public good"; pitch under 3 minutes covering team, product, market, usage and demo. | [How to Win (2024-02-20)](https://blog.colosseum.com/how-to-win-a-colosseum-hackathon/), [Frontier announcement](https://blog.colosseum.com/announcing-the-solana-frontier-hackathon/), [blog index](https://blog.colosseum.com/) [web] |
| **Colosseum Eternal** | Rolling 4-week sprints between main hackathons. | **Paused.** "New Eternal sprints are closed." | Later, if it reopens. | Founder + Market Fit, Insight, Product + Execution, Potential Market Size, Founder Communication, Viability. $25k award "roughly every six months"; the team must not have raised VC. | [colosseum.com/eternal](https://colosseum.com/eternal) [web] |
| **Solana Foundation grants** | Rolling. Review about 1 week, decision about 3 weeks. | **Open.** | **Strong fit:** open-source developer tooling is a public good. | Public good ("makes a significant open-source contribution to the Solana ecosystem"), open source, only possible on Solana, clear use of funds with measurable milestones. Types: milestone grants, convertible grants, RFPs. | [solana.org/grants-funding](https://solana.org/grants-funding) [web] |
| **Solana Foundation India Grants** (run by Superteam India) | Rolling. Created 2026-01-01; status OPEN; average response "30 Days". | **Open.** India-based builders only. | **Strong fit if the applicant is India-based.** "Developer Tooling" is a listed theme. | Up to $10,000 USDG. They want a working prototype or product, proof of work (bounties, hackathons, open source), and user research. "Proof-of-Work > Potential"; "Working Product > MVP >>> PRDs > Pitch Decks". They do not want idea-only or landing-page-only applications. Applications ask for a Loom pitch video and a breakdown of fund use. | [Superteam Earn grants API record](https://superteam.fun/api/grants/solana-foundation-india-grants), [grants page](https://superteam.fun/earn/grants) [web] |
| **Other Solana Foundation regional grants via Superteam** | Rolling, up to $10,000 USDG each: USA, UK, Ireland, Germany, Netherlands, Poland, Balkan, Ukraine, Turkey, Georgia, Kazakhstan, UAE, Singapore, Malaysia, Korea, Australia, Brazil, Nigeria, Canada, plus La Familia (Spain). | Listed as live. | Depends on region. | Each listing has its own text; only India was read in full. | [Superteam grants API](https://superteam.fun/api/grants) [web] |
| **Superteam Agentic Engineering Grants** | Rolling, global. Created 2026-03-04; status OPEN. | Open | Small. Covers one month of AI coding tools. | $200 USDG, 50% upfront and 50% after shipping. "should be a working Solana product". | [API record](https://superteam.fun/api/grants/agentic-engineering) [web] |
| **Breakpoint 2026** | 2026-11-15 to 2026-11-17, Olympia, London. | Upcoming | Distribution and meetings, not a competition. | No official Breakpoint hackathon found. | [Solana Foundation Breakpoint guide](https://solana.com/news/breakpoint-guide-part1) [web] |
| **Solana Hacker House London** | 2026-11-01 to 2026-11-12, 10:00–20:00 daily. | Upcoming; register on Luma. | Good for building in person and meeting integrators. | Not described as a competition. | [Breakpoint guide](https://solana.com/news/breakpoint-guide-part1), [luma.com/london-hh-26](https://luma.com/london-hh-26) [web] |
| **Superteam "Next Stop Breakpoint"** | "Submissions Open Soon". No dates published. | Not open yet | Low. Prizes are Breakpoint tickets, not cash, and winners pay their own travel. | Not published. | [superteam.fun/earn/next-stop-breakpoint](https://superteam.fun/earn/next-stop-breakpoint) [web] |
| **Superteam UK "Build for Breakpoint: Agentic Grants"** | Live listing; no deadline field shown. | Listed | Low | $200 USDG | [Superteam grants API](https://superteam.fun/api/grants) [web] |
| **Solana Foundation RFP forum** | No open 2026 RFP for testing or security tooling found. Past related RFPs: "Pre-Deployment Program Analysis" (fuzzing and formal verification, deadline 2024-02-29) and "Test Validator Plugin Framework" (2023). | None relevant open | Shows prior Foundation interest in testing tooling. Worth citing in a grant application. | — | [RFP category](https://forum.solana.com/c/rfp/10), [Pre-Deployment Program Analysis](https://forum.solana.com/t/pre-deployment-program-analysis/1030) [web] |
| **ETHGlobal** | Upcoming: ETHGlobal Mumbai 2026-11-05 to 2026-11-07 (in person). Next listed: ETHConf June 2027. | Upcoming | **Not a fit.** No upcoming ETHGlobal event mentions Solana. | — | [ethglobal.com/events](https://ethglobal.com/events) [web] |

**Recommendations for the owner to decide:**

1. **Now:** prepare one strong demo. ExtensionLab running the program-examples escrow against a transfer-fee mint, showing the bricked refund plus the exported regression test, is the "proof of work" that both Superteam India and the Solana Foundation explicitly ask for.
2. **Then:** apply for the Solana Foundation grant as an open-source public good, and/or the Solana Foundation India grant if eligible, with milestone-based scope:
   - scenario catalogue v1;
   - adapters for Anchor plus LiteSVM;
   - CI export;
   - 3 public case studies.
3. **Watch:** Colosseum's next hackathon (not announced) and the London Hacker House on 1–12 November.

---

## 5. Unverified items and open questions

- "SolStack scan" was not found under that name.
- I did not check what the `coral-xyz/anchor` → `otter-sec/anchor` GitHub redirect means for Anchor maintainership.
- Every "[predicted]" break in section 3 must be confirmed by running ExtensionLab. In particular:
  - the program-examples escrow `close_account` failure with withheld fees;
  - Sablier and 1inch fee shortfalls;
  - Squads v4 hook failure;
  - SF vault behavior with a scheduled fee.
- Two checks rest on documented Token-2022 behavior that has not been run here:
  - that `close_account` rejects accounts with withheld fees (Neodyme states it; Train Protocol's code comment names the error `AccountHasWithheldTransferFees`);
  - that fees are withheld in the destination account.
- The resolution of token-2022 #1109 (self-transfer skipping the hook) was not checked.
- Raydium docs contradict each other on permanent delegate. The reference page (allow-list excludes it) is treated as authoritative here.
- Colosseum: the Crypto World's Fair winner announcement date and the next hackathon dates are not published.
- Superteam regional grant terms were read in full only for India and Agentic Engineering.
- Superteam's World's Fair sidetrack list (37 listings, including a Superteam India track at 5,000 USDG) showed no deadlines in the API. They are presumably closed with the main event; not verified.
- RugCheck's exact extension flags were seen only in secondary sources.
