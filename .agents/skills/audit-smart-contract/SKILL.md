---
name: audit-smart-contract
description: Audit a smart contract, blockchain/DLT node/VM/compiler, or Web3 application end to end and produce a submittable Markdown report with a working proof-of-concept exploit. Trained on all 3,797 disclosed Immunefi Audit Competition & Attackathon reports (2,179 Medium/High/Critical + 1,618 Low/Insight across 56 competitions). Covers EVM (Solidity/Vyper/Yul), Move (Aptos/Sui/Movement), Fuel (Sway/FuelVM), SVM (Solana/Rust/Firedancer), Starknet (Cairo), Stacks (Clarity), Algorand (PyTeal/AVM), Chia (Chialisp), CosmWasm, Substrate/Rust L1s, OP-Stack/ZK Rollups, and Web3 dApps/Wallets against a LOCAL FORK, testnet, or devnet only. Never broadcasts to mainnet.
metadata:
  version: "2.0"
  status: prototype
  scope: requires-maintainer-decision
  corpus: "reports.immunefi.com (3,797 reports across 56 competitions)"
---

# Audit a Smart Contract, Blockchain/DLT Protocol, or Web3 Target (Immunefi Corpus Edition)

Drive an authorized security audit of a smart contract, blockchain/DLT client/VM/compiler, or Web3 application from intake to a submittable, triage-proof Markdown report with a confirmed proof-of-concept (PoC) exploit.

This skill encodes the root-cause vulnerability patterns, mathematical failure modes, state-machine invariants, cross-VM pitfalls, severity-escalation techniques, and PoC engineering standards distilled from **all 3,797 disclosed vulnerability reports on `https://reports.immunefi.com/`** across **56 competitions** (**683 Critical**, **840 High**, **656 Medium**, **831 Low**, and **786 Insight** reports spanning Smart Contracts (`2,587`), Blockchain/DLT (`616`), and Websites & Applications (`593`)).

> **Status: prototype.** This skill composes execution, exploitation, and on-the-fly tool installation. Per [AGENTS.md](../../../AGENTS.md) those are scope-gated capabilities that require a maintainer decision before they are integrated into REA core. This skill drives a **disposable sandbox** and REA's existing read-only analysis tools; it does not modify REA's `src/` core, and it does not install anything onto the host. Treat it as a draft workflow pending that decision.

---

## Companion Deep-Dive Reference Playbooks

Always load the matching reference playbook(s) for your target's domain before beginning Stage 1 (Recon) and Stage 2/3/4/5 (Vulnerability Hunting & PoC):

1. **[references/safety.md](references/safety.md)** — Mandatory mainnet guard, authorization verification, sandbox isolation, and secret handling rules. Read before any execution step.
2. **[references/pipeline.md](references/pipeline.md)** & **[references/chains.md](references/chains.md)** — Stage-by-stage execution pipeline and per-chain sandbox toolchains.
3. **[references/immunefi-evm-playbook.md](references/immunefi-evm-playbook.md)** — Deep synthesis of **1,176 EVM Medium/High/Critical reports**: 12 root-cause bug classes, exact Solidity code smells, protocol playbooks (Lending/CDPs/Synthetics, ERC-4626/Async Vaults, Ve-Tokenomics/Gauges/Bribes, Staking/Restaking/Validators, Bridges/FAssets/CCIP/DEX Wrappers), and Foundry PoC templates.
4. **[references/immunefi-non-evm-playbook.md](references/immunefi-non-evm-playbook.md)** — Deep synthesis of **139 Non-EVM Smart Contract Medium/High/Critical reports**: Move (Aptos/Movement), Fuel/Sway (FuelVM), Solana/SVM (Rust), Starknet/Cairo (`felt252`), Stacks/Clarity, Algorand/PyTeal (AVM), Chia/Chialisp (CLVM), and the **7 Cross-VM Mental Model Failures**.
5. **[references/immunefi-blockchain-dlt-playbook.md](references/immunefi-blockchain-dlt-playbook.md)** — Deep synthesis of **403 Blockchain/DLT Medium/High/Critical reports**: P2P/Gossip pre-validation decompression & OOM bombs, wire-cast panics (Rust/Nim/Java/TS/C), non-yielding async spin loops, opaque `BoundedVec<u8>` recursion bypass, cache-vs-uncached consensus splits, ZK-prover vs. canonical node divergence, compiler IR/DCE miscompilations, C validator tile heap overflows, threshold crypto (WSTS nonce reuse, ML-DSA FIPS 204 domain separation), and reverted bridge `MessageOut` leaks.
6. **[references/immunefi-web3-apps-and-triage.md](references/immunefi-web3-apps-and-triage.md)** — Deep synthesis of **461 Web3 Application Medium/High/Critical reports** + **1,618 Low/Insight reports**: Displayed-vs-signed divergence, ENSIP-15 normalization splits, `[<64-hex>]` labelhash escalation, SCALE runtime-vs-SDK width mismatches, EAC/PermissionedResolver role-scope collapse, V1-to-V2 migration backdoors, HCA/ERC-1271 session bypasses, **5-Step Severity Escalation Playbook**, and **Triage-Proof PoC Engineering**.

---

## Non-Negotiable Safety Rules

Read [references/safety.md](references/safety.md) before any execution step.

1. **No mainnet.** Never broadcast a transaction to a mainnet RPC or mainnet chain-id/cluster. Forking mainnet **state** into a local node (`anvil --fork-url`, local validator) is allowed and expected; broadcasting to mainnet is forbidden. Every execution endpoint must pass the mainnet guard (`experiments/smart-contract-audit/safety/mainnet_guard.py`).
2. **Authorization required.** Only audit a target the operator is authorized to test: their own code, a bug-bounty scope (e.g. Immunefi/Code4rena/Sherlock/Cantina), a CTF, or an explicit engagement. Record the authorization basis in the report intake. If authorization is unclear, stop and ask.
3. **Host stays clean.** Install tools only inside the disposable sandbox from a declared, approved manifest. Never mutate the operator's host toolchain.
4. **Evidence discipline.** Distinguish observation, derivation, inference, and unknown. A passing test or empty scanner result never proves absence of a bug. Every finding in the report carries its evidence tier and provenance.

---

## Program-Specific Scope References

Before Stage 1, check whether the target belongs to a bounty program with a scope
reference below. Load it and apply its in-scope assets, impact list, and
severity rules to intake, threat modeling, and Stage 6 severity.

- **Hedera (Immunefi Blockchain/DLT):** [references/programs/hedera-immunefi.md](references/programs/hedera-immunefi.md)

## Stage 0 & Stage 1: Intake, Threat Modeling & The State-Invariant Matrix

Automated scanners (Slither, Aderyn, cargo-audit) rarely find the Medium, High, and Critical vulnerabilities that win Immunefi competitions. Across 2,179 M/H/C reports, **over 85% of valid High/Critical findings are semantic state-machine, accounting, boundary, or cross-component desynchronization bugs**.

Before hunting individual functions, construct three explicit tables during Stage 1 (Recon):

### 1. The Mirrored State & Conservation Matrix
List every aggregate/global state variable alongside the sum of per-user/per-position variables and physical token balances it must equal or bound:

$$\text{Physical Balance } \ge \text{Global Tracked Principal} + \text{Global Tracked Unclaimed Yield/Fees} \ge \sum_{u} \text{User Principal}_u + \sum_{u} \text{User Unclaimed}_u$$

