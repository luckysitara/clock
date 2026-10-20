# Deep-Dive Synthesis of 1,176 Immunefi EVM Smart Contract Reports (Critical / High / Medium)

**Scope Studied:**
- Complete bundle of **1,176 EVM Smart Contract Medium/High/Critical reports** (`/home/rootkit/.gemini/antigravity-cli/brain/daac84dc-a996-43cc-9dfd-af7af613121a/scratch/bundles/evm_sc_mhc.md`, 63,569 lines)
- **43 full representative `.md` reports** read end-to-end (with full source excerpts and Foundry/Hardhat PoCs) across all 15 major EVM competitions:
  - `alchemix` (`#30634`, `#30651`, `#30655`, `#30683`, `#30860`, `#30939`, `#31042`, `#31078`)
  - `alchemix-v3` (`#56365`, `#56491`, `#56555`, `#56702`, `#56732`, `#56798`, `#56965`, `#57053`, `#57152`, `#58337`)
  - `plume-or-attackathon` (`#49854`, `#49863`, `#53037`, `#49616`, `#49700`, `#49710`, `#50713`)
  - `firelight-sep.2026-or-audit-competition` (`#88270`, `#88279`, `#88287`, `#88618`, `#88330`)
  - `belong` (`#56850`, `#57635`, `#57039`, `#57314`)
  - `base` (`#75413`, `#75423`)
  - `vechain-or-stargate-hayabusa` (`#59316`, `#59386`)
  - `yeet` (`#41215`, `#41280`)
  - `zerolend` (`#29031`, `#29062`)
  - `flare-fassets-or-mainnet-audit-comp` (`#47108`, `#45893`, `#45979`)
  - `puffer-finance` (`#28788`)
  - `term-structure-institutional_iop` (`#46903`, `#47115`)
  - `anvil` & `anvil-letters-of-credit` (`#36554`, `#36910`)
  - `folks-finance` (`#33780`)
  - `lombard` (`#38154`)
  - `ens-or-audit-competition` (`#90002`)

---

## 1. Complete Taxonomy of EVM Root-Cause Bug Classes (With Code Smells, Heuristics & Cited Examples)

Across the 1,176 reports, EVM Critical/High/Medium vulnerabilities cluster into **12 primary root-cause bug classes**. Below is the complete taxonomy with exact code-level patterns, step-by-step detection heuristics, and cited competition findings.

---

### Class 1: Mirrored / Aggregate vs. Per-Account State Desynchronization (Double-Counting & Omitted Updates)

#### Code-Level Smell / Vulnerable Pattern
Protocols frequently maintain both:
1. Per-user accounting (`account.debt`, `account.earmarked`, `account.collateralBalance`, `withdrawSharesOf[period][user]`), and
2. Global/aggregate counters (`totalDebt`, `cumulativeEarmarked`, `pendingWithdrawAssets`, `totalSupply`, `delegatorsEffectiveStake`), OR balance-delta inference (`token.balanceOf(transmuter) - lastTransmuterTokenBalance`).

Bugs occur when:
- **Path A (Double Deduction / Double Credit):** Function `X` (e.g., `repay()`) directly decrements `cumulativeEarmarked` and `totalDebt` AND transfers tokens to an external contract (`transmuter`), while Function `Y` (e.g., `redeem()`) later infers "unapplied coverage" from `transmuter.balanceOf() - lastTransmuterTokenBalance` and decrements `cumulativeEarmarked` and `totalDebt` a **second time**.
- **Path B (Omitted Aggregate Sync):** One branch (`_forceRepay` or `startUnstake` or `rescueWithdrawFromBlocklisted`) updates the per-user state or one aggregate counter without updating the paired counter (`cumulativeEarmarked`, `pendingWithdrawAssets`, or `delegatorsEffectiveStake`).
- **Path C (Clamped Deduction vs. Unclamped Payout):** A helper clamps the deduction from a user's balance (`account.collateralBalance -= fee > account.collateralBalance ? account.collateralBalance : fee`) but returns the **unclamped** `fee` to the caller, which transfers the full `fee` out of pooled funds.

```solidity
// SMELL: Clamped per-account deduction returns unclamped fee (#56365)
function _resolveRepaymentFee(uint256 accountId, uint256 fee) internal returns (uint256) {
    Account storage account = _accounts[accountId];
    // Deducts at most account.collateralBalance, but returns full `fee`!
    account.collateralBalance -= fee > account.collateralBalance ? account.collateralBalance : fee;
    return fee;
}
// Caller in _liquidate():
uint256 feeInYield = _resolveRepaymentFee(accountId, feeInYield);
TokenUtils.safeTransfer(myt, msg.sender, feeInYield + ...); // Drains pooled MYT!
```

#### Step-by-Step Detection Heuristic
1. **Build the State-Variable Matrix:** List every aggregate state variable (`totalDebt`, `cumulativeEarmarked`, `totalSyntheticsIssued`, `_mytSharesDeposited`, `pendingWithdrawAssets`, `totalSupply`, `totalCollateral`) alongside the sum of per-user fields it represents ($\sum \text{user.field}$).
2. **Trace Every Mutator:** For every function that touches *either* the per-user field or the aggregate variable (especially secondary flows: `_forceRepay`, `liquidate`, `burn`, `startUnstake`, `rageQuit`, `rescueWithdrawFromBlocklisted`, `merge`), verify that $\Delta(\text{Aggregate}) == \sum \Delta(\text{User})$.
3. **Check Hybrid Balance + Counter Accounting:** Does any function infer state deltas from `IERC20.balanceOf(target) - lastRecordedBalance` while another function transfers tokens to `target` AND manually updates the accounting counters? If yes, combining both flows double-counts the transfer.
4. **Verify Clamping Symmetry:** Whenever `min(a, b)` or ternary clamping `x > bal ? bal : x` is applied to a state deduction, check the return value and the subsequent `safeTransfer` amount.

#### Concrete Cited Examples
- **`#56365` (`alchemix-v3`, Critical) — `AlchemistV3._resolveRepaymentFee()` & `_liquidate()`:** `_resolveRepaymentFee` clamps the subtraction on `account.collateralBalance` when `fee > account.collateralBalance`, but returns the unclamped `fee`. `_liquidate` transfers `feeInYield` from pooled `myt`, draining all pooled collateral via repeated undercollateralized self-liquidations.
- **`#56965` (`alchemix-v3`, Critical) — `AlchemistV3.repay()` + `redeem()`:** `repay()` with MYT reduces `account.earmarked`, `cumulativeEarmarked`, and `totalDebt`, and sends MYT to `transmuter` without updating `lastTransmuterTokenBalance`. When `redeem()` runs next, it computes `deltaYield = transmuterBal - lastTransmuterTokenBalance` (which includes the repaid MYT) and subtracts `coverToApplyDebt` from `cumulativeEarmarked` and `totalDebt` a **second time**, corrupting global debt and `_survivalAccumulator`.
- **`#58337` (`alchemix-v3`, High) — `AlchemistV3._forceRepay()`:** Decrements `account.earmarked -= earmarkToRemove` and `account.debt -= creditToYield` during liquidation force-repay, but forgets to decrement `cumulativeEarmarked -= earmarkToRemove`. `cumulativeEarmarked` becomes permanently higher than $\sum \text{account.earmarked}$, inflating `liveEarmarked` in `redeem()` and corrupting `_survivalAccumulator`.
- **`#41215` (`yeet`, Critical) — `StakeV2.startUnstake()` & `accumulatedDeptRewardsYeet()`:** `startUnstake()` immediately decrements `totalSupply -= unStakeAmount` while keeping the tokens in `StakeV2` during `VESTING_PERIOD`. Because `accumulatedDeptRewardsYeet()` returns `stakingToken.balanceOf(address(this)) - totalSupply`, the pending unstake principal is misclassified as excess reward and zapped out in `executeRewardDistributionYeet()`, rendering `StakeV2` insolvent.
- **`#59386` (`vechain-or-stargate-hayabusa`, High) — `Stargate.unstake()` / `_delegate()` / `_updatePeriodEffectiveStake()`:** When a delegator requests exit, their effective stake is subtracted from the validator's checkpoint for the next period. If the validator later transitions to `VALIDATOR_STATUS_EXITED`, `unstake()` checks `if (currentValidatorStatus == VALIDATOR_STATUS_EXITED || delegation.status == DelegationStatus.PENDING)` and subtracts `effectiveStake` a **second time**, underflowing (`0x11` panic) and permanently freezing the user's staked VET.
- **`#30651` (`alchemix`, Critical) — `RevenueHandler.checkpoint()`:** Uses `uint256 thisBalance = IERC20(token).balanceOf(address(this)); epochRevenues[currentEpoch][token] += thisBalance;` instead of tracking `thisBalance - totalUnclaimed[token]`. Unclaimed revenue from prior epochs is re-added to `epochRevenues` every epoch, causing insolvency.

---

### Class 2: Checkpoint, Binary Search, & Boundary Off-By-One Errors (`<` vs `<=`, Duplicate Timestamps)

#### Code-Level Smell / Vulnerable Pattern
Time-weighted governance (`VotingEscrow`), staking periods (`Stargate`, `FirelightVault`), and Fenwick/Segment trees (`Transmuter.queryGraph`) rely on discrete block/timestamp/period checkpoints. Three recurring bugs appear:
1. **Binary Search with Duplicate Timestamps:** `_checkpoint()` appends a new entry to `pointHistory` every time it is called, even within the same `block.timestamp`. A binary search that terminates immediately on `if (lastPoint.ts == t) break;` returns an *arbitrary middle checkpoint* at timestamp `t` (depending on array length `upper`), whereas `balanceOfTokenAt` or another lookup returns the *latest* checkpoint at `t`.
2. **Strict vs. Inclusive Inequality (`<=` vs `<` or `>` vs `>=`) at Period/Block Boundaries:**
   - Returning `0` when `startBlock == endBlock` in `if (endBlock <= startBlock) return 0;` drops single-block queries (`lastEarmarkBlock + 1 == block.number`).
   - Using `endPeriod > nextClaimablePeriod` instead of `>=` causes the final claimable period to fall through into an unbounded active-delegator branch.
   - Using inclusive `upperLookupRecent(periodStart)` (`<=`) for period-start exposure while classifying a transaction at `block.timestamp == periodStart` as belonging to the *new* period.
3. **Missing Initial Checkpoint Fallback:** Relying on a mutable fallback (`fallbackComm = $.validators[validatorId].commission`) when no checkpoint exists for `[t_0, t_1]`, then mutating that variable later without backfilling `t_0`.

```solidity
// SMELL 1: Binary search breaking on first exact timestamp match when duplicate timestamps exist (#30655)
for (uint256 i = 0; i < 128; i++) {
    if (lower >= upper) break;
    uint256 center = upper - (upper - lower) / 2;
    Point memory lastPoint = pointHistory[center];
    if (lastPoint.ts == t) {
        lower = center; // BUG: If multiple points have ts == t, this picks `center`, not the last one!
        break;
    } else if (lastPoint.ts < t) {
        lower = center;
    } else {
        upper = center - 1;
    }
}

// SMELL 2: Off-by-one boundary check in range query (#56732)
function queryGraph(uint256 startBlock, uint256 endBlock) public view returns (uint256) {
    if (endBlock <= startBlock) return 0; // BUG: When startBlock == endBlock (1-block delta), returns 0!
    ...
}
```

#### Step-by-Step Detection Heuristic
1. **Duplicate-Timestamp Test:** Can `_checkpoint()` / `push()` be called twice in the same block (`block.timestamp`)? Does it overwrite in-place (`if (last.ts == block.timestamp)`) or append a new index? If it appends, check every binary search (`totalSupplyAtT`, `getPriorVotingIndex`, `upperLookup`) to see what index it returns when 2, 3, or $N$ entries share timestamp `t`, and whether growing the array later shifts `center = upper - (upper - lower) / 2`.
2. **Exact-Equality Boundary Substitution:** For every conditional involving block numbers, timestamps, or periods (`startBlock`, `endBlock`, `lastEarmarkBlock`, `periodStart`, `lastEpochEnd`, `nextClaimablePeriod`, `endPeriod`), plug in:
   - $A = B - 1$
   - $A = B$ (exact equality: consecutive blocks `lastBlock + 1 == block.number`, or transaction mined at `block.timestamp == periodStart` / `_lastEpochEnd`)
   - $A = B + 1$
3. **Compare Paired Readers:** When a ratio $\frac{\text{numerator}(t)}{\text{denominator}(t)}$ is computed (e.g., $\frac{\text{getPastVotes}(u, t)}{\text{totalSupplyAtT}(t)}$ or $\frac{\text{balanceOfAt}(t)}{\text{totalVoting}(t)}$), verify both functions use the exact same tie-breaking rule at timestamp `t`.

#### Concrete Cited Examples
- **`#30655` (`alchemix`, Critical) — `VotingEscrow.totalSupplyAtT()`:** Binary search breaks on `lastPoint.ts == t` instead of finding the latest checkpoint at `t`. An attacker calls `veALCX.checkpoint()` + `createLock()` in the same block at `proposalSnapshot`, then calls `veALCX.checkpoint()` after voting starts to shift `(upper - lower) / 2` so `totalSupplyAtT(proposalSnapshot)` lands on the pre-lock checkpoint while `getPastVotes` uses the post-lock checkpoint — inflating voting power above 100% of `totalSupplyAtT` to hijack governance and mint all ALCX.
- **`#30860` (`alchemix`, Critical) — `Bribe.earned()` & `Voter.distribute()`:** `Voter.distribute()` calls `Bribe.resetVoting()` (setting `totalVoting = 0`) and is callable at `block.timestamp >= _lastEpochEnd`. If called at exact `block.timestamp == _lastEpochEnd` followed by a tiny `Voter.vote()` in the same block, `votingCheckpoints` for `_lastEpochEnd` is overwritten with the tiny vote. In the next block, `Bribe.earned()` reads `_priorSupply = votingCheckpoints[getPriorVotingIndex(_lastEpochEnd)].votes` and divides by the tiny `_priorSupply`, letting the attacker drain 100% of epoch bribes.
- **`#56732` (`alchemix-v3`, Critical) — `Transmuter.queryGraph()`:** Uses `if (endBlock <= startBlock) return 0;`. Whenever `AlchemistV3._earmark()` is called in consecutive blocks (`block.number == lastEarmarkBlock + 1`), it calls `queryGraph(lastEarmarkBlock + 1, block.number)` where `startBlock == endBlock`. `queryGraph` returns `0`, permanently skipping that block's matured redemptions and systematically under-earmarking debt.
- **`#88287` (`firelight-sep.2026-or-audit-competition`, High) — `FirelightVault.withdraw()` & `totalAssetsAt()`:** At `block.timestamp == periodStart`, `currentPeriod()` has already advanced to `P`, so `withdraw()` places withdrawn assets into `withdrawAssets[P + 1]` (payable for period `P` claims). However, `withdraw()` also pushes the post-withdraw active balance (`0`) to `_traceTotalAssets` at `periodStart`. Because `totalAssetsAt(periodStart)` uses inclusive `upperLookupRecent(periodStart)` (`<=`), `assetsAtCapturePeriod` becomes `0`, capping `payout()` at `min(amount, min(0, activePayableAmount)) == 0` and letting stakers completely evade incident slashing.
- **`#59316` (`vechain-or-stargate-hayabusa`, High) — `Stargate._claimableDelegationPeriods()`:** Checks `if (endPeriod != type(uint32).max && endPeriod < currentValidatorPeriod && endPeriod > nextClaimablePeriod)`. Once an exited delegator claims up to `endPeriod`, `nextClaimablePeriod` becomes `endPeriod + 1` (or when `nextClaimablePeriod == endPeriod`), failing `endPeriod > nextClaimablePeriod` and falling through to `if (nextClaimablePeriod < currentValidatorPeriod) return (nextClaimablePeriod, completedPeriods);`. The exited NFT ("ghost stake") continues claiming VTHO rewards forever while its stake was already removed from the denominator `delegatorsEffectiveStake`.
- **`#53037` (`plume-or-attackathon`, Critical) — `RewardsFacet.addValidator()` / `getEffectiveCommissionRateAt()`:** `addValidator()` sets `$.validators[validatorId].commission` without pushing an initial commission checkpoint. When `setValidatorCommission()` later lowers commission to `0`, validator commission is settled up to now, but user claims for the initial interval fall back to `fallbackComm = $.validators[validatorId].commission` (`0`), retroactively applying the new 0% rate to past intervals and overpaying rewards.

---

### Class 3: Flash-Loan / Same-Block Snapshot Omission & Live Balance Manipulation

#### Code-Level Smell / Vulnerable Pattern
Governance gauges, redemption bad-debt scalers, and batched yield distributors read **live** balances (`balanceOf(msg.sender)`, `getTotalUnderlyingValue()`, `balanceOf(holder)`) instead of snapshotted or time-locked balances:
1. **Asymmetric Cast vs. Clear Weight:** `vote()` records weights or adds `weight * votingToken.balanceOf(msg.sender)` using live balance, or `clearVote()` subtracts `existing.weights[i] * votingToken.balanceOf(msg.sender)` using the *current* live balance instead of the balance at the time `vote()` was called.
2. **Spot TVL in Solvency / Bad-Debt Ratio:** `claimRedemption()` scales payouts by `badDebtRatio = totalSyntheticsIssued * 10**decimals / getTotalUnderlyingValue()`, where `getTotalUnderlyingValue()` increases immediately on `deposit()` and decreases on `withdraw()` in the same transaction.
3. **Cross-Batch Live Balance Iteration:** A multi-transaction batched distribution (`distributeYieldWithLimit(batchSize)`) iterates over an array of holders and reads `balanceOf(holder)` live in each batch without snapshotting at batch 0 or locking transfers.

```solidity
// SMELL: Subtracting live balance on clearVote/update instead of snapshotted voting power (#56798)
function clearVote(uint256 ytId) public {
    Vote memory existing = votes[msg.sender][ytId];
    for (uint256 i = 0; i < existing.strategyIds.length; i++) {
        // BUG: If votingToken.balanceOf(msg.sender) is now 0, subtracts 0 from aggStrategyWeight!
        aggStrategyWeight[ytId][existing.strategyIds[i]] -=
            existing.weights[i] * votingToken.balanceOf(msg.sender);
    }
    delete votes[msg.sender][ytId];
}
```

#### Step-by-Step Detection Heuristic
1. **Search for `.balanceOf(` and Spot TVL Getters:** Every time a contract reads `token.balanceOf(user)` or `vault.totalAssets()` / `getTotalUnderlyingValue()` to compute voting weight, reward share, or bad-debt scaling, ask: *Can the caller change this balance within the same transaction before and after this call?*
2. **Check Add/Remove Symmetry Across Balance Changes:** If `enter()` / `vote()` adds $f(\text{balance}_{t_1})$ to a global accumulator and `exit()` / `clearVote()` subtracts $f(\text{balance}_{t_2})$, what happens if the user transfers or flash-borrows tokens so $\text{balance}_{t_1} \gg \text{balance}_{t_2}$ (or vice versa)?
3. **Audit Multi-Batch Loops:** If a function processes indices `[startIndex, endIndex)` across multiple transactions, can an attacker move tokens/NFTs from an already-processed index $i < \text{startIndex}$ to an unprocessed index $j \ge \text{startIndex}$ between batches?