| Global / Aggregate Variable | Per-User / Per-Bucket Counterpart | Physical Token / Escrow Backing | All Mutator Functions (Including Emergency, Force, Liquidate, Merge, Split, Slash, Rescue) |
| :--- | :--- | :--- | :--- |
| e.g. `totalDebt`, `cumulativeEarmarked` | `account.debt`, `account.earmarked` | `alAsset.totalSupply()` | `mint`, `burn`, `repay`, `_forceRepay`, `liquidate`, `redeem` |
| e.g. `totalSupply`, `pendingWithdrawAssets` | `withdrawSharesOf[period][user]` | `asset.balanceOf(vault)` | `deposit`, `requestWithdraw`, `claimWithdraw`, `rescueWithdrawFromBlocklisted`, `startUnstake` |
| e.g. `delegatorsEffectiveStake` | `delegation.stake`, `endPeriod` | `VET` / staked token balance | `delegate`, `requestDelegationExit`, `unstake`, `validatorExit` |
| e.g. `total_active_stake_key` | `total_rewards_key`, `xALGO` supply | Application ALGO balance | `immediate_mint`, `claim_delayed_mint`, `propose_block`, `burn` |

### 2. The Temporal & Epoch Boundary Matrix
List every time-dependent transition (`epochStart`, `periodStart`, `lastEarmarkBlock`, `proposalSnapshot`, `expiryTime`, `cooldownEnd`) and record:
- Whether time is measured in **blocks** (`block.number`), **Unix seconds** (`block.timestamp`), **milliseconds** (`get_tick_count()`), or **TAI64** (`std::block::timestamp()` on Fuel = $2^{62} + \text{unix} + 10$).
- Whether boundary comparisons use strict (`<`, `>`) or inclusive (`<=`, `>=`) operators, and what happens when two transitions occur in the **exact same block/timestamp** ($t_1 == t_2$) or **consecutive blocks** ($b_2 == b_1 + 1$).

### 3. The Trust & Representation Boundary Matrix
Map every boundary where data changes representation or crosses trust zones:
- **Decimals & Units:** 18-decimal debt/internal shares vs. 6/8-decimal underlying tokens (`USDC`, `WBTC`); signed `i128` vs. prime field `felt252` vs. `u256`; 8-byte Algorand `ApplicationID` vs. 32-byte `ApplicationAddress`.
- **External Callbacks & Untrusted Inputs:** Router swap calldata (`1inch`, `OKX`), CCIP `offchainTokenData`, Subgraph/Indexer responses, Rhinestone orchestrator quotes, P2P wire frames (Snappy/zstd/SCALE/Protobuf), and URL route parameters (`$name`).

---

## Master Hunting Playbook: Domain 1 — EVM Smart Contracts (12 High-Yield Bug Classes)

Read [references/immunefi-evm-playbook.md](references/immunefi-evm-playbook.md) for full code excerpts and 43 deep-dive case studies. Check every EVM codebase against these 12 root-cause classes:

### Class 1: Mirrored / Aggregate vs. Per-Account State Desynchronization
- **What to Hunt:**
  1. **Clamped Deduction Paying Unclamped Fee/Reward:** A helper clamps the deduction from a user's balance (`account.collateralBalance -= fee > bal ? bal : fee`) but returns the full unclamped `fee` to `_liquidate()`, which transfers `fee` out of pooled collateral (`#56365`, `#56385`, `#56519`, `#56740` — Critical).
  2. **Hybrid Balance-Delta + Manual Counter Double Deduction:** `repay()` with yield tokens directly subtracts from `cumulativeEarmarked` and `totalDebt` and transfers tokens to `Transmuter` without updating `lastTransmuterTokenBalance`. The next `redeem()` measures `transmuter.balanceOf() - lastTransmuterTokenBalance` and subtracts `coverToApplyDebt` from `cumulativeEarmarked` and `totalDebt` a **second time** (`#56965` — Critical).
  3. **Omitted Aggregate Update in Secondary Flows:** `_forceRepay()` decrements `account.earmarked` without decrementing `cumulativeEarmarked` (`#58337` — High).
  4. **Premature `totalSupply` Reduction During Unbonding vs. Balance-Based Reward Surplus:** `startUnstake()` immediately decrements `totalSupply` while tokens sit in the contract for a 10-day vesting period; `accumulatedDeptRewardsYeet()` computes `stakingToken.balanceOf(address(this)) - totalSupply`, treating unbonding user principal as excess yield and distributing it to other stakers (`#41215`, `#41286` — Critical).
  5. **Unclaimed Prior-Epoch Revenue Re-Counted as New Revenue:** `RevenueHandler.checkpoint()` reads `IERC20(token).balanceOf(address(this))` instead of `balanceOf(address(this)) - totalUnclaimed[token]`, adding unclaimed yield from past epochs into new epoch distributions (`#30651`, `#31242`, `#31263` — Critical).
  6. **Double Stake Subtraction When User Exit Crosses Validator Exit:** User signals exit while validator is `ACTIVE` (scheduling a stake decrease for period $P+1$); validator then transitions to `EXITED` before user calls `unstake()`. `unstake()` sees `validator == EXITED` and subtracts the user's effective stake a second time, underflowing (`0x11` panic) and permanently freezing the user's staked funds (`#59386`, `#59443`, `#59564` — High).

### Class 2: Checkpoint, Binary Search & Period/Block Boundary Off-By-One (`<` vs `<=`)
- **What to Hunt:**
  1. **Binary Search Breaking on First Match With Duplicate Timestamps:** In `VotingEscrow.totalSupplyAtT(t)`, the binary search breaks immediately on `if (lastPoint.ts == t) { lower = center; break; }` instead of continuing to the *latest* point at timestamp `t` (whereas `balanceOfTokenAt` returns the latest point). Calling `checkpoint()` + `createLock()` in the same block at `proposalSnapshot` and later appending new checkpoints shifts `(upper - lower) / 2` so `totalSupplyAtT` returns the pre-lock supply while `getPastVotes` returns the post-lock voting power (`#30655` — Critical).
  2. **Consecutive-Block Range Query Returning Zero (`endBlock <= startBlock`):** `Transmuter.queryGraph(startBlock, endBlock)` starts with `if (endBlock <= startBlock) return 0;`. When `_earmark()` is called in consecutive blocks (`block.number == lastEarmarkBlock + 1`), it queries `queryGraph(lastEarmarkBlock + 1, block.number)` where `startBlock == endBlock`, returning `0` and permanently skipping that block's matured redemptions (`#56732`, `#57066` — Critical).
  3. **Strict `>` Instead of `>=` at Delegation End Period ("Ghost Stake" Infinite Drain):** `Stargate._claimableDelegationPeriods()` checks `if (endPeriod != type(uint32).max && endPeriod < currentValidatorPeriod && endPeriod > nextClaimablePeriod)`. Once a user claims up to `endPeriod`, `nextClaimablePeriod` equals `endPeriod + 1` (or `endPeriod`), failing `endPeriod > nextClaimablePeriod` and falling through into the active-delegator `else` branch—allowing an exited NFT to claim VTHO rewards forever (`#59316`, `#59358`, `#59361` — High).
  4. **Inclusive `upperLookupRecent(periodStart)` (`<=`) vs. New-Period Classification at `block.timestamp == periodStart`:** In `FirelightVault`, a `withdraw()` at exact `block.timestamp == periodStart` is classified as a withdrawal in the *new* period $P$, but pushes the reduced post-withdraw balance to `_traceTotalAssets` at timestamp `periodStart`. Because `totalAssetsAt(periodStart)` uses inclusive `<=`, the opening asset ceiling for period $P$ drops to the post-withdrawal balance, letting stakers evade incident slashing (`#88287`, `#88308`, `#88345` — High).
  5. **Overlapping Period Capacity Snapshot Reuse:** `CoverOrderAllocator._computeAvailableCapacity()` reads `totalAssetsAt(currentPeriodStart())` without subtracting valid incident payouts from Period $P-1$ that execute during Period $P$, selling new cover against already-spent collateral (`#88214`, `#88237`, `#88244`, `#88279` — High).
  6. **Exact Epoch-End Timestamp Overwrite in `Bribe`:** Calling `Voter.distribute()` (`Bribe.resetVoting()`, setting `totalVoting = 0`) and a tiny `Voter.vote()` at exact `block.timestamp == _lastEpochEnd` overwrites the epoch-end checkpoint denominator `_priorSupply`, letting the tiny voter claim 100% of the epoch's bribes in the next block (`#30860` — Critical).