#### Concrete Cited Examples
- **`#56798` (`alchemix-v3`, Critical) — `PerpetualGauge.vote()` & `clearVote()`:** `PerpetualGauge` weights votes by `votingToken.balanceOf(msg.sender)` (`veALCX`, which supports `IERC3156FlashLender.flashLoan`). Furthermore, `clearVote()` subtracts `existing.weights[i] * votingToken.balanceOf(msg.sender)` using current balance. An attacker flash-borrows `veALCX`, calls `gauge.vote()` for a malicious strategy, repays the flash loan (so balance is `0`), and calls `gauge.clearVote()` — which subtracts `0` from `aggStrategyWeight`, permanently locking in massive ghost weight and draining all funds via `AlchemistAllocator`.
- **`#56555` (`alchemix-v3`, Critical) — `Transmuter.claimRedemption()`:** Computes `badDebtRatio = alchemist.totalSyntheticsIssued() * 10**decimals / alchemist.getTotalUnderlyingValue()`. When `badDebtRatio > 1e18`, redemptions are scaled down. An attacker sandwiches `claimRedemption()` with `alchemist.deposit(flashLoanAmount)` and `alchemist.withdraw(flashLoanAmount)` in the same transaction to temporarily drop `badDebtRatio <= 1e18`, claiming 100% unscaled collateral during bad debt and leaving the protocol insolvent.
- **`#49710` (`plume-or-attackathon`, High) — `ArcToken.distributeYieldWithLimit()`:** Processes `$.holders.at(i)` in batches and computes `share = (totalAmount * balanceOf(holder)) / effectiveTotalSupply` using live `balanceOf(holder)`. An attacker holding tokens in Address A (Batch 1) claims yield in Batch 1, transfers the tokens to Address B (Batch 3), and claims yield a second time in Batch 3.
- **`#30683` (`alchemix`, Critical) — `Voter.reset()` + `VotingEscrow.merge()`:** `onlyNewEpoch(_tokenId)` tracks whether a specific `tokenId` has voted/reset in the current epoch, not whether the underlying locked collateral has. An attacker calls `voter.reset(tokenId1)` to accrue FLUX, merges `tokenId1` into `tokenId2` via `veALCX.merge(tokenId1, tokenId2)`, and calls `voter.reset(tokenId2)` in the same epoch — repeating across $N$ small NFTs to mint unlimited FLUX in one block.

---

### Class 4: Rounding, Precision Loss, & Virtual-Offset Inversion (Dust Amplification & Zero-Output Exploits)

#### Code-Level Smell / Vulnerable Pattern
Integer division truncation (`a / b`) and ERC-4626 share conversion math fail in four distinct ways:
1. **Unchecked Zero Truncation on Unit Conversion:** Converting debt tokens (18 decimals) to underlying tokens (6 decimals) via `amount / 1e12` rounds down to `0` when `amount < 1e12`, allowing callers to burn `1e12 - 1` debt for `0` underlying cost (`toFree = 0` or `fee = 0`) in a loop.
2. **Repeated Timestamp Advancement on Zero-Flooring Delta:** A reward/commission accumulator updates `lastUpdateTime = block.timestamp` even when `(totalStaked * rewardPerTokenIncrease * commissionRate) / PRECISION**2 == 0`, permanently erasing accrued sub-precision rewards whenever anyone triggers an update early.
3. **ERC-4626 Virtual Offset Inversion (`totalAssets == 0 && totalSupply > 0`):** OpenZeppelin's virtual offset formula `shares = assets.mulDiv(totalSupply + 10**offset, totalAssets + 1, Floor)` assumes `totalAssets == 0` implies `totalSupply == 0`. If slashing or payouts reduce `totalAssets` to `0` *without burning shares* (`totalSupply > 0`), a `1 wei` deposit mints `1 * (totalSupply + 1) / 1 = totalSupply + 1` shares (>50% of the entire vault)!
4. **Zero-Check Bypass via Truncation in Health/Collateral Factor:** A health check computes `collateralInCredited = (collateral * price) / 10**exp` and returns `cfBasisPoints = 0` when `collateralInCredited == 0` ("to avoid division by zero"), then checks `if (cfBasisPoints > requiredCF) revert`, treating `0` (insolvent dust collateral) as healthy!

```solidity
// SMELL 1: ERC-4626 virtual offset when totalAssets() can be slashed to 0 while totalSupply() > 0 (#88330)
function _convertToSharesTotals(uint256 assets, uint256 totSupply, uint256 totAssets, Math.Rounding rounding) private view returns (uint256) {
    // When totAssets == 0 and totSupply == 20_000e6:
    // assets = 1 wei => shares = 1 * (20_000e6 + 1) / (0 + 1) = 20_000e6 + 1 (>50% of supply for 1 wei!)
    return assets.mulDiv(totSupply + 10 ** _decimalsOffset(), totAssets + 1, rounding);
}

// SMELL 2: Decimal normalization truncating to 0 without reverting (#57053)
function normalizeDebtTokensToUnderlying(uint256 amount) public view returns (uint256) {
    return amount / underlyingConversionFactor; // Returns 0 for amount < 1e12 (6-decimal underlying)!
}
```

#### Step-by-Step Detection Heuristic
1. **Check All Decimal-Conversion Divisions:** Locate every division by `10 ** (18 - decimals)` or `conversionFactor`. Substitute `amount = conversionFactor - 1`. Does the caller reduce debt/shares by `amount` while transferring or unlocking `0` underlying tokens?
2. **Test the `(totalAssets == 0, totalSupply > 0)` State in Vaults:** Can a vault's `totalAssets()` ever reach `0` (via slashing, `payout()`, bad debt, or strategy loss) while `totalSupply() > 0`? Plug `totAssets = 0, totSupply = 1e18, assets = 1` into `convertToShares`.
3. **Test the `(totalAssets > 0, totalSupply == 0)` Donation State:** Does `totalAssets()` use `asset.balanceOf(address(this))` without virtual shares or with `_decimalsOffset() == 0`? If an attacker donates `101e18` before the first `100e18` deposit, does `convertToShares` round to `0` (`#56850`)?
4. **Inspect `if (val == 0) return 0;` Early Returns:** Trace where that `0` return value goes. If the caller checks `if (returnedMetric > maxAllowed) revert`, a `0` return bypasses the check (`#36910`).
5. **Check Timestamp Updates on Zero Delta:** In reward/commission loops, if `delta == 0`, does `lastUpdateTime = block.timestamp` still execute (`#49700`)?

#### Concrete Cited Examples
- **`#88330` (`firelight-sep.2026-or-audit-competition`, Medium) — `FirelightVault._convertToSharesTotals()` & `payout()`:** `payout()` slashes vault assets down to `totalAssets() == 0` without burning shares (`totalSupply() == 20,000e6`). Because `_convertToSharesTotals` uses `assets.mulDiv(totSupply + 1, totAssets + 1)`, an attacker depositing `1 wei` post-slash mints `20,000,000,001` shares (>50% of total supply; `200 wei` mints 99.5%) and steals half of all future vault rewards.
- **`#57053` (`alchemix-v3`, Critical) — `AlchemistV3.normalizeDebtTokensToUnderlying()` & `_subDebt()`:** For 6-decimal USDC (`underlyingConversionFactor = 1e12`), calling `burn(1e12 - 1, victimTokenId)` reduces `account.debt` by `1e12 - 1` while `toFree = normalizeDebtTokensToUnderlying(1e12 - 1) == 0`, leaving `account.rawLocked` untouched. Worse, if `account.debt` is reduced below `account.earmarked`, `redeem()` seizes collateral equal to `earmarked` even though the debt was already burned!
- **`#36910` (`anvil-letters-of-credit`, Critical) — `LetterOfCredit.modifyLOCCollateral()` & `Pricing.collateralFactorInBasisPoints()`:** When a creator withdraws almost all collateral (leaving `1 wei`), `collateralAmountInCreditedToken` truncates `(1 * price) / 10**(-exponent)` to `0`. `collateralFactorInBasisPoints` sees `collateralInCredited == 0` and returns `0` to avoid division by zero. `modifyLOCCollateral` checks `if (cfBasisPoints > requiredCollateralFactorBasisPoints) revert`, which passes because `0 <= required`! The creator withdraws 99.9999% of the collateral from an active Letter of Credit.
- **`#49700` (`plume-or-attackathon`, High) — `PlumeRewardLogic.updateRewardPerTokenForValidator()`:** Computes `grossRewardForValidatorThisSegment = (totalStaked * rewardPerTokenIncrease) / REWARD_PRECISION` and `commissionDeltaForValidator = (grossReward * commissionRate) / REWARD_PRECISION`, then unconditionally sets `validatorLastUpdateTimes = block.timestamp`. By repeatedly staking/unstaking dust, an attacker keeps `commissionDeltaForValidator == 0` while advancing `validatorLastUpdateTimes`, blocking all validator commission.
- **`#56850` (`belong`, Critical) — `Staking.sol` (Solady `ERC4626`):** `totalAssets()` uses `SafeTransferLib.balanceOf(asset(), address(this))` with `_decimalsOffset() == 0` (`totalSupply() + 1` / `totalAssets() + 1`) and no `require(shares > 0)` check. Frontrunning a victim's `100e18` deposit with a `101e18` direct transfer causes `100e18 * 1 / 101e18 = 0` shares minted, stealing the victim's `100e18` deposit.

---

### Class 5: Retroactive Global Parameter Mutation Without Prior Accrual / Sync

#### Code-Level Smell / Vulnerable Pattern
A protocol computes user obligations, collateral decay, interest, or reward splits lazily using `delta = globalAccumulator - user.lastAccumulator` combined with a global configuration parameter (`minimumCollateralization`, `commission`, `rewardRate`, `resetNonce`). When an admin or operator updates the global parameter **without first checkpointing/syncing existing state under the old parameter**, the new parameter is retroactively applied to historical intervals or incompatible accumulator scales.