### Class 3: Flash-Loan / Same-Block Snapshot Omission & Live Balance Manipulation
- **What to Hunt:**
  1. **Asymmetric Live Balance in `vote()` / `clearVote()`:** `PerpetualGauge.vote()` weights votes by live `votingToken.balanceOf(msg.sender)` (`veALCX`, which has an ERC-3156 flashloan) and `clearVote()` subtracts `existing.weights[i] * votingToken.balanceOf(msg.sender)`. An attacker flash-borrows `veALCX`, calls `vote()`, repays the flashloan (balance = `0`), and calls `clearVote()`—which subtracts `0` from `aggStrategyWeight`, permanently locking in the flash-loaned vote weight (`#56798` — Critical).
  2. **Spot TVL in Bad-Debt Scaling:** `Transmuter.claimRedemption()` scales down payouts when `badDebtRatio = totalSyntheticsIssued * 10**decimals / getTotalUnderlyingValue() > 1e18`. Sandwiching `claimRedemption()` with a flash-loaned `deposit()` and `withdraw()` in the same transaction inflates `getTotalUnderlyingValue()` so `badDebtRatio <= 1e18`, bypassing the bad-debt haircut and draining the pool (`#56555` — Critical).
  3. **Batched Yield Distribution Without Snapshot or Transfer Lock:** `ArcToken.distributeYieldWithLimit(batchSize)` processes `holders[i]` across multiple transactions using live `balanceOf(holder)`. An attacker claims yield in Batch 1, transfers tokens to a Batch 3 address, and claims yield again (`#49710` — High).
  4. **Per-TokenId Epoch Guard Bypassed via `merge()` / `poke()`:** `Voter.poke(_tokenId)` omits `onlyNewEpoch(_tokenId)`, allowing infinite FLUX accrual in a loop (`#30634`, `#30650` — Critical); or `Voter.reset(_tokenId)` checks `onlyNewEpoch(_tokenId)` per token ID, allowing an attacker to call `reset(id1)`, `veALCX.merge(id1, id2)`, and `reset(id2)` in the same epoch (`#30683`, `#30800`, `#30825` — Critical).

### Class 4: Rounding, Precision Loss & Virtual-Offset Inversion
- **What to Hunt:**
  1. **ERC-4626 Virtual Offset Inversion After Slashing (`totalAssets() == 0 && totalSupply() > 0`):** OpenZeppelin's `assets.mulDiv(totalSupply + 10**offset, totalAssets + 1, Floor)` assumes `totalAssets == 0` only when `totalSupply == 0`. When `FirelightVault.payout()` slashes `totalAssets()` to `0` while `totalSupply() == 20,000e6`, depositing `1 wei` mints `1 * (20,000e6 + 1) / 1 = 20,000,000,001` shares (>50% of the vault for `1 wei`!) (`#88330` — Medium/High).
  2. **First-Depositor Inflation / Donation Attack (`totalAssets() > 0 && totalSupply() == 0`):** Vaults using `asset.balanceOf(address(this))` with `_decimalsOffset() == 0` and no `require(shares > 0)` allow front-running a victim's deposit with 1 wei mint + direct token donation so the victim mints `0` shares (`#56850`, `#56863`, `#56907` — Critical).
  3. **Sub-Unit Debt Burn Rounding `toFree` to `0`:** `normalizeDebtTokensToUnderlying(amount)` computes `amount / 1e12` (18-decimal debt to 6-decimal USDC). Burning `1e12 - 1` wei of debt reduces `account.debt` by `1e12 - 1` while `toFree == 0` (leaving `account.rawLocked` frozen, and reducing `account.debt < account.earmarked` so `redeem()` seizes collateral for already-burned debt) (`#57053` — Critical).
  4. **Health/Collateral Factor Returning `0` on Dust Truncation:** `Pricing.collateralFactorInBasisPoints()` computes `collateralInCredited = (collateral * price) / 10**exp` and returns `0` when `collateralInCredited == 0` to avoid division-by-zero. Because `modifyLOCCollateral()` checks `if (cfBasisPoints > requiredCF) revert`, leaving `1 wei` of collateral truncates `collateralInCredited` to `0`, returns `cfBasisPoints = 0`, and lets the creator withdraw 99.9999% of an active Letter of Credit's collateral (`#36910`, `#36931` — Critical).
  5. **Timestamp Advancement on Zero-Floored Commission Delta:** `updateRewardPerTokenForValidator` floors `commissionDeltaForValidator` to `0` on small intervals and still sets `validatorLastUpdateTimes = block.timestamp`, letting anyone grief validator commission to `0` via cheap dust pokes (`#49700` — High).

### Class 5: Retroactive Global Parameter Mutation Without Prior Accrual / Sync
- **What to Hunt:**
  1. **`setMinimumCollateralization()` Without Syncing `_collateralWeight`:** Updating `minimumCollateralization` causes subsequent redemptions to scale `_collateralWeight` with the new multiplier while existing accounts still hold old `rawLocked`, over-decaying their collateral during `_sync()` (`#56491` — Critical).
  2. **`addValidator()` Missing Initial Commission Checkpoint:** `addValidator()` sets `validator.commission` without pushing a checkpoint at $t_0$. When `setValidatorCommission()` later updates commission, historical intervals $[t_0, t_1]$ fall back to the new mutable `validator.commission`, retroactively altering user rewards (`#53037` — Critical).
  3. **Removed & Re-Added Reward Token Skipping Timestamp Initialization:** `_initializeRewardStateForNewStake()` only sets `userValidatorRewardPerTokenPaidTimestamp = block.timestamp` for *currently active* reward tokens. When a previously removed reward token with historical accrued rewards is removed a second time, a fresh staker inherits timestamp `0` (or old timestamp $X$) and steals historical rewards (`#49616`, `#49731` — High).
  4. **Pool Reset (`resetPool()`) Leaving Pre-Reset User Pending Unstake Units Active:** `TimeBasedCollateralPool.resetPool()` resets the pool's unit exchange rate after a 99% slash, but pre-reset pending unstake units for epoch $X+1$ are not scaled down and later claim against post-reset 1:1 `ExitBalances` (`#36554` — Critical).

### Class 6: Lifecycle State-Machine Bypass, Zero-Deposit Array Stuffing & Poisoned Flags
- **What to Hunt:**
  1. **Operations on Unsettled / Expired Loans Pulling Pre-Approved Allowances:** In `Term Structure Settlement.sol`, lenders pre-approve `10x` collateral at `createSettlement()`. Because `addCollateralBeforeSettle()` increments `collateralAmt` without transferring tokens and `repay()` / `liquidate()` omit `require(loan.settled)`, a borrower inflates `collateralAmt` on an unsettled loan and calls `repay()` / `liquidate()` to drain the lender's wallet (`#46819`, `#46903`, `#47112`, `#47115` — Critical).
  2. **Zero-Amount Deposit Pushing Duplicate `poolId` into `colPools` Array:** `UserLoanLogic.increaseCollateral()` checks `if (loan.collaterals[poolId].balance == 0) loan.colPools.push(poolId);` before adding `fAmount`. Calling `deposit(0)` $N$ times pushes `poolId` $N$ times into `colPools`, multiplying collateral valuation by $(N+1)\times$ in `getLoanLiquidity()` (`#33269`, `#33665`, `#33780`, `#33978` — Critical).
  3. **Single Boolean `isWithdrawClaimed[period][account]` Poisoning Reusable Pending Bucket:** `rescueWithdrawFromBlocklisted()` sets `isWithdrawClaimed[period][from] = true` for a pending period $P+1$. If `from` is un-blocklisted and requests another withdrawal in the same period, the new shares are added to `withdrawSharesOf[period][from]` and `pendingWithdrawAssets`, where they are permanently unclaimable and unrescuable (`#88231`, `#88254`, `#88270`, `#88286` — High).
  4. **Pre-Transfer Before Non-Reverting `killSwitch` Early Return:** `VaultV2.allocateInternal` transfers tokens to `MYTStrategy` before calling `allocate()`. When `killSwitch == true`, `allocate()` returns `(ids, 0)` without reverting, stranding the transferred tokens (`#57152` — High).

### Class 7: Unvalidated Router/Wrapper/Adapter Calldata, Approvals & Partial-Fill Stranding
- **What to Hunt:**
  1. **User-Controlled `teller` / `router` Receiving `safeApprove` in Wrapper:** `DexAggregatorWrapperWithPredicateProxy._okxHelper()` accepts an unvalidated `TellerWithMultiAssetSupport teller` and arbitrary `okxRouter` calldata, calling `supportedAsset.safeApprove(address(teller.vault()), amount)`—allowing an attacker to pass a fake `teller` and drain all tokens from the wrapper (`#49863` — Critical).
  2. **Unhandled 1inch / OKX Partial Fills Stranding User Input Tokens & Allowances:** Wrapper pulls `amountIn` from user, approves router, executes a partial fill (`spent < amountIn`), and neither refunds `amountIn - spent` to the user nor resets the leftover router allowance (`#49854`, `#51283`, `#51847`, `#52923` — Critical).
  3. **Refunds Sent to `_msgSender()` (Intermediary Contract) Instead of `receiver`:** `StakeV2` calls `Zapper.zapOut()`, which refunds unswapped dust to `_msgSender()` (`StakeV2`) instead of the user, permanently locking the funds (`#41280` — High).
  4. **Unvalidated External Reward Contract Inflating `totalCollateral`:** `CollateralPool.claimAirdropDistribution(IDistributionToDelegators _distribution)` lets an agent pass an arbitrary contract address and adds its return value directly to `totalCollateral` (`#45893`, `#46688`, `#46858`, `#46985` — High).

### Class 8: Partial Payout / Slashing Without Burning Shares or Adjusting Debt Return
- **What to Hunt:**
  1. **Payout Capped at `min(entitled, balance)` While Burning 100% of Shares/Synthetics:** `Transmuter.claimRedemption()` caps `distributable` MYT to `balAfterRedeem` when `AlchemistV3` has a shortfall, but still burns the user's full `position.amount` of `alAsset` without returning the unredeemed portion (`#56702` — Critical). Similarly, Flare FAssets `CollateralPool._selfCloseExitTo()` burns 100% of `_tokenShare` while `payoutFromVault` caps the payout at `Math.min(_amountWei, vaultBalance)` (`#47108`, `#46378` — High).
  2. **Slashed Queued Withdrawal Reverting Completion Forever:** In `PufferVault.claimWithdrawalFromEigenLayer()`, if EigenLayer slashes a queued withdrawal (`withdrawalRootPending = false`), `completeQueuedWithdrawal` reverts permanently, preventing `$.eigenLayerPendingWithdrawalSharesAmount` from ever being decremented and leaving `totalAssets()` permanently inflated (`#28788` — Critical).

### Class 9: Non-Fungible Lock / Vesting Ledger Desync & OpenZeppelin v5 `_requireOwned` Misuse
- **What to Hunt:**
  1. **OpenZeppelin v5 `_requireOwned(tokenId)` Does NOT Check `msg.sender`:** In OZ ERC-721 v5, `_requireOwned(tokenId)` only checks that `tokenId` has been minted (`_ownerOf(tokenId) != address(0)`). `VestedZeroNFT.split(tokenId, fraction)` called `_requireOwned(tokenId)` without checking `ownerOf(tokenId) == msg.sender`, allowing anyone to split and steal 99.99% of every user's vesting NFT (`#29031`, `#29204`, `#29288` — Critical).
  2. **Fungible Voting Power Used to Unstake Arbitrary NFT `tokenId`:** `OmnichainStaking.unstakeToken(uint256 tokenId)` burned `power(tokenId)` from `msg.sender` without checking that `msg.sender` deposited `tokenId`. Because `power = amount * duration`, an attacker locking `20 ZERO` for 4 years (`power = 80`) could unstake and steal Alice's NFT containing `40 ZERO` locked for 2 years (`power = 80`) (`#29062`, `#29135` — Critical).
  3. **ERC-4626 Share Transfer Desynchronizing Per-User Time-Lock Array:** `Belong Staking.sol` records deposit timestamps in `stakes[to]` on `_deposit` and checks `stakes[owner]` on `_withdraw`, but does not override `_update` / `_beforeTokenTransfer`. Transferring `sLONG` shares leaves `stakes[recipient]` empty, permanently blocking normal withdrawals for recipients (`#56872`, `#57298`, `#57399`, `#57635` — Critical).

### Class 10: Signature, Calldata-Binding TOCTOU & Unbound Bridge `offchainTokenData`
- **What to Hunt:**
  1. **Multisig/Consortium Approval Binding Container ID (`incidentId`) Instead of Mutable `roundId`:** `IncidentManager.approveCurrentAssessment(uint256 incidentId)` resolves `incident.currentAssessmentRoundId` at execution time. A curator cancels Round 1 (100 loss), submits Round 2 (900 loss) under the same `incidentId`, and executes the consortium's pre-signed transaction (`#88618` — High).
  2. **Unbound CCIP `offchainTokenData` in Manual Execution:** In Chainlink CCIP `releaseOrMint`, `offchainTokenData` is passed by the manual executor and is not authenticated by CCIP's Merkle root unless matched against `releaseOrMintIn.sourcePoolData`. An attacker swaps `offchainTokenData` from a `1 satoshi` transfer onto a victim's `10 LBTC` CCIP message, minting `1 satoshi` and consuming the `10 LBTC` message (`#38154`, `#38342`, `#38634` — Medium).
  3. **Front-Runnable Permit / Approval Signatures Causing DoS:** Passing a user's ERC-2612 `permit` or `modifyCollateralizableTokenAllowanceWithSignature` inside `stake()` without a `try/catch` or fallback allowance check allows anyone to front-run the signature from the mempool, increments the nonce, and causes the user's `stake()` transaction to revert (`#36268`, `#36475`, `#36501` — Medium).

### Class 11: Role Hierarchy Bypass & Extreme Parameter Denominators
- **What to Hunt:**
  1. **`DEFAULT_ADMIN_ROLE` Granted to Deployer on UUPS Proxy:** `ArcTokenFactory.createToken()` grants `DEFAULT_ADMIN_ROLE` (`0x00`) to `msg.sender` and `UPGRADER_ROLE` to `address(this)` (to enforce an implementation whitelist) without calling `_setRoleAdmin(UPGRADER_ROLE, ...)`. Because `DEFAULT_ADMIN_ROLE` is the admin of `UPGRADER_ROLE`, `msg.sender` calls `grantRole(UPGRADER_ROLE, msg.sender)` and upgrades to arbitrary code (`#50713` — High).
  2. **Denominator Parameter Allowed Down to `1 BIPS`:** `CollateralPool.setTopupTokenPriceFactorBIPS` only checks `_factorBIPS > 0`, allowing an agent to set `1 BIPS` (`0.01%`) and mint $10,000\times$ pool shares (`#45979`, `#47020` — High).