```solidity
// SMELL: Updating global parameter that scales historical accumulator deltas (#56491)
function setMinimumCollateralization(uint256 value) external onlyAdmin {
    // BUG: Updates minimumCollateralization without syncing existing accounts or scaling _collateralWeight!
    minimumCollateralization = value;
}
```

#### Step-by-Step Detection Heuristic
1. **Audit Every Setter Function (`set*`, `update*`, `remove*`, `reset*`):** List every state variable modified by an admin/operator/validator setter.
2. **Trace Lazy Consumers:** Find all functions that read that state variable inside a lazy calculation over a historical period $[t_{\text{last}}, t_{\text{now}}]$ or accumulator delta $(W_{\text{now}} - W_{\text{last}})$.
3. **Ask:** *If User A last interacted at $t_0$ when parameter $P = P_0$, and at $t_1$ the parameter changes to $P_1$, what value does User A's next interaction at $t_2$ compute for the interval $[t_0, t_1]$?*

#### Concrete Cited Examples
- **`#56491` (`alchemix-v3`, Critical) — `AlchemistV3.setMinimumCollateralization()`:** `_sync()` computes `collateralToRemove = PositionDecay.ScaleByWeightDelta(account.rawLocked, _collateralWeight - account.lastCollateralWeight)` where `account.rawLocked = (debt * minimumCollateralization) / 1e18`. When `setMinimumCollateralization` raises `minimumCollateralization`, existing accounts keep their old `rawLocked`, while subsequent redemptions increment `_collateralWeight` using the new `minimumCollateralization`, causing `_sync()` to over-remove collateral from existing users.
- **`#49616` (`plume-or-attackathon`, High) — `StakingFacet._initializeRewardStateForNewStake()` & `RewardsFacet.removeRewardToken()`:** When a user stakes from a `0` balance, `_initializeRewardStateForNewStake` sets `userValidatorRewardPerTokenPaidTimestamp = block.timestamp` only for *currently active* `$.rewardTokens`, skipping removed tokens. If `PUSD` was removed at $t=X$, re-added at $t=X+1$, accrued rewards until $t=X+500$, and removed again at $t=X+500$, a user who had staked/unstaked at $t=X$ and stakes a huge amount at $t=X+500$ retains `userValidatorRewardPerTokenPaidTimestamp == X` for `PUSD` and immediately claims 500 seconds of `PUSD` rewards they never earned.
- **`#36554` (`anvil`, Critical) — `TimeBasedCollateralPool.resetPool()` & `_resetAccountTokenStateIfApplicable()`:** When a pool is diluted by 99% and `resetPool()` is called during epoch $X$, contract-level pending unstakes for epoch $X+1$ are zeroed and the unit exchange rate resets to 1:1. However, a user who unstaked *before* the reset still has `accountStateStorage.firstPendingUnstakeEpoch = X + 1` and `firstPendingUnstakeUnits = 1000` (diluted units). When post-reset users deposit and unstake in epoch $X$, they populate `tokenEpochExitBalances[X + 1]` at the new 1:1 rate. In epoch $X+2$, `_resetAccountTokenStateIfApplicable` sees `epoch < currentEpoch` and releases the pre-reset user's 1000 diluted units against the post-reset 1:1 `ExitBalance`, draining post-reset depositors' USDC.

---

### Class 6: Lifecycle State-Machine Bypass & Poisoned Single-Use Flags

#### Code-Level Smell / Vulnerable Pattern
1. **Missing State Guard on Multi-Step Lifecycle:** A workflow requires `create -> settle -> {repay, liquidate, addCollateral, removeCollateral}`, where allowances are approved up-front at `create`. If `repay()`, `liquidate()`, or `addCollateralBeforeSettle()` omits `require(loan.settled)` or `require(block.timestamp <= settlement.expiryTime)`, an attacker can mutate or liquidate an unsettled/expired loan and pull tokens directly from the counterparty's pre-approved allowance!
2. **Single-Use Boolean Flag on an Additive/Reusable Bucket:** A mapping `isWithdrawClaimed[period][user] = true` is set when a slot is rescued or claimed, but the bucket `withdrawSharesOf[period][user] += shares` can still receive new entries for the same `(period, user)` key.
3. **Silent No-Op Return on KillSwitch / Pause After Token Transfer:** Caller transfers tokens to an adapter/strategy *before* calling `adapter.allocate()`, and when `killSwitch == true`, `adapter.allocate()` returns `(ids, 0)` instead of reverting, stranding the transferred tokens.

```solidity
// SMELL 1: Missing `if (!loanInfo.settled) revert` in post-settlement operations (#46903, #47115)
function addCollateralBeforeSettle(string memory _loanId, uint256 addedAmount) public nonReentrant {
    LoanInfo memory loanInfo = loans[loanId];
    if (loanInfo.settled) revert LoanAlreadySettled(loanId);
    // BUG: Increases loanInfo.debtData.collateralAmt in storage WITHOUT transferring tokens when takerType == LEND!
    loanInfo.addCollateral(addedAmount);
    loans[loanId] = loanInfo;
}
function repay(string memory _loanId, uint256 repayAmt, bool repayAll) external nonReentrant {
    LoanInfo memory loanInfo = loans[loanId];
    // BUG: Missing `if (!loanInfo.settled) revert LoanNotSettled(loanId);`!
    // Pulls `loanInfo.debtData.collateralAmt` from `loanInfo.lender` (who approved 10x allowance at createSettlement)!
}
```

#### Step-by-Step Detection Heuristic
1. **Draw the Explicit State Machine:** List all states of an entity (`NONE`, `CREATED/UNSETTLED`, `PENDING`, `ACTIVE/SETTLED`, `EXPIRED`, `LIQUIDATED`, `CLAIMED`, `EXITED`). For **every** external/public function, check which states it explicitly rejects and which states it implicitly allows.
2. **Check Pre-Approved Allowances Against Unsettled States:** Whenever a protocol requires users/lenders to `approve()` a contract before an order/loan is matched or settled, check if `repay`, `liquidate`, `cancel`, or `withdraw` can be called *before* `settle()` transfers the principal.
3. **Audit Boolean "Consumed" Flags (`isClaimed`, `withdrawn`, `settled`):** Is the key of the boolean flag `(period, user)` capable of receiving additional deposits (`withdrawSharesOf[period][user] += ...`) *after* the flag is set to `true`?
4. **Check Pre-Transfer + Non-Reverting Early Return:** If Contract A transfers tokens to Contract B and then calls `B.process()`, does `B.process()` have any early-return branch (`if (paused || killSwitch) return 0;`) that doesn't revert?

#### Concrete Cited Examples
- **`#46903` & `#47115` (`term-structure-institutional_iop`, Critical) — `Settlement.addCollateralBeforeSettle()`, `repay()`, `liquidate()`:** At `createSettlement()`, the lender must approve `10 * totalCollateralAmt` to `Settlement`. Before `settle()` is ever called, a malicious borrower calls `addCollateralBeforeSettle(loanId, 5000 ether)` (which increments `loanInfo.debtData.collateralAmt` without transferring tokens) and then calls `repay()` or `liquidate()` (which both lack `require(loanInfo.settled)`), pulling `5000 ETH` directly from the lender's wallet via `safeTransferFrom(lender, borrower, collateralAmt)`!
- **`#88270` (`firelight-sep.2026-or-audit-competition`, High) — `FirelightVault.rescueWithdrawFromBlocklisted()`:** Marks `isWithdrawClaimed[period][from] = true` when rescuing a pending withdrawal in `period = currentPeriod() + 1`. If `from` is later un-blocklisted within the same period `currentPeriod()` and makes another withdrawal request, `_requestWithdraw` adds to `withdrawSharesOf[period][from] += shares` and `pendingWithdrawAssets += assets`, but `isWithdrawClaimed[period][from]` is already `true` — permanently bricking both `claimWithdraw(period)` and any second `rescueWithdrawFromBlocklisted`, and permanently understating `totalAssets()` for all LPs.
- **`#57152` (`alchemix-v3`, High) — `VaultV2.allocateInternal()` & `MYTStrategy.allocate()`:** `VaultV2.allocateInternal` transfers `assets` to `adapter` via `SafeERC20.safeTransfer(asset, adapter, assets)` *before* calling `IAdapter(adapter).allocate(...)`. When `MYTStrategy.killSwitch == true`, `allocate()` returns `(ids(), int256(0))` instead of reverting, leaving `vault.allocation` unchanged at `0` while the tokens are permanently stranded inside the strategy.
- **`#33780` (`folks-finance`, Critical) — `UserLoanLogic.increaseCollateral()`:** Checks `if (loan.collaterals[poolId].balance == 0) loan.colPools.push(poolId);` before `loan.collaterals[poolId].balance += fAmount;` without requiring `fAmount > 0`. Calling `deposit(..., 0)` 20 times pushes the same `poolId` into `loan.colPools` 20 times; a single real deposit then gets counted 21 times in `getLoanLiquidity()`, inflating collateral value $21\times$ to drain all lending pools.

---

### Class 7: Unvalidated External Call Parameters, Router/Wrapper Approvals, & Leftover Refunds

#### Code-Level Smell / Vulnerable Pattern
1. **Arbitrary Router Calldata + Unverified Receiver + User-Controlled Approval Target:** A DEX aggregator wrapper or zapper accepts arbitrary swap calldata (`data`) and user-supplied structs (`teller`, `executor`), grants token approvals to `address(teller.vault())` without validating `teller`, and fails to verify that the swap's `dstReceiver` is `address(this)` or that `balanceAfter - balanceBefore >= minOut`.
2. **Partial Fill / Unspent Input Stranded in Intermediary:** A wrapper pulls `amountIn` from `msg.sender`, approves a router (1inch, OKX, Kodiak), executes a swap that partially fills (`spent < amountIn`), and never refunds `amountIn - spent` to `msg.sender`.
3. **Intermediary `msg.sender` Confusion on Refunds:** Contract A (`StakeV2`) calls Contract B (`Zapper.zapOut()`), and Contract B refunds unswapped leftover tokens to `_msgSender()` (`StakeV2`, which has no sweep/recovery function) instead of `receiver` (the user).
4. **Blind Trust of External `.claim()` Return Value:** A pool allows an operator/agent to pass an external contract address (`IRewardManager _rewardManager`) and increments `totalCollateral += _rewardManager.claim(...)` without whitelisting `_rewardManager` or checking actual `balanceOf` delta.

```solidity
// SMELL: Approving a user-controlled contract and trusting swap calldata in a wrapper (#49863)
function _okxHelper(
    ERC20 supportedAsset, ..., TellerWithMultiAssetSupport teller, bytes calldata data
) internal returns (uint256 supportedAssetAmount) {
    // 1. Arbitrary swap calldata `data` sent to okxRouter; dstReceiver can be attacker!
    (bool success, bytes memory result) = okxRouter.call{value: value}(data);
    // 2. `teller` is user-controlled! Approves attacker's fake `teller.vault()` to spend wrapper's tokens!
    supportedAsset.safeApprove(address(teller.vault()), supportedAssetAmount);
    teller.deposit(supportedAsset, supportedAssetAmount, 0);
}
```

#### Step-by-Step Detection Heuristic
1. **Audit Every Wrapper/Zapper/Adapter Contract:** Check every parameter passed into external swap/zap functions:
   - Are `teller`, `router`, `vault`, `rewardManager`, or `distribution` addresses validated against an on-chain registry/whitelist?
   - Does the wrapper measure `uint256 balBefore = token.balanceOf(address(this))` and `balAfter` around the router call, or does it trust the router's return value / user's `dstReceiver`?
   - What happens to unspent input tokens (`amountIn - actualSpent`) on a partial fill?
   - When Contract A calls Contract B on behalf of a user, where do dust/refund transfers in Contract B go (`msg.sender` vs `user`/`receiver`)?

#### Concrete Cited Examples
- **`#49863` (`plume-or-attackathon`, Critical) — `DexAggregatorWrapperWithPredicateProxy._okxHelper()`:** Takes a user-supplied `TellerWithMultiAssetSupport teller` and raw `bytes calldata data` for `okxRouter`. It neither validates `teller` nor checks that `okxRouter` sent output tokens to `address(this)`. An attacker passes a malicious `teller` whose `.vault()` returns the attacker's address, stealing both swap outputs and all existing ERC-20 tokens sitting in the wrapper via `supportedAsset.safeApprove(address(teller.vault()), supportedAssetAmount)`.
- **`#49854` (`plume-or-attackathon`, Critical) — `DexAggregatorWrapperWithPredicateProxy._oneInchHelper()`:** Pulls the full `srcToken` amount from the user into the wrapper before calling 1inch `aggregationRouter.call(data)`. Because 1inch supports partial fills, unspent `srcToken` remains stranded in the wrapper (and can be stolen via `#49863`).
- **`#41280` (`yeet`, High) — `StakeV2.claimRewardsIn*()` & `Zapper.zapOut*()`:** When a user claims rewards in a single token via `StakeV2`, `StakeV2` calls `Zapper`, making `StakeV2` the `_msgSender()` inside `Zapper`. `Zapper` sends any unswapped `token0Debt` / `token1Debt` to `_msgSender()` (`StakeV2`) instead of `receiver`, permanently freezing the user's unswapped yield inside `StakeV2`.
- **`#45893` (`flare-fassets-or-mainnet-audit-comp`, High) — `CollateralPool.claimAirdropDistribution()` & `claimDelegationRewards()`:** Takes an arbitrary `IDistributionToDelegators _distribution` / `IRewardManager _rewardManager` parameter from `onlyAgent`, calls `uint256 claimed = _distribution.claim(...)`, and adds `totalCollateral += claimed` without verifying `_distribution` or checking actual native/token balance received. A malicious agent passes a mock contract returning `10000 ether`, inflating `totalCollateral` and draining all users' FLR on `exit()`.
- **`#30939` (`alchemix`, Critical) — `RevenueHandler._melt()`:** Calls `IPoolAdapter(poolAdapter).melt(revenueToken, debtToken, revenueTokenBalance, revenueTokenBalance)` passing `revenueTokenBalance` (the input amount) as `minimumAmountOut`! When `revenueToken` and `debtToken` have different decimals or prices, `_melt()` either reverts permanently (freezing revenue) or uses zero slippage protection.

---

### Class 8: Partial Payout / Slashing Without Burning Shares or Adjusting Debt Return

#### Code-Level Smell / Vulnerable Pattern
When a vault, collateral pool, or redemption queue has less liquidity than a user's entitlement (`available < entitled`), the contract caps the payout via `paid = Math.min(entitled, available)` to avoid reverting, **but still burns 100% of the user's shares/synthetic tokens** (or fails to refund the unfulfilled portion of synthetic debt).

```solidity
// SMELL: Capping payout to available balance while burning full user shares/tokens (#47108, #56702)
function payoutFromVault(Agent.State storage _agent, address _receiver, uint256 _amountWei) internal returns (uint256 _amountPaid) {
    CollateralTypeInt.Data storage collateral = getVaultCollateral(_agent);
    // Pays only min(entitled, balance), while caller CollateralPool._selfCloseExitTo() burns 100% of user's CPT shares!
    _amountPaid = Math.min(_amountWei, collateral.token.balanceOf(address(vault)));
    vault.payout(collateral.token, _receiver, _amountPaid);
}
```

#### Step-by-Step Detection Heuristic
1. **Search for `Math.min` or Ternary Caps in Payout/Redeem Flows:** Every time a payout is capped to `balanceOf(vault)` or `balAfterRedeem`:
   - Trace what happens to the user's input shares/tokens (`token.burn(...)`, `_burn(...)`).
   - Are the burned shares scaled down proportionally to `paid / entitled`, or is the unfulfilled portion (`entitled - paid`) returned/preserved?
2. **Trace External Slashing in 2-Step Async Withdrawals:** In liquid restaking (EigenLayer, Symbiotic, Babylon), if a queued withdrawal is slashed externally between `initiateWithdrawal` and `completeWithdrawal`:
   - Does `completeWithdrawal` revert permanently (leaving `pendingWithdrawalShares` inflated forever, as in `#28788`)?

#### Concrete Cited Examples
- **`#56702` (`alchemix-v3`, Critical) — `Transmuter.claimRedemption()`:** Caps `distributable = totalYield <= balAfterRedeem ? totalYield : balAfterRedeem` when `AlchemistV3.redeem()` returns less MYT than `totalYield` (e.g., due to VaultV2 redeem fees or rounding). However, `syntheticReturned` is still calculated only from `amountNottransmuted - syntheticFee`, and the entire `position.amount` of `alAsset` is burned — permanently destroying the user's unredeemed `alAsset` without giving them MYT.
- **`#47108` (`flare-fassets-or-mainnet-audit-comp`, High) — `CollateralPool._selfCloseExitTo()` & `Agents.payoutFromVault()`:** When a user exits with `_redeemToCollateral = true`, `payoutFromVault` caps the payout at `Math.min(_amountWei, collateral.token.balanceOf(address(vault)))` without reverting or scaling down `_tokenShare`, while `_selfCloseExitTo` burns the user's full `_tokenShare` of CPT pool tokens.
- **`#28788` (`puffer-finance`, Critical) — `PufferVault.claimWithdrawalFromEigenLayer()`:** EigenLayer's `slashQueuedWithdrawal()` sets `withdrawalRootPending[withdrawalRoot] = false`. When Puffer subsequently calls `claimWithdrawalFromEigenLayer()`, `_EIGEN_STRATEGY_MANAGER.completeQueuedWithdrawal()` permanently reverts on `require(withdrawalRootPending[withdrawalRoot])`. Because it reverts, `$.eigenLayerPendingWithdrawalSharesAmount -= queuedWithdrawal.shares[0]` can never execute, leaving `getELBackingEthAmount()` and `totalAssets()` permanently over-inflated (protocol insolvency).

---

### Class 9: Non-Fungible Lock / Vesting Ledger Desynchronization on Transfer, Split, or Unstake

#### Code-Level Smell / Vulnerable Pattern
Protocols combine fungible ERC-20/ERC-4626 shares with non-fungible time-locks (`mapping(address => Stake[]) stakes` or `TokenLocker` NFTs):
1. **ERC-4626 Share Transfer Without Lock-Ledger Hook:** Deposit pushes `{shares, timestamp}` to `stakes[to]`, and withdraw consumes unlocked entries from `stakes[owner]`, but `ERC20.transfer()` moves shares without moving `stakes` entries (`#57635`).
2. **Fungible Voting Power Used to Unstake Non-Fungible Lock Positions:** Users stake NFTs with different `(amount, lockDuration)` into a pool that mints fungible voting power `power = amount * duration`, and `unstakeToken(uint256 tokenId)` allows *anyone* who burns `power(tokenId)` to withdraw *any* `tokenId` (`#29062`).
3. **Missing Ownership Check on NFT Split/Merge:** `split(uint256 tokenId, uint256 fraction)` checks `_requireOwned(tokenId)` (which in OpenZeppelin ERC721 v5 only checks that `tokenId` *has an owner*, i.e., `ownerOf(tokenId) != address(0)`, NOT that `msg.sender == ownerOf(tokenId)`!) and mints the split NFT to `msg.sender` (`#29031`).