### Class 12: L2 / Rollup Proof-Executor vs. Canonical EVM Divergence
- **What to Hunt:**
  1. **EVM Opcode Environment Mismatch (`BLOBBASEFEE`):** Canonical Base L2 execution hardcodes `blob_gasprice = 1` and repurposes `header.blob_gas_used` for DA footprint, whereas the ZK proof executor computes EIP-4844 `BlobExcessGasAndPrice` from `parent_header.blob_gas_used`, yielding `block.blobbasefee > 1` inside the ZK guest (`#75413` — Medium/High).

---

## Master Hunting Playbook: Domain 2 — Non-EVM Smart Contracts (Move, Sway, Solana, Cairo, Clarity, Algorand, Chialisp)

Read [references/immunefi-non-evm-playbook.md](references/immunefi-non-evm-playbook.md) for the complete 139-report Non-EVM synthesis. Whenever auditing a non-EVM chain, audit against the **7 Cross-VM Mental Model Failures**:

### 1. Starknet / Cairo (`felt252` Prime Field $\mathbb{F}_P$ vs. `i128` / `u256`)
- **Negative `i128` $\rightarrow$ `felt252` $\rightarrow$ `u256` Wrap to $P \approx 3.618 \times 10^{75}$:** In Cairo, `(-x_i128).into()` produces `felt252` value $P - x$. If a function returns `felt252` (e.g., `getAccountValue` or `get_account_free_balance` when PnL is negative) and the caller converts that `felt252` into `u256` via `.into()`, a `-42` debt wraps to $+3.618 \times 10^{75}$, corrupting vault `_total_assets()`, `_is_liquidated()`, and fast-withdrawal checks (`#46997`, `#47309`, `#47310` — Medium).
- **Missing Sign Check on `felt252` $\rightarrow$ `i128` Transfer Amount:** `Felt252TryIntoI128::try_into()` converts field elements in $[P - 2^{127}, P - 1]$ into **negative `i128` values**. In `paraclear.cairo::account_transfer_partial`, passing `amount_collateral = (-10^12_i128).into()` unwraps to `amount_i128 = -10^12`, passes `sender_balance >= amount_i128`, and executes `sender_balance - (-10^12)`, minting $10^{12}$ collateral out of thin air (`#46888` — High).
- **Pre-Transfer Decimal Scaling Truncation (`amount / 100 == 0`):** Dividing internal 8-decimal `amount` by `scale_factor = 100` (for 6-decimal USDC) *before* `erc20.transferFrom` while crediting unscaled `amount` to internal storage allows depositing `amount = 99` for `0` USDC cost (`#46892` — High).
- **Fail-Open Zero Registry Address & Pre-Collateral Margin Check:** Checking transfer restrictions only `if transfer_registry_address.is_non_zero()` without checking `get_caller_address() == sender` allows anyone to drain accounts when the registry is unconfigured (`#46843`, `#47370` — Critical); checking `excess_balance >= 0` *before* moving `amount_collateral` in `account_transfer_partial` lets traders strip collateral from losing positions (`#47314` — Medium).

### 2. Fuel / Sway (`FuelVM` UTXO Multi-Asset Call Frames, 64-Bit Registers & TAI64)
- **Manual Storage Reentrancy Lock + Early `return` = Permanent Protocol Deadlock:** Sway has no Solidity-style post-body `nonReentrant` modifier cleanup. In `fluid-protocol`, `internal_redeem_collateral_from_trove` sets `storage.lock.write(true)` and later executes `if (new_debt < MIN_NET_DEBT) { return values; }` without resetting `storage.lock.write(false)`, permanently locking all redemptions across the protocol (`#37323`, `#37452`, `#37624`, `#37671` — Critical).
- **Negative `I256` Borrow Principal Passed Through `.wrapping_neg()` Before Sign Check:** In `swaylend_iop`, `is_liquidatable_internal` calls `present_value(principal.wrapping_neg())` when `principal < I256::zero()`. Because `wrapping_neg()` flips the borrow principal positive, `present_value` takes the `principal >= 0` branch and accrues borrower debt at the lower `base_supply_index` instead of `base_borrow_index` (`#35758` — Critical).
- **Native Call-Frame `msg_amount()` Immediate Credit & Zero-Amount `transfer()` Panic:**
  - `msg_amount()` is credited to the contract before execution starts; checking `msg_amount() >= amount` without refunding `msg_amount() - amount` permanently locks excess native tokens (`#35793`, `#35876` — High).
  - FuelVM's `TR`/`TRO` opcode (`transfer(to, asset, amount)`) **panics and reverts the transaction when `amount == 0`**, breaking all marketplace trades whenever `protocol_fee == 0` or `royalty_fee == 0` (`#34966` — High).
- **64-Bit Register Overflow in `u8`/`u16`/`u32` `pow()` & Emulated Signed Math (`I8..I256`, `IFP*`):** FuelVM registers are 64-bit, so `exp` in `u8::pow` overflows past 255 into the upper 56 bits without triggering `$of` unless bounded (`#33227`, `#33331` — High); `sway-libs` signed integer `subtract` and `IFP::ceil`/`multiply` invert signs on negative operands (`#32696`, `#32700`, `#32706`, `#33039` — High).
- **TAI64 vs. Unix Timestamps:** `std::block::timestamp()` returns TAI64 ($2^{62} + \text{unix} + 10 \approx 4.61 \times 10^{18}$); comparing it directly to Redstone/Pyth Unix timestamps breaks oracle freshness checks (`#37276` — Medium).
- **ABI Supertraits Compiled as Public Entrypoints:** The Sway compiler exposes all `abi MyContract : SuperTrait` methods in the contract's public selector dispatch table even though they are omitted from `contract-abi.json` (`#33351` — Critical).

### 3. Move (Aptos / Movement Labs)
- **Feature-Flagged Prologue/Epilogue Asymmetric Branches:** In `transaction_validation.move::epilogue_gas_payer`, when `amount_to_burn < storage_fee_refunded`, storage refunds are only minted `if (!features::governed_gas_pool_enabled())` with no `else` branch—losing 100% of storage refunds when `governed_gas_pool` is active (`#43135`, `#42513` — High).
- **`CoinStore<T>` vs. `FungibleStore` Migration Gaps:** Routing gas fees through `fungible_asset::deposit_internal(governed_gas_pool_store_address, fa)` aborts every transaction with `EFUNGIBLE_STORE_EXISTENCE` if `initialize` only created a legacy `CoinStore` (`#42480` — Medium); using `coin::withdraw_from` in the epilogue breaks the invariant that frozen `CoinStore` accounts can still pay gas (`#42928` — Medium).
- **Mid-Block `process_collected_fees()` Wiping Block Proposer:** Calling `upgrade_burn_percentage()` before any fees are collected hits the zero-fee early exit in `process_collected_fees()`, which extracts and clears `collected_fees.proposer`, burning all validator fees for the rest of the block (`#43132` — Medium).

### 4. Solana / SVM (Rust / Anchor / Native)
- **Unsynced Direct SPL Transfers Before `UpdateVaultBalance` Crank:** Anyone can `spl_token::transfer` SPL tokens directly to a vault's ATA (`vault_st_token_account.amount`). Because `mint_to` (deposit) does not call `update_vault_balance` first and rewards do not vest over time, an attacker deposits flash-loaned stake right after rewards land in the ATA, cranks `update_vault_balance`, and steals 90%+ of the epoch's yield (`#36903`, `#37295`, `#37311`, `#37315` — High).
- **Recursive Fee Charging on Fee-Wallet Redemptions:** In `jito-restaking`, `burn_withdrawal_ticket` charges `vault_fee_bps` and `program_fee_bps` (minting new VRT fee shares) even when the burner *is* `vault_fee_wallet` or `program_fee_wallet`, trapping fee withdrawals in an infinite geometric series (`#37314` — High).