```solidity
// SMELL: Misusing OZ v5 `_requireOwned(tokenId)` thinking it checks `msg.sender == owner` (#29031)!
function split(uint256 tokenId, uint256 fraction) external {
    _requireOwned(tokenId); // BUG: In OZ ERC721, _requireOwned only checks that tokenId is minted, NOT msg.sender!
    ...
    _mint(msg.sender, ++lastTokenId); // Mints 99.99% of victim's vested tokens to msg.sender!
}
```

#### Step-by-Step Detection Heuristic
1. **Audit `_requireOwned(tokenId)` Calls:** In OpenZeppelin v5 `ERC721`, `_requireOwned(tokenId)` only verifies that `tokenId` exists (`_ownerOf(tokenId) != address(0)`). Any function that mutates, splits, merges, stakes, or claims from `tokenId` MUST check `_isAuthorized(owner, msg.sender, tokenId)` or `ownerOf(tokenId) == msg.sender`!
2. **Check Fungible-to-Non-Fungible Bridges:** If an NFT is deposited into a staking/bridge contract in exchange for fungible shares/points, does `unstake(tokenId)` verify `stakerOf[tokenId] == msg.sender`?
3. **Check `_update` / `_beforeTokenTransfer` Hooks:** If a contract has both `ERC20`/`ERC4626` balances and a separate per-address array/mapping (`stakes[user]`, `rewardDebt[user]`, `locks[user]`), trace what happens when `transfer()` or `transferFrom()` is called.

#### Concrete Cited Examples
- **`#29031` (`zerolend`, Critical) — `VestedZeroNFT.split()`:** Calls `_requireOwned(tokenId)` without checking `ownerOf(tokenId) == msg.sender`, then splits `tokenId` and mints `++lastTokenId` with `1 - fraction/denominator` of the vesting balance directly to `msg.sender`. Anyone can steal 99.99% of every `VestedZeroNFT` in the contract.
- **`#29062` (`zerolend`, Critical) — `OmnichainStaking.unstakeToken()`:** Users lock ZERO in `TokenLocker` where voting power is `amount * duration`. Alice locks `40 ZERO` for 2 years (`power = 80`); attacker locks `20 ZERO` for 4 years (`power = 80`) and stakes NFT #2 in `OmnichainStaking`. Because `unstakeToken(uint256 tokenId)` only burns the required voting power from `msg.sender` without checking who deposited `tokenId`, the attacker calls `unstakeToken(1)` and steals Alice's `40 ZERO` NFT.
- **`#57635` (`belong`, Critical) — `Staking.sol`:** Inherits Solady `ERC4626` and records time-locks in `stakes[to].push(Stake(shares, block.timestamp))` on `_deposit`, requiring `_consumeUnlockedSharesOrRevert(_owner, shares)` on `_withdraw`. Because `_beforeTokenTransfer` is not overridden, transferring ERC-4626 shares to a recipient leaves `stakes[recipient]` empty, permanently blocking standard `withdraw`/`redeem` for all transferred shares.

---

### Class 10: Signature, Calldata-Binding, & Cross-Chain Message Substitution Vulnerabilities

#### Code-Level Smell / Vulnerable Pattern
1. **Approval Calldata Missing Mutable Version/Round ID (TOCTOU):** A multi-sig/consortium signs `approveCurrentAssessment(uint256 incidentId)` where the contract resolves `incident.currentAssessmentRoundId` from storage at execution time. A lower-privilege role (`CURATOR_ROLE`) cancels round 1, submits a malicious round 2 under the same `incidentId`, and broadcasts the consortium's unchanged signed transaction (`#88618`).
2. **`abi.encodePacked` Collision Across Dynamic Types:** Hashing multiple dynamic strings/bytes (`abi.encodePacked(name, symbol, contractURI, ...)`) without length delimiters allows `"USDC" + "USDC" == "USD" + "CUSDC"` signature collisions (`#57314`).
3. **Unbound Off-Chain Data in Cross-Chain Bridges (CCIP / LayerZero):** In Chainlink CCIP `releaseOrMint(ReleaseOrMintInV1)`, `offchainTokenData` is supplied by the manual executor in `manuallyExecute` and is **not** authenticated by CCIP's Merkle root unless the token pool compares it against `releaseOrMintIn.sourcePoolData` (`extraData`) (`#38154`).

```solidity
// SMELL 1: Approval function binds container ID (`incidentId`) but reads mutable `currentAssessmentRoundId` from storage (#88618)
function approveCurrentAssessment(uint256 incidentId) external onlyRole(ASSESSMENT_APPROVER_ROLE) {
    Incident storage incident = _activeIncidentWithStatus(incidentId, IncidentStatus.UNDER_EVALUATION);
    // BUG: Resolves `incident.currentAssessmentRoundId` at execution time! Curator can swap round 1 -> round 2!
    (uint256 assessmentRoundId, AssessmentRound storage assessmentRound) = _setCurrentAssessmentStatus(...);
    _executePayout(..., assessmentRound.totalAssessmentLoss);
}
```

#### Step-by-Step Detection Heuristic
1. **Inspect Every Function Called by a Multisig / Timelock / Approver Role:** Does the function take only a high-level ID (`incidentId`, `proposalId`, `orderId`) while reading a mutable sub-pointer (`currentRoundId`, `payloadHash`, `schedule`) that another role can modify before the transaction is mined?
2. **Grep for `abi.encodePacked(`:** If `abi.encodePacked` has two or more dynamic types (`string`, `bytes`, `address[]`, `uint256[]`), it is vulnerable to hash collision; if used in signature verification, check for missing `nonce`, `chainId`, `address(this)`, or EIP-712 domain separator.
3. **Check Dual-Channel Bridge Payloads (CCIP `offchainTokenData` vs `sourcePoolData`):** Whenever a bridge handler receives both an on-chain message (`sourcePoolData` / `amount`) and an auxiliary proof/payload (`offchainTokenData`), verify that `keccak256(payload) == keccak256(sourcePoolData)` and `action.amount == releaseOrMintIn.amount`.

#### Concrete Cited Examples
- **`#88618` (`firelight-sep.2026-or-audit-competition`, High) — `IncidentManager.approveCurrentAssessment()`:** Takes only `uint256 incidentId` and resolves `incident.currentAssessmentRoundId` at execution. After the Risk Consortium reviews a 100-unit loss schedule in Round 1 and signs the Safe transaction `approveCurrentAssessment(1)`, the curator calls `cancelCurrentAssessment(1)`, submits a 900-unit loss schedule in Round 2, and executes the consortium's original signature — slashing 900 units from `FirelightVault`.
- **`#38154` (`lombard`, Medium) — `CLAdapter.initiateWithdrawal()` & `LBTCTokenPool.releaseOrMint()`:** During manual execution on CCIP `OffRamp`, anyone can pass arbitrary `offchainTokenData` alongside a victim's CCIP message. Because `LBTCTokenPool` and `CLAdapter` never verify that `offchainTokenData`'s `payload` matches `releaseOrMintIn.sourcePoolData` (`extraData`), an attacker attaches a valid notarized `offchainTokenData` for a tiny `1 satoshi` transfer to the victim's `10 LBTC` CCIP message, minting `1 satoshi`, marking the victim's `10 LBTC` CCIP message as `SUCCESS`, and permanently freezing the victim's `10 LBTC`.
- **`#57314` (`belong`, Medium) — `SignatureVerifier.checkAccessTokenInfo()`:** Uses `keccak256(abi.encodePacked(name, symbol, contractURI, feeNumerator, block.chainid))` without nonces or length delimiters, allowing a valid signature for `("USDC", "USDC")` to be replayed by another user to deploy `("USD", "CUSDC")`.

---

### Class 11: Role Hierarchy Bypass, Stale Migration Authority, & Parameter Bound Abuse

#### Code-Level Smell / Vulnerable Pattern
1. **OpenZeppelin `DEFAULT_ADMIN_ROLE` Hierarchy Bypass:** A factory (`ArcTokenFactory`) grants `DEFAULT_ADMIN_ROLE` (`0x00`) on a newly deployed UUPS proxy to `msg.sender` (the token deployer) while granting `UPGRADER_ROLE` to `address(this)` (the factory) to enforce an `allowedImplementations` whitelist. Because `DEFAULT_ADMIN_ROLE` is the default `adminRole` for all roles in `AccessControlUpgradeable` (unless `_setRoleAdmin(UPGRADER_ROLE, ...)` is called), `msg.sender` simply calls `token.grantRole(UPGRADER_ROLE, msg.sender)` and upgrades the proxy to arbitrary bytecode (`#50713`).
2. **Dangerous Parameter Bounds Allowing 10,000x Share Inflation:** An operator/agent setter checks `require(_topupTokenPriceFactorBIPS > 0 && _topupTokenPriceFactorBIPS < 10000)` instead of enforcing a safe floor (e.g., `>= 9000`). Setting `topupTokenPriceFactorBIPS = 1` multiplies minted shares by `10000 / 1 = 10,000x` (`#45979`).
3. **Stale Indexed Authority Used in Migration:** A migration flow (`ENS V1 -> V2`) reads a cached/subgraph `managerAddress` (`staleTime: 5 min`) without re-reading the authoritative on-chain `V1Registry.owner(node)` at execution time, restoring `ROLE_SET_RESOLVER` to an already-revoked attacker (`#90002`).

#### Step-by-Step Detection Heuristic
1. **Check `_setRoleAdmin` Whenever `DEFAULT_ADMIN_ROLE` Is Granted Externally:** In OpenZeppelin `AccessControl`, `getRoleAdmin(ROLE)` returns `0x00` (`DEFAULT_ADMIN_ROLE`) by default. If a user is given `DEFAULT_ADMIN_ROLE` on a child contract, they can grant themselves *every* role on that contract unless `_setRoleAdmin(RESTRICTED_ROLE, FACTORY_ROLE)` was explicitly called.
2. **Substitute Extremes (`1` and `MAX - 1`) into Every Divisor Parameter:** Whenever a configurable BIPS parameter is used in a denominator (`mulDiv(MAX_BIPS, factorBIPS)`), substitute the minimum allowed value (`1`) and check if it allows hyper-inflation of shares or collateral.

#### Concrete Cited Examples
- **`#50713` (`plume-or-attackathon`, High) — `ArcTokenFactory.createToken()`:** Grants `DEFAULT_ADMIN_ROLE` to `msg.sender` and `UPGRADER_ROLE` to `address(this)` without changing `UPGRADER_ROLE`'s role admin. The deployer calls `grantRole(UPGRADER_ROLE, msg.sender)` and `upgradeToAndCall(maliciousImpl)`, completely bypassing `ArcTokenFactory.upgradeToken()`'s `allowedImplementations` whitelist.
- **`#45979` (`flare-fassets-or-mainnet-audit-comp`, High) — `CollateralPool.setTopupTokenPriceFactorBIPS()`:** Allows an agent to set `topupTokenPriceFactorBIPS = 1` (`> 0`). In `_collateralToTokenShare`, `collateralForTopupPricing.mulDiv(10000, 1)` inflates the agent's minted pool shares by $10,000\times$, letting the agent deposit `10 FLR` to mint `100,000 PCT` and drain >90% of the FLR deposited by honest pool holders.
- **`#90002` (`ens-or-audit-competition`, Medium) — `computeMigrationPreflight.ts` / `migrationService.ts`:** Recomputes migration plans from a cached V1 domain snapshot without re-querying `V1Registry.owner(node)` on-chain, granting V2 `ROLE_SET_RESOLVER` to a V1 manager whom the victim had already revoked prior to migration.

---

### Class 12: L2 / Rollup Proof-Executor & Canonical EVM Divergence