### 5. Algorand / PyTeal (AVM)
- **8-Byte `uint64` `ApplicationID` vs. 32-Byte `ApplicationAddress` (`sha512_256("appID" || itob(app_id))`) Mismatch:** In Folks Finance Wormhole NTT, `TransceiverManager.attestation_received` converts `message.handler_address` (32 bytes) to an app ID via `safe_convert_bytes32_to_uint64` (which requires the top 24 bytes to be `0x00`), while `MessageHandler` compares `message.handler_address` against `Global.current_application_address()` (a full 32-byte SHA512_256 digest)—permanently bricking 100% of incoming cross-chain transfers (`#56615`, `#57018`, `#57333` — High).
- **AVM `uint64` Underflow Panic When Subtracting `Principal + Yield` From `Principal`-Only State:** In `consensus_v2.py::burn()`, `total_active_stake_key` tracks only deposited ALGO principal while rewards accrue in `total_rewards_key`, yet `burn()` subtracts the full `algo_to_send` (`principal + rewards`) from `total_active_stake_key`—driving `total_active_stake_key` to `0` early and permanently freezing ~111,000 ALGO on Mainnet via AVM `- would result negative` panics (`#37660`, `#37661`, `#37863`, `#37889` — High).

### 6. Stacks / Clarity
- **Unvalidated Principal Version Bytes (`0..=31` vs. `{20, 21, 22, 26}`):** `StandardPrincipalData::new` accepts any `version < 32`. Deposit scripts with `version = 0` pass off-chain Rust parsing so BTC is swept, but Clarity `complete-deposit` cannot mint sBTC to a version-0 principal (`#38740` — High).
- **Clarity `fold` / Nested-Call Event Amplification vs. Off-Chain `axum` / DB Limits:** Emitting >2 MB of `(print ...)` events via 4-level nested Clarity calls exceeds the sBTC Signer's `axum` 2 MB `DefaultBodyLimit` on `/new_block`, trapping `stacks-node` in an infinite 1s retry loop (`#38111` — High); batching 550 `initiate-withdrawal-request` calls in one `fold` triggers a PostgreSQL foreign-key race that halts all Signers (`#40692` — High).

### 7. Chia / Chialisp (CLVM)
- **CAT TAIL Program Validating Untrusted Solution Amount Instead of `cat_truths` (`Truths`):** In `circuitdaoiop`, `vault_borrow.clsp` hashes caller-supplied `byc_issuing_coin_info` (`amount = 10,000,000,000`) to get `byc_issuing_coin_id` without checking `amount == borrow_amount (1)`, and `byc_tail.clsp` verifies the `0x3f` message against solution param `new_byc_coin_amount (1)` without asserting `(= new_byc_coin_amount (my_amount_cat_truth Truths))`—allowing an attacker to mint 10 billion BYC stablecoins while recording 1 BYC of debt (`#43705` — Critical).
- **Singleton Uniqueness Keyed on Mutable `INNER_PUZZLE_HASH` Instead of Immutable `LAUNCHER_ID`:** Rotating `INNER_PUZZLE_HASH` via `announcer_mutate` and re-registering in `announcer_registry` allows infinite Sybil registration to steal 99% of CRT rewards (`#44355` — High).
- **Unconstrained Slashed Mojos in SpendBundle:** Recreating a penalized singleton with `penalized_deposit < DEPOSIT` without a `CREATE_COIN` locking the difference lets the announcer self-penalize and pocket the slashed XCH (`#44324` — Medium).

---

## Master Hunting Playbook: Domain 3 — Blockchain / DLT Core Nodes, Consensus, VMs & Compilers

Read [references/immunefi-blockchain-dlt-playbook.md](references/immunefi-blockchain-dlt-playbook.md) for the complete 403-report Blockchain/DLT synthesis.

### 1. Pre-Validation P2P/Gossip Decompression, Wire-Cast Panics & Non-Yielding Async Loops
- **Unbounded Pre-Validation Snappy/zstd Decompression on Gossip `message_id_fn`:** In Base (`crates/consensus/gossip`), `compute_message_id` and `NetworkPayloadEnvelope::decode` call `snap::raw::Decoder::decompress_vec` *before* deduplication, validation, or peer scoring without checking `decompress_len(data) <= MAX_GOSSIP_SIZE` (`#74391`, `#74480`, `#74556` — Critical). In Movement, Celestia DA light nodes decompress `zstd` bombs without size caps (`#42143`, `#42233` — Critical).
- **Language-Specific Wire-Cast & Deserialization Panics:**
  - **Nim (`nimbus-eth2`):** Casting wire-parsed protobuf `bsize: uint64 >= 0x80000000_00000000'u64` to signed `int(bsize)` throws a fatal unhandled `RangeDefect` (`#38146`, `#38733` — Medium).
  - **Java (`teku`):** `maxBlobsPerBlock * message.getCount().longValue()` overflows signed `long` to negative, bypassing `> maxRequestBlobSidecars` and subtracting from `RateTrackerImpl` (`#38920` — Medium).
  - **TypeScript (`shardeum`):** Custom JSON reviver `safeJsonParse` / `safeStringify` passes `{"dataType": "bb", "value": {"length": 1e9}}` to `Buffer.from(value, 'base64')`, allocating 1–10 GB on the event loop (`#33424`, `#39434` — Critical); Rust Neon FFI passes `msg_len = (2^29) - 23` to V8 string creation, panicking on `unwrap()` (`#33428` — Critical).
  - **Rust (`stacks`, `movement`):** Unchecked slice `data[..AES_GCM_NONCE_SIZE]` in `wsts::decrypt` panics on empty share (`#37814` — High); `exp_date_secs = u64::MAX` overflows `seconds_from_epoch` in `KeylessSignature` (`#42934` — Critical); 200 nested Move structs with `max-gas = 3` hit `panic!("Unexpected offsets...")` (`#42837` — Critical).
- **Non-Yielding `tokio::select!` Error Loop & Litep2p Accept-Loop Termination:**
  - In Base batcher (`SubmissionQueue::submit_pending`), when `BlobEncoder::encode_packed` returns `Err`, the error arm executes `pipeline.requeue(id); drop(permit); continue;` with no `.await`, spinning synchronously forever on the same oversized frame and starving all `tokio::select!` branches (`#76494` — High).
  - In Substrate `litep2p`, pending unestablished TCP connections are not bounded by `connection_limits`; exhausting file descriptors (`EMFILE`) causes the listener stream to return `Poll::Ready(None)` and permanently die (`#88250` — High).
- **Opaque `BoundedVec<u8>` Bypassing Extrinsic Recursion Depth:** `pallet_multisig::propose` stores inner call bytes as `BoundedVec<u8, 10240>` and decodes via `RuntimeCall::decode` instead of `decode_all_with_depth_limit(256)`. A 3,412-deep nested `Utility::batch` traps the Wasm runtime during block building, rolls back fee/nonce, and bypasses `MAX_SKIPPED_TRANSACTIONS` (`#88969` — High).

### 2. Consensus Splits, Prover/Node Divergence & C Tile Memory Corruption
- **Cache-Hit vs. Cache-Miss Divergence:**
  - **Move VM `TypeLayout` Cache (`#43333` — Critical):** `struct_name_to_type_layout` checks `DEPTH_MAX = 128` on cold construction but skips the depth check on a cache hit—causing nodes with warm caches to accept `V128` while cold nodes abort.
  - **Base Flashblocks `CachedExecutionProvider` (`#74944` — High):** Checks `pending_blocks.has_transaction_hash(prev_tx_hash)` (`HashMap::contains_key`) instead of verifying exact sequential prefix order; omitting an intermediate nonce tx causes cached nodes to accept the block while uncached nodes reject it.
- **ZK-Prover / Stateless Executor vs. Canonical Node Divergence (Base Azul):**
  - `OpZkvmPrecompiles` calls `.extend(get_precompiles())`, overwriting Azul's 81,984-byte-capped `bn254Pairing` and 6,900-gas `P256VERIFY_OSAKA` with vanilla Istanbul/3,450-gas implementations (`#75101` — Critical).
  - `TrieDB::update_accounts` treats `revm` `AccountStatus::DestroyedChanged` (same-block `CREATE2` $\rightarrow$ EIP-6780 `SELFDESTRUCT` $\rightarrow$ `CREATE2`) as pure deletion via `was_destroyed()` (`#75576` — High).
  - SP1 range guest silently truncates execution on `PipelineError::EndOfSource` at block $M < N$, passes block number (~46M) instead of Unix timestamp (~1.74B) to `cfg.is_isthmus_active(...)`, and commits `l2BlockNumber = N` with `l2PostRoot = root@M` (`#76386` — High).
- **32-Bit vs. 64-Bit `usize` Receipt Divergence:** FuelVM `LDC` opcode converts `$rC: u64` to `usize` (`MemoryOverflow`) *before* checking `> contract_max_size` (`ContractMaxSize`) (`#32825` — High).
- **Compiler IR & C Validator Memory Bugs:**
  - Sway compiler puts destination pointer register `$r1` of wide arithmetic `WQAM` into `def_registers()` instead of `use_registers()`, so DCE deletes the pointer initialization (`#32269` — High); `load_store_to_memcopy` ignores escaped mutable references across function calls (`#32872` — High).
  - Firedancer C tiles: `fd_shred_merkle_cnt(variant)` extracts a 4-bit depth (`0..15`) *before* Ed25519 signature verification and indexes `inclusion_proofs[1023]` up to `65,536` (remote heap OOB write, `#34272` — Medium); `fd_shred` copies up to `FD_NET_MTU` (2048) into a `FD_SHRED_MAX_SZ` (1228) slot (`#34564`); `fd_bank` allocates `txn_sidecar_mem` for 1 tx instead of 31 microblock txns (`#34290`).

### 3. Threshold Cryptography, Post-Quantum Signatures, Mempool Extensions & Token Conservation
- **WSTS Threshold Signature Nonce Reuse ($\rightarrow$ Private Key Recovery):** `Party::sign_with_tweak(&self, ...)` in `wsts` signs without consuming/zeroing `self.nonce`. A coordinator sending 3 `SignatureShareRequest`s with different messages for 1 nonce commitment solves 3 linear equations to recover all signers' private key shares and steal the sBTC Bitcoin peg (`#37861` — Critical).
- **Empty Iterator Vacuous Validation in Coordinator:** `signer/src/bitcoin/validation.rs` validates BTC fees inside `self.reports.deposits.iter().map(...)`. Passing `deposits = []` skips all fee checks and lets a coordinator sweep the entire `peg-UTXO` to a BTC miner as transaction fees (`#38458` — Critical).
- **ML-DSA (Dilithium) Missing FIPS 204 `ctx` & Degenerate `t1` Keys:** Quantus verifies runtime extrinsics and off-chain messages with `ctx = None` (`#89819` — Critical), and `verify_var` only rejects all-zero `t1`, allowing single-coefficient `t1` in `1..=31` or `992..=1023` to forge signatures without a secret key (`#88227` — Medium).
- **High-Security Call Whitelist Bypassed via `ChargeTransactionPayment` Tip:** `ReversibleTransactionExtension` restricts compromised high-security accounts to whitelisted calls (`cancel`), but does not cap the transaction `tip`, allowing an attacker to drain 99.999% of the account balance immediately to a colluding miner via a tip on a no-op `cancel` call (`#88609`, `#88841`, `#89347` — Critical).
- **Fuel L2 $\rightarrow$ L1 `MessageOut` Receipts Included on Reverted Transactions:** `update_execution_data` appends `receipts.iter().filter_map(|r| r.message_id())` to the block header's `message_out_merkle_root` *before* checking `if reverted`, allowing an attacker to call L2 bridge withdraw, revert the tx (keeping their L2 tokens), and still claim the L1 bridge withdrawal (`#32965` — Critical).
- **Substrate `fungible::Mutate::transfer` Destroying Tokens Near Existential Deposit:** When `amount < ED` and remaining balance `< ED`, `can_deposit` checks `total_withdraw = amount + dust >= ED` (`Ok`), `withdraw` burns `dust`, and `increase_balance(dest, amount, BestEffort)` silently drops `amount < ED` for a non-existent recipient—debiting the sender while crediting `0` to the recipient (`#88628` — Critical).

---

## Master Hunting Playbook: Domain 4 — Web3 Applications, Wallet Flows & Off-Chain Infrastructure

Read [references/immunefi-web3-apps-and-triage.md](references/immunefi-web3-apps-and-triage.md) for the complete 461-report Web3 App synthesis.

### 1. Displayed-vs-Signed Divergence & Normalization / Decoder Splits
- **Split-Node ENS Profile & Renewal Resolution:**
  - `apps/manager` resolves owner/expiry from `ens_normalize(urlParam)` (`vitalik.eth`) while resolving receiving addresses/records (`#89270`, `#89360` — Critical) or renewal calldata (`#89456` — Critical) from raw `urlParam` (`Vitalik.eth` or `vitalik\u200B.eth` / `vita\u00ADlik.eth` with zero-width/default-ignorable format chars that render pixel-identically under CSS `white-space: nowrap`).
- **Raw Ownership Guard vs. Normalized Token ID Transfer (Direct NFT Theft):**
  - In `apps/portal` (`/ALICE.eth/ownership/transfer`), `resolveEnsOwner` checks raw `ALICE.eth` (gifted by attacker), while `getEnsTokenId(getLabel(name))` normalizes `"ALICE"` $\rightarrow$ `"alice"`, encoding `safeTransferFrom` for the victim's valuable canonical `alice.eth` NFT (`#89278`, `#89474` — Critical).
- **Encoded-Labelhash (`[<64-hex>]`) Subname Reparsing Privilege Escalation:**
  - When a parent owner clicks **Delete** on a literal spam subname `[75aaf377...cb81f].parent.eth`, `deleteSubnameWriteParameters` passes `"[75aaf377...cb81f]"` to viem `labelhash()`, which strips the brackets instead of hashing the string and calls `UserRegistry.unregister(labelhash("valuable"))`—burning the real `valuable.parent.eth` so a low-privilege `ROLE_REGISTRAR` delegate can hijack it (`#90725` — Critical).
- **Unsafe `resolveAddressOrName` Fallback to Stale V1 Controller (`ENSRegistry.owner`):**
  - When an ENS name has no `addr` record, `resolveAddressOrName` falls back to `getEnsOwner`, which returns V1 `ENSRegistry.owner(node)` (the former seller of an unwrapped V1 `.eth` name) while dropping `BaseRegistrar.ownerOf` (`registrant`), irreversibly sending NFT transfers and role grants to the former seller (`#89395`, `#89398` — Critical).
- **Runtime-vs-SDK SCALE Decoder Width Divergence (`MultiAddress::Index`):**
  - Quantus runtime defines `MultiAddress<AccountId, ()>` (`Index(())` = 1 byte), while the generated Dart SDK decodes `Index(Compact<BigInt>)` ($\ge 2$ bytes). An 88-byte `Utility.if_else` payload decodes with zero trailing bytes in the Dart SDK as a 130-planck transfer while executing `Balances.transfer_all` on-chain (`#89808`, `#89987` — High).