#### Code-Level Smell / Vulnerable Pattern
In L2/ZK-rollup proof systems (such as Base Azul's SP1 + TEE `AggregateVerifier`), the off-chain/guest proof executor must construct the exact same EVM block environment and fork activation rules as the canonical L2 node:
1. **Header Field Repurposing Divergence (`BLOBBASEFEE`):** Canonical Base execution hardcodes `blob_gasprice = 1` and repurposes the post-Jovian header's `blob_gas_used` field to store Base's DA footprint. The ZK proof executor instead feeds `parent_header.blob_gas_used` into Ethereum's EIP-4844/Prague `BlobExcessGasAndPrice::new()`, producing `block.blobbasefee > 1` inside the ZK proof when DA footprint is high (`#75413`).
2. **Block Number Passed to Timestamp Fork Guard (`is_isthmus_active`):** The ZK range client calls `cfg.is_isthmus_active(l2_safe_head.block_info.number)` (passing a block number `~40,000,000` into a function expecting a Unix timestamp `~1,700,000,000`), causing the guard to evaluate to `false` and accept a truncated 20-block execution as a valid 600-block state root transition (`#75423`).

#### Step-by-Step Detection Heuristic
1. **Diff Canonical EVM Env Builder vs. Proof Executor Env Builder:** Compare every field of `BlockEnv` / `CfgEnv` (`blob_excess_gas_and_price`, `basefee`, `prevrandao`, `spec_id`) between `crates/execution/evm` and `crates/proof/executor`. Any opcode (`BLOBBASEFEE`, `BASEFEE`, `DIFFICULTY`) that can return a different value between canonical execution and proof execution allows an attacker to write `if (block.blobbasefee > 1) { L2ToL1MessagePasser.initiateWithdrawal(...); }` to forge withdrawals on L1!
2. **Check Units on Fork Activation Calls:** Verify every `is_<fork>_active(arg)` call to confirm whether `arg` is `block.timestamp` or `block.number`, and verify that the witness executor asserts `safe_head.block_info.number == boot.claimed_l2_block_number` after derivation finishes.

---

## 2. Protocol-Specific Deep-Dive Playbooks

### Playbook A: Lending, CDPs, Synthetics & Self-Repaying Loans (`alchemix`, `alchemix-v3`, `zerolend`, `folks-finance`, `term-structure`, `anvil`)
1. **Dual Debt States (Active Debt vs. Earmarked / Transmuted Debt):**
   - In self-repaying loans (`AlchemistV3`), user debt splits into `account.debt` (total) and `account.earmarked` (queued for redemption), mirrored globally by `totalDebt` and `cumulativeEarmarked`.
   - **Check every debt-reducing action** (`burn`, `repay`, `_forceRepay`, `liquidate`, `redeem`):
     - Does `repay()` with yield tokens transfer tokens to `Transmuter` *without* updating `lastTransmuterTokenBalance`, causing the next `redeem()` to double-subtract `coverToApplyDebt` (`#56965`)?
     - Does `_forceRepay()` reduce `account.earmarked` without reducing `cumulativeEarmarked` (`#58337`)?
     - Can `burn()` reduce `account.debt` below `account.earmarked` using dust amounts (`conversionFactor - 1`) that round `toFree` to `0` (`#57053`)?
     - Does `liquidate()` settle earmarked debt first, or does it double-count `account.earmarked` (`#56360`, `#56393`)?
2. **Liquidation Fee Clamping vs. Pooled Collateral Payout:**
   - When an account is deeply underwater (`account.collateralBalance < protocolFee + liquidatorFee`), trace `_resolveRepaymentFee`: if the deduction from `account.collateralBalance` is clamped to `account.collateralBalance`, does the return value still equal the unclamped `fee`, pulling the difference from other users' pooled collateral (`#56365`)?
3. **Bad-Debt Scaling Manipulation via Flash Deposit/Withdraw:**
   - If redemption payouts (`Transmuter.claimRedemption`) scale down when `totalSyntheticsIssued / getTotalUnderlyingValue() > 1`, check if `getTotalUnderlyingValue()` can be inflated within the same transaction via `deposit()` + `claimRedemption()` + `withdraw()` (`#56555`).
4. **Loan Lifecycle & Pre-Settlement Allowances:**
   - In institutional/orderbook lending (`Term Structure Settlement.sol`), check whether `repay()`, `liquidate()`, `addCollateral()`, or `removeCollateral()` can be called when `loan.settled == false` or `block.timestamp > settlement.expiryTime`, exploiting the lender's pre-approved `10x` collateral allowance (`#46903`, `#47115`).
5. **Duplicate Collateral Array Entries via Zero-Amount Deposits:**
   - Check if `increaseCollateral(loan, poolId, amount)` pushes `poolId` to `loan.colPools` when `loan.collaterals[poolId].balance == 0` *without* requiring `amount > 0` (`#33780`).

### Playbook B: ERC-4626 Vaults, Async Period Vaults, Ve-Tokenomics, Gauges & Bribes (`firelight`, `belong`, `alchemix`, `yeet`)
1. **ERC-4626 Edge States:**
   - **Post-Slash Zero Assets (`totalAssets() == 0 && totalSupply() > 0`):** If a vault can be slashed (`FirelightVault.payout()`) without burning shares, OpenZeppelin's virtual offset (`mulDiv(totalSupply + 1, totalAssets + 1)`) inverts — a `1 wei` deposit mints `totalSupply + 1` shares (>50% of the vault) (`#88330`).
   - **First-Depositor Donation (`totalAssets() > 0 && totalSupply() == 0`):** Check if `totalAssets()` uses `asset.balanceOf(address(this))` and whether `deposit()` reverts on `shares == 0` (`#56850`).
   - **Transfer Desync with Lock Ledgers:** If an ERC-4626 vault tracks deposit timestamps in `stakes[user]`, check if `ERC20.transfer()` transfers shares without updating `stakes[recipient]`, bricking `withdraw()`/`redeem()` (`#57635`).
2. **Async Period / Epoch Boundary Accounting (`FirelightVault`, `Anvil TBCP`):**
   - Check what happens when `withdraw()` is called at exact `block.timestamp == periodStart`: does `_requestWithdraw` bucket the withdrawal into `currentPeriod() + 1` while `_logTrace` updates `_traceTotalAssets` at `periodStart`, corrupting inclusive `totalAssetsAt(periodStart)` (`#88287`)?
   - Check overlapping payout windows: if a Period $P$ incident is paid out during Period $P+1$, does `CoverOrderAllocator` still read the historical `totalAssetsAt(periodPPlus1Start)` checkpoint, selling new cover against already-spent collateral (`#88279`)?
   - Check rescue/blocklist functions: does `rescueWithdrawFromBlocklisted` set a permanent `isWithdrawClaimed[period][from] = true` flag on an additive pending slot (`#88270`)?
3. **Ve-Tokenomics (`VotingEscrow`), `Voter`, `Bribe`, and `PerpetualGauge`:**
   - **Flash-Loan + ClearVote Asymmetric Weight:** Does `vote()` or `clearVote()` multiply weights by live `votingToken.balanceOf(msg.sender)` (`#56798`)?
   - **Missing Epoch Guard on `poke()`:** Does `Voter.poke(tokenId)` lack `onlyNewEpoch(tokenId)`, allowing infinite FLUX minting in a single block (`#30634`)?
   - **Collateral-Hopping Across `tokenId`s via `merge()`:** Can a user call `voter.reset(tokenIdA)`, `veALCX.merge(tokenIdA, tokenIdB)`, and `voter.reset(tokenIdB)` in the same epoch (`#30683`)?
   - **Binary Search with Duplicate Timestamps:** Does `totalSupplyAtT(t)` break on the first match `lastPoint.ts == t` while `balanceOfTokenAt` scans to the latest checkpoint at `t` (`#30655`)?
   - **Exact Epoch-End Timestamp Overwrite in `Bribe`:** If `Voter.distribute()` (`Bribe.resetVoting()`) and a tiny `vote()` execute at `block.timestamp == _lastEpochEnd`, does `Bribe.earned()` use that tiny vote as `_priorSupply` (`#30860`)?
   - **Unbounded Checkpoints vs. Capped Claim Loops:** Does `RewardsDistributor._claimable` cap its loop at `50` iterations of `userEpoch` so that spamming 50 checkpoints in one week prevents `weekCursor` from advancing (`#31078`)?

### Playbook C: Staking, Restaking, Liquid Staking & Validator Delegation (`plume`, `vechain-or-stargate-hayabusa`, `puffer-finance`, `lombard`)
1. **Validator Commission & Reward Per Token Checkpoints (`Plume`):**
   - When a validator is added via `addValidator()`, is an initial commission checkpoint pushed, or does user reward calculation fall back to mutable `validator.commission` (`#53037`)?
   - When a reward token is removed and later re-added, does `_initializeRewardStateForNewStake` initialize `userValidatorRewardPerTokenPaidTimestamp` for removed/historical tokens, or only currently active `$.rewardTokens` (`#49616`)?
   - Does `updateRewardPerTokenForValidator` advance `validatorLastUpdateTimes` when integer division floors `commissionDeltaForValidator` to `0` (`#49700`)?
2. **Delegation Exit & Validator Exit State Transitions (`VeChain Stargate`):**
   - Check the boundary between the last active period (`endPeriod`) and post-exit periods (`nextClaimablePeriod == endPeriod + 1`): does `endPeriod > nextClaimablePeriod` fail to match equality and fall through to the active-delegator branch, creating an infinite VTHO "ghost stake" drain (`#59316`)?
   - Check double-subtraction on validator exit: if a user already requested exit (subtracting their stake from future period checkpoints) and the validator then becomes `EXITED`, does `unstake()` subtract the stake a second time and panic with arithmetic underflow (`#59386`)?
3. **EigenLayer / Restaking 2-Step Withdrawal Slashing (`Puffer`):**
   - If a queued withdrawal is slashed on EigenLayer (`slashQueuedWithdrawal` sets `withdrawalRootPending = false`), does the liquid restaking vault's `claimWithdrawalFromEigenLayer` permanently revert and leave `eigenLayerPendingWithdrawalSharesAmount` inflated (`#28788`)?

### Playbook D: Bridges, Cross-Chain, FAssets, Rollup Proofs & DEX Wrappers (`flare-fassets`, `base`, `plume`, `lombard`)
1. **Rollup / ZK / TEE Proof Verification (`Base Azul`):**
   - Compare every header field and EVM environment rule between the canonical L2 node and the ZK/TEE proof executor (`BLOBBASEFEE` derived from repurposed `blob_gas_used` DA footprint in `#75413`).
   - Check fork-activation guards in the derivation pipeline (`is_isthmus_active` called with `block.number` instead of `block.timestamp` in `#75423`, combined with unverified `intermediate_root_interval`).
2. **CCIP / Cross-Chain Token Pools (`Lombard`):**
   - In `releaseOrMint`, verify that `offchainTokenData` (which is unauthenticated in manual CCIP execution) is explicitly validated against `releaseOrMintIn.sourcePoolData` (`extraData`) and `releaseOrMintIn.amount` (`#38154`).
3. **FAssets & Agent Collateral Pools (`Flare FAssets`):**
   - Check partial redemption paths (`_selfCloseExitTo` with `_redeemToCollateral = true`): does `payoutFromVault` pay `Math.min(required, balance)` while burning 100% of the user's CPT tokens (`#47108`)?
   - Check external reward claims (`claimAirdropDistribution`, `claimDelegationRewards`): can an agent pass a malicious contract address and inflate `totalCollateral` via a fake return value (`#45893`)?
   - Check BIPS discount/topup parameters (`setTopupTokenPriceFactorBIPS`): can setting a denominator parameter to `1` BIPS mint $10,000\times$ pool shares (`#45979`)?
4. **DEX Aggregator Wrappers (`Plume`):**
   - Never allow user-supplied `teller` / `router` addresses to receive `safeApprove` without whitelist validation (`#49863`), always enforce `dstReceiver == address(this)` via balance-delta checks, and refund unspent `srcToken` after partial fills (`#49854`).

### Playbook E: NFTs, Marketplaces, Royalties, Access Control & Migrations (`belong`, `zerolend`, `ens`, `plume`)
1. **Fee Routing & Revenue Stranding (`BelongCheckIn`):**
   - Trace every fee subtraction (`subsidyMinusFees = platformSubsidy - processingFee`). If the contract only pulls `subsidyMinusFees` from escrow and forgets to call `_handleRevenue(processingFee)`, the fee remains in the venue's escrow balance and is lost to the platform (`#57039`).
2. **Signature Verification (`SignatureVerifier`):**
   - Replace `abi.encodePacked` over dynamic strings (`name`, `symbol`, `contractURI`) with `abi.encode` or EIP-712 structs, and include a per-signer `nonce` (`#57314`).
3. **AccessControl & Upgradeability (`ArcTokenFactory`, `VestedZeroNFT`):**
   - Never grant `DEFAULT_ADMIN_ROLE` on a child proxy to a user if `UPGRADER_ROLE` has `DEFAULT_ADMIN_ROLE` as its admin (`#50713`).
   - Remember that OpenZeppelin v5 `_requireOwned(tokenId)` only checks that the NFT exists, NOT that `msg.sender` owns it (`#29031`).

---

## 3. Foundry PoC Engineering Patterns Used in Winning Critical/High Reports

Winning Immunefi reports use 6 high-signal Foundry PoC engineering patterns to make impact indisputable:

1. **Snapshot / Revert A/B Baseline Comparison (`vm.snapshot()` / `vm.revertTo()` or Positive vs. Negative Control):**
   - **Used in:** `#56555` (`Transmuter` bad debt scaling) and `#88287` (`FirelightVault` boundary checkpoint).
   - **Pattern:** Before executing the exploit, take a state snapshot (`uint256 snap = vm.snapshotState();`). First execute the *honest user flow* (e.g., `claimRedemption` without flash deposit, or `withdraw` at `periodStart + 1`) and record `honestPayout`. Then `vm.revertToState(snap);` and execute the *exploit flow* (sandwiching `claimRedemption` with `deposit`/`withdraw`, or calling `withdraw` at exact `periodStart`). Log and assert `assertGt(attackerPayout, honestPayout)` and `assertEq(vaultToken.balanceOf(vault), 0)` (insolvency).
2. **Self-Contained Flash-Borrower Contract (`IERC3156FlashBorrower`) for Same-Block Governance/Gauge Attacks:**
   - **Used in:** `#56798` (`PerpetualGauge` flash-vote exploit).
   - **Pattern:** Write a minimal helper contract implementing `onFlashLoan(...)` in the test file. Inside `testFlashVoteExploit()`, call `veALCX.flashLoan(borrower, address(veALCX), maxFlashLoan, "")`. Inside `onFlashLoan`, call `gauge.vote(...)` and approve repayment; after `flashLoan` returns (when attacker balance is back to `0`), call `gauge.clearVote()` and assert `aggStrategyWeight` retains the flash-borrowed weight permanently.
3. **Targeted `vm.mockCall` to Isolate Core State-Machine Bugs from Complex Periphery Dependencies:**
   - **Used in:** `#57053` (`AlchemistV3` precision loss) and `#47115` (`Term Structure` unsettled liquidation).
   - **Pattern:** When testing a core accounting flaw in `AlchemistV3` or `Settlement`, use `vm.mockCall(address(transmuter), abi.encodeWithSelector(ITransmuter.queryGraph.selector), abi.encode(earmarkAmount))` or mock `SignatureChecker.isValidSignatureNow` only when periphery setup is orthogonal, OR deploy the real production contracts via `ERC1967Proxy` (`#88270`, `#88287`) when full end-to-end integration is required by competition rules.
4. **Multi-Block / Multi-Epoch Time-Warp Choreography (`vm.roll` + `vm.warp`):**
   - **Used in:** `#30655` (duplicate-timestamp binary search), `#30860` (`Bribe` exact `_lastEpochEnd`), `#49616` (`Plume` historical reward token), and `#36554` (`Anvil` post-reset drain).
   - **Pattern:** Always advance both `vm.roll(block.number + N)` and `vm.warp(block.timestamp + T)` together when protocols mix block-based (`queryGraph`, `lastEarmarkBlock`) and timestamp-based (`VotingEscrow`, `Bribe`, `FirelightVault`) checkpoints. Pin exact boundary timestamps (`vm.warp(nextEpochStart)`) to prove `<` vs `<=` bugs.
5. **Loop Amplification of Sub-Unit Truncation (`conversionFactor - 1`):**
   - **Used in:** `#57053` (`normalizeDebtTokensToUnderlying` rounding to `0`).
   - **Pattern:** Compute `uint256 maxDust = alchemist.underlyingConversionFactor() - 1;` (`1e12 - 1` wei of 18-decimal debt = `$0.000001`). Execute `alchemist.burn(maxDust, victimId)` in a loop (or show single-iteration `debtBefore - debtAfter == 1e12 - 1` while `rawLockedBefore == rawLockedAfter`), then trigger `redeem()` to prove that the victim's collateral is seized despite the debt already being burned.
6. **End-to-End Insolvency Proof (Proving the Last Honest User Reverts):**
   - **Used in:** `#56365`, `#41215`, `#36554`, `#88270`.
   - **Pattern:** Never stop at showing a corrupted storage variable (`totalDebt` or `pendingWithdrawAssets`). Always include a second honest user (`bob` / `users[2]`) who deposited before or after the attack, and end the PoC with `vm.prank(bob); vm.expectRevert(); vault.withdraw(...)` (or `claimWithdraw` / `releaseEligibleTokens`), proving on-chain that the protocol cannot honor its obligations to honest users.