### 2. EAC / PermissionedResolver Role-Scope Collapse, V1-to-V2 Migration & HCA Session Keys
- **Account-Only Role Grouping (`groupRolesByAccount`):** Merging a delegate's `ROOT_RESOURCE` (`0x00`) and name-scoped roles into one table row and picking `resolvedNames.find(n => n !== '(root)') ?? ''` causes **Remove user** to emit a single name-scoped revocation, leaving the removed delegate holding global `ROLE_SET_ADDR` across all names on the resolver (`#89253`, `#89293` — Critical).
- **Unresolved Name Resource Silently Escalating to `grantRootRoles`:** When a name-scoped resource hash fails lookup in `buildResourceToNameMap` (because the subname was deleted/repointed or uses record-level roles), `roleName` defaults to `''` (`scope: 'root'`), so editing that delegate calls `grantRootRoles` on `ROOT_RESOURCE` (`#89483`, `#90707` — Critical).
- **Persistent Root Roles on Shared Per-HCA `PermissionedResolver` Surviving Name Sale:** Registration grants the seller `ROLES_ALL` on `ROOT_RESOURCE` of their shared `PermissionedResolver`; transferring a name NFT resets registry roles but leaves the seller able to rewrite address records on the sold name (`#89264`, `#89279` — Critical).
- **V1-to-V2 Migration Granting `ROLE_SET_RESOLVER` to Former V1 Seller:** For any unwrapped V1 `.eth` name bought or gifted without calling `reclaim()`, `classifyNames.ts` sees `owner != registrant` and `buildAtomicMigrationBatches.ts` encodes `ETHRegistry.grantRoles(canonicalId, ROLE_SET_RESOLVER, formerSeller)` into the victim's V2 migration batch (`#89314`, `#89336`, `#89363` — Critical).
- **Patched Rhinestone SDK Session Mode Downgrade (`0x0205` $\rightarrow$ `0x0201` / `0x03`) & Swallowed Budget Quotes:**
  - Overriding same-chain standalone HCA session signatures to `SIG_MODE_ERC1271` (`0x0201`) skips the execution emissary callback and lets a browser session key drain 100% of the HCA's USDC via repeated `refundAmount = 50 USDC` fee settlements (`#91014` — Critical), or execute unvalidated `preClaimOps` in Permit2 mode `0x03` (`#92413` — High).
  - Swallowing a failed budget fetch (`catch(() => null)`) in `TokenPickerContent.tsx` displays `$8.00` base rent with `isEstimate = false` while `registration.machine.ts` independently fetches the budget and debits `20.20 USDC` via EIP-2612 `permit` (`#93021` — High).
- **Static Actor IDs (`tx-revoke-roles`) & Positional Table Row IDs (`"0"`, `"1"`):**
  - Hardcoding `id: 'tx-revoke-roles'` without evicting completed actors from `TransactionManager` makes every subsequent **Remove user** immediately display **Done** without sending a transaction (`#89431` — Critical).
  - Omitting `getRowId` in `DataTable.tsx` keys rows as `"0"`, `"1"`; deleting Row `"0"` shifts collaborator Row `"1"` into `"0"` without resetting `ResolverRolesSidebar`'s `editedPermissions`, silently granting `ROLE_UPGRADE` (`1 << 20`) to the collaborator on **Save** (`#90931` — Critical).

---

## Stage 5 & Stage 6: The 5-Step Severity Escalation & Triage-Proof PoC Playbook

Before finalizing any candidate finding, run this 5-step escalation and verification protocol (detailed in [references/immunefi-web3-apps-and-triage.md](references/immunefi-web3-apps-and-triage.md)):

### 1. Never Stop at the Intermediate Symptom — Trace to Terminal Asset / Authority Impact
- **Symptom $\rightarrow$ Terminal Impact Escalations:**
  - *Stale UI state / unkeyed row ID* $\rightarrow$ Trace the diff engine (`editedPermissions & ~row.roles`) to prove silent `grantRootRoles(ROLE_UPGRADE)` $\rightarrow$ `upgradeToAndCall` full contract takeover (`#90931`).
  - *Rounding down to `0` on sub-unit amount* $\rightarrow$ Loop `conversionFactor - 1` (`1e12 - 1` wei) so `account.debt < account.earmarked`, proving `redeem()` seizes the victim's collateral for already-burned debt (`#57053`), OR show that `collateralInCredited == 0` causes `collateralFactorInBasisPoints()` to return `0` and bypass `if (cf > requiredCF) revert` (`#36910`).
  - *Bracketed string passed to `labelhash()`* $\rightarrow$ Show how a low-privilege `ROLE_REGISTRAR` delegate plants `[<hex(labelhash("valuable"))>].parent.eth` so the admin's UI delete button calls `unregister(labelhash("valuable"))` (`#90725`).

### 2. Verify Against the 4 Triage Disqualification Traps
Never claim High/Critical if your scenario violates any of these four rules (either find an unprivileged vector or grade honestly):
1. **No Privileged / Admin Attacker Mocks:** Do not replace a trusted protocol contract with a `MaliciousEscrow` (`#57596` downgraded from Critical to Low) unless the parameter is callable by an unprivileged user or scoped agent (`#49863`, `#45893`).
2. **No Self-Inflicted Mid-Flight UI Races:** A bug that requires the victim to type a wrong address, click Submit, and rapidly edit the input box during a 200ms spinner (`#91822`) is Low, whereas a persistent state-machine actor leak across sessions (`#89431`, `#89433`) or an attacker-gifted unnormalized NFT (`#89278`) is Critical.
3. **Always Prove Insolvency / Freeze With a Second Honest User:** Do not stop at `assertGt(corruptedVar, 0)`. Include an honest user (`bob`) in the PoC and end with `vm.prank(bob); vm.expectRevert(); vault.withdraw(...)`, proving on-chain that funds are frozen or insolvent.
4. **Always Include Both a Positive Exploit Arm and a Baseline Control Arm:**
   - Use `uint256 snap = vm.snapshotState();` to run the honest baseline first (`honestPayout`), then `vm.revertToState(snap);` and run the exploit (`attackerPayout`), asserting `assertGt(attackerPayout, honestPayout)` (`#56555`, `#88287`).
   - For Web3 / dApp bugs, capture both the **visible DOM text** and the **exact EIP-1193 `eth_sendTransaction` / `eth_signTypedData_v4` payload**, execute the calldata on Anvil, and assert the post-state (`#89278`, `#93130`).

---

## Output: The Submittable Report

Produce one Markdown file per engagement from the template `experiments/smart-contract-audit/report/REPORT_TEMPLATE.md`, or generate it from structured findings:

```sh
python3 -I experiments/smart-contract-audit/report/generate_report.py \
  --meta engagement.json --findings findings.json --out report.md
```

Each finding records:
1. **Title & Verbatim Scope Mapping:** Exact Target URL, Asset Type (`Smart Contract`, `Blockchain/DLT`, `Websites and Applications`), Severity, and the **verbatim Impact category** from the bounty/competition scope table in the first 15 lines.
2. **Root Cause at the Producing Boundary:** File path, line numbers, exact code snippet, and why existing guards or known-issue patches do not prevent it (include a **"Why This Is Not Known Issue X"** comparison table when applicable).
3. **End-to-End Proof of Concept:** Self-contained test file (`forge test`, `cargo test`, `forc test`, `aptos move test`, `snforge test`, `pytest`, or Playwright + Anvil) with pinned commit SHA, exact single-command reproduction, positive exploit assertion + baseline control assertion, and full execution output logs.
4. **Quantified Impact & Remediation:** Exact funds at risk / invariant broken, plus a minimal diff at the producing boundary that makes the PoC fail.

## When You Cannot Confirm

If a candidate cannot be confirmed with a running PoC, report it as an **unconfirmed candidate** with its evidence tier and the specific check that would confirm or refute it. Never promote an unconfirmed candidate to a confirmed finding.
