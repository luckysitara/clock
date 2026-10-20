# Non-EVM Smart Contract Vulnerability Synthesis (Immunefi Corpus)

This playbook synthesizes **139 Medium, High, and Critical (M/H/C)** smart contract vulnerability reports across **7 non-EVM ecosystems** from the Immunefi corpus. It is structured to train an autonomous auditing agent on VM-specific execution models, asset representations, arithmetic traps, and cross-VM mental model failures.

---

## Section 1: Ecosystem-by-Ecosystem Deep Dive

---

### 1. Move (Aptos / Movement Labs — `movement-labs-attackathon`)

#### A. Execution Model & Asset Model Pitfalls
1. **Transaction Prologue/Epilogue & Feature-Flagged Gas/Storage Accounting (`transaction_validation.move`)**:
   - Aptos/Movement separates gas execution burns from storage slot allocation fees (`storage_fee_charged` vs `storage_fee_refunded`). When a transaction deletes storage slots, `storage_fee_refunded` can exceed `amount_to_burn`.
   - Protocol forks (such as Movement Labs adding `governed_gas_pool`) frequently branch inside `epilogue_gas_payer` via `features::governed_gas_pool_enabled()`. Missing a corresponding branch when `amount_to_burn < storage_fee_refunded` silently drops storage refunds or breaks gas accounting.
2. **Dual Asset Standards (`CoinStore<T>` vs `FungibleStore`)**:
   - Aptos/Movement is transitioning from legacy `0x1::coin::CoinStore<T>` to `0x1::fungible_asset::FungibleStore` (toggled via `features::operations_default_to_fa_apt_store_enabled()`).
   - **Auto-Creation Asymmetry**: `fungible_asset::deposit_internal(store_addr, fa)` strictly asserts `assert!(exists<FungibleStore>(store_addr), error::not_found(EFUNGIBLE_STORE_EXISTENCE))`. Unlike `primary_fungible_store::deposit`, low-level `deposit_internal` does **not** lazily create the `FungibleStore` resource if it has not been initialized via `create_store`.
   - **Frozen Store Bypass in Epilogue**: Standard `coin::withdraw_from` asserts `!coin_store.frozen`. However, system gas collection (`transaction_fee::burn_fee` / `collect_into_aggregatable_coin`) uses special internal primitives that bypass `frozen` checks so frozen accounts can still pay gas and recover. Routing gas collection through standard `coin::withdraw_from` breaks this invariant.
3. **Block Prologue & `AggregatableCoin` State Lifecycle**:
   - In `block.move`, `block_prologue` runs at the start of Block $N$: it first calls `transaction_fee::process_collected_fees()` (distributing Block $N-1$'s fees and resetting `collected_fees.proposer = option::none()`) and then calls `register_proposer_for_fee_collection(proposer)` for Block $N$.
   - Any governance or system function (e.g., `upgrade_burn_percentage`) that calls `process_collected_fees()` **mid-block** when `collected_fees.amount` is currently zero will hit the early-exit branch inside `process_collected_fees()` that does `let _ = option::extract(&mut collected_fees.proposer)`, wiping out the current block's registered proposer!
4. **Move VM `TypeLayout` Loader Cache vs Recursion Depth Limits**:
   - The Move VM runtime (`third_party/move/move-vm/runtime/src/loader/mod.rs`) enforces `DEPTH_MAX = 128` during recursive struct layout construction (`type_to_type_layout_impl`). However, `struct_name_to_type_layout` caches intermediate struct layouts in a shared cache without storing their internal depth—and when hit on a subsequent call, returns the cached `TypeLayout` **without running `check_depth!(depth)`**.

#### B. Step-by-Step Auditing Checklist & Code Smells (Move)
- [ ] **Check every `features::*_enabled()` branch in `aptos-framework`**: In `transaction_validation.move`, `transaction_fee.move`, and custom framework modules, trace both `true` and `false` branches for every feature flag (`governed_gas_pool_enabled`, `operations_default_to_fa_apt_store_enabled`, `collect_and_distribute_gas_fees`). Verify that edge cases (`amount_to_burn == 0`, `storage_fee_refunded > amount_to_burn`) are handled symmetrically in both branches.
- [ ] **Verify resource initialization across `CoinStore` and `FungibleStore` migrations**: Whenever a module routes deposits/withdrawals based on `operations_default_to_fa_apt_store_enabled()`, check whether `create_store` / `ensure_primary_store_exists` was called during genesis (`initialize_for_verification` / `initialize`) for system addresses.
- [ ] **Check `coin_store.frozen` reachability from `epilogue`**: Never call `coin::withdraw` or `coin::withdraw_from` inside transaction prologue/epilogue gas-charging paths; gas collection must use internal extraction that ignores `frozen`.
- [ ] **Audit callers of `process_collected_fees()` outside `block_prologue`**: Search for all calls to `process_collected_fees()`. If called during transaction execution, verify it preserves `collected_fees.proposer`.
- [ ] **Audit VM Loader / Verifier Caches for Unchecked Metadata**: In custom Move VM natives or loader caches (`TypeLayout`, `TypeTag`, `Function` caches), verify that every limit enforced during cold construction (recursion depth, node count, type parameter count) is also stored and re-verified on a warm cache hit.

#### C. Concrete Cited Examples
- **Report `#43135` & `#42513`** (High — `movement-labs-attackathon`): *No storage fee refund when governed gas pool is enabled*.
  - **Mechanism**: In `aptos-move/framework/aptos-framework/sources/transaction_validation.move::epilogue_gas_payer`, when `amount_to_burn < storage_fee_refunded`, `let mint_amount = storage_fee_refunded - amount_to_burn;`. The code executes `if (!features::governed_gas_pool_enabled()) { transaction_fee::storage_refund(gas_payer, mint_amount); }` with **no `else` branch**. Because Movement enables `governed_gas_pool` by default, 100% of net storage refunds are silently lost.
  - **PoC Method**: Move unit test (`#[test] fun test_epilogue_storage_refund_with_governed_gas_pool()`) enabling `GOVERNED_GAS_POOL` feature flag, funding a `gas_payer`, calling `epilogue_gas_payer` with `storage_fee_refunded = 5000` and `execution_gas_used = 2000`, and asserting the user's balance failed to increase by `3000`.
- **Report `#42480`** (Medium — `movement-labs-attackathon`): *`governed_gas_pool::deposit_gas_fee_v2` aborts all transactions when `FA_APT_STORE` is enabled*.
  - **Mechanism**: `deposit_gas_fee_v2` branches on `features::operations_default_to_fa_apt_store_enabled()` and calls `deposit_from_fungible_store(gas_payer, amount)`, which calls `fungible_asset::deposit_internal(governed_gas_pool_store_address, fa)`. However, `governed_gas_pool::initialize` only created `CoinStore<AptosCoin>` and never created a `FungibleStore` at `governed_gas_pool_store_address`.
  - **PoC Method**: Move test calling `deposit_gas_fee_v2` with `OPERATIONS_DEFAULT_TO_FA_APT_STORE` enabled; aborts with `EFUNGIBLE_STORE_EXISTENCE` (`0x60004`).
- **Report `#42928`** (Medium — `movement-labs-attackathon`): *`governed_gas_pool::deposit_from` breaks frozen `CoinStore` gas payment invariant*.
  - **Mechanism**: `governed_gas_pool::deposit_from` uses `coin::withdraw_from<AptosCoin>(account, amount)` in the transaction epilogue instead of `transaction_fee::burn_fee`. Because `coin::withdraw_from` asserts `!coin_store.frozen`, any account with a frozen `CoinStore` fails in the epilogue.
- **Report `#43132`** (Medium — `movement-labs-attackathon`): *`upgrade_burn_percentage` resets block proposer to `None` when called before fees are collected*.
  - **Mechanism**: `transaction_fee::upgrade_burn_percentage` calls `process_collected_fees()`. If executed at the start of a block when `is_aggregatable_coin_zero(&collected_fees.amount)` is `true`, `process_collected_fees` executes `let _ = option::extract(&mut collected_fees.proposer);`, clearing the proposer set by `block_prologue` and causing all subsequent transaction fees in that block to be burned instead of paid to the validator.
- **Report `#43333`** (Critical — `movement-labs-attackathon`): *Move VM `TypeLayout` cache skips max depth check, causing deterministic consensus split*.
  - **Mechanism**: In `third_party/move/move-vm/runtime/src/loader/mod.rs::struct_name_to_type_layout`, on a cache hit (`if let Some(layout) = self.type_cache.read().struct_layouts.get(&key)`), the function immediately returns `Ok(layout.clone())` without checking if `depth + layout_depth > DEPTH_MAX (128)`.
  - **Attack Path & PoC**: Deploy a module with nested structs `V1 { f: u64 }`, `V2 { f: V1 }`, ..., `V128 { f: V127 }`. Calling a function with `V128` on a cold cache fails with `VM_MAX_VALUE_DEPTH_REACHED`. Calling a function with `V3` first caches `V3`'s layout at depth 1; calling the function with `V128` on the same node then hits the cache at `V3` (depth 126) and succeeds! Because Block-STM worker threads maintain thread/node-dependent cache warmth, honest validators diverge on transaction validity and halt the chain.

---

### 2. Fuel / Sway (`fuel-network-or-attackathon`, `swaylend_iop`, `thundernft-or-iop`, `fluid-protocol`)

#### A. Execution Model & Asset Model Pitfalls
1. **UTXO Native Multi-Asset Call Frames (`msg_asset_id()`, `msg_amount()`, `transfer()`)**:
   - In FuelVM, every asset is a native first-class asset (there is no ERC20 `transferFrom` pull pattern for native assets). When a user calls a `#[payable]` contract function, they attach a single `(msg_asset_id(), msg_amount())` pair to the call frame.
   - **Immediate Credit Trap**: `msg_amount()` is **automatically added to the contract's native balance before the first line of the function executes**. If a contract validates `require(msg_amount() >= required_amount)` instead of `==` (or fails to `transfer(sender, msg_asset_id(), msg_amount() - required_amount)`), the excess tokens are permanently locked in the contract!
   - **Zero-Amount `transfer()` Panic**: Calling `std::asset::transfer(to, asset_id, 0)` executes the FuelVM `TR` or `TRO` instruction with `amount == 0`, which **panics at the VM level (`Revert(0)`)**! Unlike Solidity ERC20 where `transfer(to, 0)` is a no-op, any unguarded fee transfer (`transfer(fee_recipient, asset, protocol_fee)`) when `protocol_fee == 0` bricks the entire transaction.
   - **Custody vs Call-Attachment Confusion**: Once a user deposits a native NFT or token into a contract (e.g., `place_order` on a sell order), the contract owns the UTXO balance. Requiring `msg_asset_id() == order.nft_asset_id` when the seller later accepts a bid (`_execute_sell_taker_order`) makes execution impossible because the seller no longer holds the NFT in their wallet.
2. **64-Bit Register Architecture vs Sub-64-Bit Types (`u8`, `u16`, `u32`) & Emulated Signed Math (`I8..I256`, `IFP64..IFP256`)**:
   - FuelVM has 64-bit registers (`u64`). Smaller types (`u8`, `u16`, `u32`) live inside 64-bit registers in memory/registers. Hardware ALU overflow (`$of`) only triggers when an operation overflows 64 bits (`> u64::MAX`). Thus, `u8`/`u16`/`u32` arithmetic in Sway `std` must explicitly insert `__gt(res, Self::max())` checks after ASM instructions (`add`, `mul`, `exp`). Missing that check in `u8::pow` / `u16::pow` / `u32::pow` lets values silently overflow into the upper 56/48/32 bits of the register!
   - FuelVM has **no native signed integer instructions**. `sway-libs` implements signed integers (`I8`..`I256`) using an offset bias (`indent = 2^(bits - 1)`, representing `0`) or signed fixed-point (`IFP64`..`IFP256`) using an unsigned `Underlying` + `non_negative: bool`. Manual sign-bit state machines in `sway-libs` suffer from catastrophic sign-inversion, `wrapping_neg()`, underflow-near-indent, and `floor()`/`ceil()` bugs.
3. **TAI64 Timestamps vs Unix Timestamps**:
   - `std::block::timestamp()` in Sway returns a **TAI64** timestamp (`(1 << 62) + unix_timestamp + 10 = ~4.611686 × 10^18`), **not** a Unix epoch timestamp (`~1.7 × 10^9`). Comparing `std::block::timestamp()` directly against an external oracle's Unix timestamp (such as Redstone) without converting via `from_tai64()` / subtracting `4611686018427387914` breaks all timestamp comparisons by $4.61 \times 10^{18}$ seconds.
4. **Sway Storage Slot Hashing & Collisions**:
   - Sway computes the storage slot of a top-level variable `<var>` in `<namespace>` as `sha256("storage.<namespace>.<var>")`. `StorageMap<K, V>` computes item slots as `sha256(key, field_id)`. Custom `in 0x...` annotations or non-domain-separated hashes in `StorageVec`/`StorageBytes` allow cross-variable slot collisions.
5. **Manual Storage Reentrancy Locks & Early Returns**:
   - When Sway contracts use a storage boolean (`storage.lock_internal_...write(true)`) across multi-contract calls instead of a call-frame guard, **an early `return` does not revert state**. If a function returns early on an edge case (e.g., `if new_debt < MIN_NET_DEBT { return ...; }`) before `storage.lock.write(false)`, the lock stays `true` in storage forever, permanently bricking the protocol.
6. **No Native Deploy-Time-Only `constructor`**:
   - In Sway, `fn constructor(...)` is an ordinary callable ABI entrypoint. Unless guarded by an initialized storage flag **and** a `configurable` deployer address check, anyone can front-run or call `constructor` post-deployment.

#### B. Step-by-Step Auditing Checklist & Code Smells (Fuel / Sway)
- [ ] **Grep for `msg_amount()` and `msg_asset_id()`**:
  - Does any function check `msg_amount() >= X` without refunding `msg_amount() - X`?
  - If a function calls an external contract that charges a dynamic fee (e.g., `pyth.update_price_feeds`), does it query `pyth.update_fee(update_data)` instead of trusting a user-supplied `fee` struct field, and does it refund `msg_amount() - fee`?
- [ ] **Grep for every `transfer(` and `mint_to(` call**:
  - Can the `amount` parameter ever be `0` (e.g., `protocol_fee == 0`, `royalty_fee == 0`, `reward == 0`)? If it is not wrapped in `if amount > 0 { transfer(...); }`, it is an immediate DoS bug.
- [ ] **Inspect `std::block::timestamp()` vs Oracle Timestamps**:
  - Check whether oracle payloads (Pyth, Redstone, Stork) return Unix seconds/milliseconds or TAI64. Ensure `TAI64` offset (`(1 << 62) + 10`) is applied consistently before subtraction.
- [ ] **Trace all `I256` / `I128` / `IFP*` signed math operations**:
  - Check how negative balances/principals are converted before helper calls. Watch for `wrapping_neg()` flipping a negative debt into a positive supply balance before checking `if principal >= I256::zero()`!
  - Check custom `sway-libs` signed integer subtraction when `self < other` and fixed-point `mul`/`div`/`floor`/`ceil` when operands are negative.
- [ ] **Trace all storage-based locks (`storage.*lock*.write(true)`)**:
  - Follow every control-flow path between `.write(true)` and `.write(false)`. Any `return` statement before `.write(false)` is a permanent protocol deadlock (Critical DoS).
- [ ] **Check ABI supertraits & `constructor` access control**:
  - Check `abi MyContract : SuperAbi` declarations—every method implemented in `impl SuperAbi for Contract` is externally callable at runtime even if omitted from `contract-abi.json`!

#### C. Concrete Cited Examples
- **Report `#37323`, `#37452`, `#37624`, `#37671`** (Critical — `fluid-protocol`): *Permanent protocol deadlock via unreset reentrancy lock on early return in `internal_redeem_collateral_from_trove`*.
  - **Mechanism**: In `contracts/trove-manager-contract/src/main.sw`, `internal_redeem_collateral_from_trove` sets `storage.lock_internal_redeem_collateral_from_trove.write(true);`. Later, if a partial redemption would leave a trove with `new_debt < MIN_NET_DEBT`, it executes:
    ```sway
    if (new_debt < MIN_NET_DEBT) {
        single_redemption_values.cancelled_partial = true;
        return single_redemption_values; // <-- lock_internal_redeem_collateral_from_trove remains true!
    }
    ```
  - **Attack Path & PoC**: An attacker opens a trove with `2,000` USDF debt (`MIN_NET_DEBT = 500`) and calls `redeem_collateral` for `1,600` USDF (`new_debt = 400 < 500`). The function returns early without reverting, leaving `lock_internal_redeem_collateral_from_trove == true` in storage permanently and bricking all future redemptions across the protocol. Verified via Rust SDK test (`cargo test --package trove_manager_contract test_dead_lock`).
- **Report `#35758`** (Critical — `swaylend_iop`): *Borrow principal interest accrued at Supply Rate instead of Borrow Rate in `is_liquidatable_internal`*.
  - **Mechanism**: In `contracts/market/src/main.sw`, borrower principals are stored as negative `I256` values (`principal < I256::zero()`). Inside `is_liquidatable_internal`, when `principal < I256::zero()`, the code calls `present_value(principal.wrapping_neg())`. Because `wrapping_neg()` turns the negative borrow principal into a **positive** `I256`, `present_value(principal)` takes the `if principal >= I256::zero()` branch and multiplies by `base_supply_index` (the lower supply interest rate) instead of `base_borrow_index`!
  - **Attack Path & PoC**: Underwater borrowers accrue debt at the lower supply rate during liquidation checks (`is_liquidatable_internal`), preventing liquidation of insolvent positions while `absorb_internal` (which calls `present_value(principal)` without `wrapping_neg()`) uses the higher borrow index, creating unbacked bad debt.
- **Report `#34966`** (High — `thundernft-or-iop`): *Zero-fee `transfer()` in `_transfer_fees_and_funds` reverts all trades when protocol or royalty fee is 0*.
  - **Mechanism**: In `contracts/thunder_exchange/src/main.sw`, `_transfer_fees_and_funds` unconditionally calls `transfer(fee_recipient, asset, protocol_fee)` and `transfer(royalty_recipient, asset, royalty_fee)`. When `protocol_fee == 0` or `royalty_fee == 0` (either because fee rate is set to 0 or integer division `price * rate / 10000` truncates to 0 for small prices), FuelVM's `TR` instruction panics on zero amount.
- **Report `#34560`, `#34605`, `#34980`** (Critical — `thundernft-or-iop`): *Arbitrary NFT theft via `update_order` + `cancel_order` state manipulation*.
  - **Mechanism**: In `ThunderExchange`, `place_order` for `Side::Sell` locks the maker's NFT (`msg_amount()`) in the exchange. However, `update_order(order)` has an empty `Side::Sell => {}` branch that updates storage `sell_order.put(..., order)` without validating `msg_amount()` or preventing a user from updating `order.amount` on a `Side::Sell` order (or updating a `Side::Buy` order into `Side::Sell`). The attacker places a sell order for 1 ERC1155 token, calls `update_order` to set `amount = 100`, and calls `cancel_order` to withdraw 100 ERC1155 tokens belonging to other users.
- **Report `#37276`** (Medium — `fluid-protocol`): *TAI64 vs Unix timestamp mismatch in `oracle-contract`*.
  - **Mechanism**: `oracle-contract/src/main.sw` compares Redstone's Unix timestamp (`redstone.read_timestamp(...)`) against Fuel's `std::block::timestamp()` (TAI64) without subtracting the $2^{62} + 10$ TAI64 offset.
- **Report `#35793` & `#35876`** (High — `swaylend_iop`): *Excess native asset permanently locked via `msg_amount() >= amount`*.
  - **Mechanism**: `src-20::burn` checks `require(msg_amount() >= amount)` and burns `amount`, leaving `msg_amount() - amount` locked in the `src-20` contract forever while still counted in `total_supply`. Similarly, `Market::update_price_feeds` accepts payable native ETH for Pyth fees and never refunds `msg_amount() - update_fee`.
- **Report `#33351`** (Critical — `fuel-network-or-attackathon`): *Sway compiler exposes ABI supertrait methods as public contract entrypoints*.
  - **Mechanism**: In `sway-core/src/asm_generation/fuel/fuel_asm_builder.rs::generate_contract_entry`, the compiler generates public function selector dispatch branches for both the contract's `abi` methods **and** all `interface_surface` supertrait methods, even though the Sway specification states supertrait methods are internal-only and omits them from `contract-abi.json`. A raw call with the selector of an unguarded supertrait method executes directly on-chain.
- **Report `#33331` & `#33227`** (High — `fuel-network-or-attackathon`): *Silent register overflow in `u8`, `u16`, `u32` `pow()`*.
  - **Mechanism**: In `sway-lib-std/src/primitive_conversions.sw`, `u8::pow`, `u16::pow`, and `u32::pow` execute `asm(r1: self, r2: exp, r3) { exp r3 r1 r2; r3: Self }` without checking `if __gt(res, Self::max()) { revert(0); }`. Because FuelVM registers are 64-bit, `255_u8.pow(2)` produces `65025` inside an `u8` variable without overflowing the 64-bit `exp` opcode, corrupting subsequent comparisons and serialization.

---

### 3. Solana / SVM / Rust (`jito-restaking`)

#### A. Execution Model & Asset Model Pitfalls
1. **Direct SPL Token Account Transfers (`token_account.amount`) vs Internal Accounting (`vault.tokens_deposited`)**:
   - On Solana, every SPL token balance is stored in an external Associated Token Account (`TokenAccount`) owned by the SPL Token program. **Any user can permissionlessly transfer SPL tokens directly into a vault's `TokenAccount` via `spl_token::instruction::transfer` without invoking the Vault program.**
   - If a vault program syncs external rewards via a crank instruction (e.g., `UpdateVaultBalance`) that computes `rewards = vault_token_account.amount - vault.tokens_deposited` and **instantaneously** increases `vault.tokens_deposited` (which determines the VRT share exchange rate), two severe vulnerabilities arise:
     1. **Pre-Crank Deposit Front-Running**: Between the moment an external reward distributor transfers SPL tokens into `vault_token_account` and the moment `UpdateVaultBalance` is cranked, `vault.tokens_deposited` is still at the old value. If `MintTo` (deposit) does not auto-sync or vest rewards over time, an attacker can deposit a huge flash-loaned stake right after the SPL transfer, crank `UpdateVaultBalance`, and immediately queue a withdrawal—capturing a massive fraction of the epoch's rewards for 0 seconds of staking!
     2. **Discrete Epoch Boundary Sandwiching**: Even if `UpdateVaultBalance` can only be called once per epoch, jumping the exchange rate discretely without a time-weighted vesting period allows deposits just before epoch $E$ ends to capture full epoch rewards.
2. **Self-Referential Fee Wallets in Share-Burning Flows**:
   - When a Solana vault collects management/reward/withdrawal fees by minting VRT shares to `vault_fee_wallet` and `program_fee_wallet`, those fee wallets must eventually burn their VRT shares via the same `EnqueueWithdrawal` -> `BurnWithdrawalTicket` pipeline as normal users.
   - If `BurnWithdrawalTicket` does not exempt `vault_fee_wallet` and `program_fee_wallet` from `withdrawal_fee_bps` and `program_fee_bps`, burning fee shares recursively mints *new* fee shares back to the fee wallets on every withdrawal!

#### B. Step-by-Step Auditing Checklist & Code Smells (Solana / SVM)
- [ ] **Check how Vault/Pool programs reconcile `token_account.amount` with internal state**:
  - Search for `token_account.amount.saturating_sub(vault.tokens_deposited)` or direct reads of `ctx.accounts.token_account.amount`.
  - Ask: *What happens if SPL tokens are transferred directly to `token_account` and a user calls `deposit` / `mint` BEFORE the reward-sync crank is called?*
  - Ask: *Do rewards vest linearly over slots/epochs, or does a single crank instruction jump the share price instantaneously?*
- [ ] **Check fee-exemption logic when fee recipients redeem shares**:
  - When fees are paid in LP/VRT shares rather than underlying tokens, trace how the fee recipient (`vault_fee_wallet`, `program_fee_wallet`, `treasury`) redeems those shares. Verify that redeeming fee shares does not re-trigger protocol/vault withdrawal fees.
- [ ] **Verify account reloads (`Account::reload()`) after CPIs**:
  - If a program CPIs into `spl_token::transfer` or `mint_to` and subsequently reads deserialized account structs, confirm it reloads or updates internal state consistently.

#### C. Concrete Cited Examples
- **Report `#36903`, `#37295`, `#37311`, `#37315`** (High — `jito-restaking`): *Immediate reward accrual without vesting / missing `update_vault_balance` before `mint_to` enables reward theft*.
  - **Mechanism**: In `vault_program/src/update_vault_balance.rs::process_update_vault_balance`, the vault calculates `let st_rewards = new_st_balance.saturating_sub(vault.tokens_deposited())?;`, takes `vrt_reward_fee`, and sets `vault.set_tokens_deposited(new_st_balance)`. Meanwhile, `vault_program/src/mint_to.rs::process_mint_to` mints VRT based on the current `vault.tokens_deposited()` without syncing `vault_st_token_account.amount` or enforcing a reward vesting schedule.
  - **Attack Path & PoC**:
    1. Honest stakers deposit `100` ST tokens (`100` VRT supply).
    2. Operator transfers `50` ST reward tokens into `vault_st_token_account` (`new_st_balance = 150`, `vault.tokens_deposited = 100`).
    3. Attacker calls `mint_to` with `900` ST tokens *before* `update_vault_balance` runs. Because `vault.tokens_deposited` is still `100`, attacker receives `900` VRT (90% of total VRT supply!).
    4. `update_vault_balance` runs: `st_rewards = 1050 - 1000 = 50`. The `50` ST rewards are distributed across all `1,000` VRT—meaning the attacker steals **90% (`45` ST)** of the honest stakers' rewards!
    5. PoC written using `jito-restaking` `TestBuilder` (`cargo nextest run`).
- **Report `#37314`** (High — `jito-restaking`): *Recursive fee charging on `vault_fee_wallet` and `program_fee_wallet` during `burn_withdrawal_ticket`*.
  - **Mechanism**: In `vault_program/src/burn_withdrawal_ticket.rs`, when burning a withdrawal ticket, the program unconditionally splits `vrt_Burn_amount` into `vault_fee_amount`, `program_fee_amount`, and `staker_amount`, and mints `vault_fee_amount + program_fee_amount` VRT back to `vault_fee_wallet` and `program_fee_wallet`—even when `staker` **is** `vault_fee_wallet` or `program_fee_wallet`!
  - **Attack Path & PoC**: With `withdrawal_fee_bps = 2000` (20%) and `program_fee_bps = 500` (5%), every time `vault_fee_wallet` withdraws its accumulated VRT fees, 20% stays locked as new VRT in `vault_fee_wallet` and 5% is taxed to `program_fee_wallet`. Compounding this series $\sum_{n=0}^{\infty} 0.05 \times 0.20^n = 6.25\%$, `program_fee_wallet` steals 25% more fees than configured, and `vault_fee_wallet` requires 42 epochs (84 days) of recursive withdrawals to extract its fees.

---

### 4. Starknet / Cairo (`iop-paradex`)

#### A. Execution Model & Asset Model Pitfalls
1. **`felt252` Prime Field Arithmetic ($\mathbb{F}_P$) vs Signed (`i128`) & Unsigned (`u256`) Integers**:
   - Cairo's native word is `felt252`, an element of the prime field $\mathbb{F}_P$ where $P = 2^{251} + 17 \cdot 2^{192} + 1 \approx 3.6185 \times 10^{75}$. **There are no negative numbers in $\mathbb{F}_P$**: every `felt252` is canonically an integer in $[0, P - 1]$.
   - **Trap 1: `i128` $\rightarrow$ `felt252` $\rightarrow$ `u256` Cast Chain (Negative-to-Astronomical-Positive)**:
     - In Cairo's core library, `ImplI128IntoFelt252::into(self: i128) -> felt252` maps negative values $-x$ to their field additive inverse $P - x \pmod P$.
     - For example, `(-42_i128).into()` produces `felt252` value $P - 42 = 3618502788666131213697322783095070105623107215331596699973092056135872020439$!
     - If a view function or helper returns `felt252` (by calling `.into()` on a signed `i128` PnL or account value) and the caller converts that `felt252` into `u256` via `.into()`, a `-42` negative account value becomes a **$+3.618 \times 10^{75}$ `u256` balance**!
   - **Trap 2: Accepting `felt252` for an Unsigned Amount and Unwrapping to `i128`**:
     - Conversely, `Felt252TryIntoI128::try_into(self: felt252) -> Option<i128>` maps field elements in $[P - 2^{127}, P - 1]$ to **negative `i128` values** $[-2^{127}, -1]$!
     - If an external entrypoint accepts `amount: felt252` and converts it to `i128` via `let amount_i128: i128 = amount.try_into().unwrap();` without asserting `amount_i128 >= 0`, an attacker can pass $P - 1000$ as `amount: felt252`, which unwraps to `amount_i128 = -1000_i128`! Subtracting `amount_i128` from the sender's balance (`sender_balance - (-1000)`) **mints** `1000` collateral to the sender out of thin air!
2. **Decimal Scaling Truncation Before `ERC20::transferFrom`**:
   - Paradex stores internal `Paraclear` token balances at $10^8$ precision (`internal_decimals = 8`), while external tokens like USDC have `decimals = 6` (`scale_factor = 10^(8 - 6) = 100`).
   - If `_deposit(amount: felt252)` computes `amount_scaled: u256 = amount.into() / 100` (integer division), executes `erc20.transferFrom(caller, this, amount_scaled)`, and then credits the **unscaled** `amount` to internal storage, passing `amount = 99` transfers `99 / 100 = 0` USDC from the ERC20 contract while crediting `99` internal units to the attacker!
3. **Fail-Open Uninitialized `ContractAddress` (`is_non_zero()`)**:
   - In Cairo, an uninitialized storage `ContractAddress` defaults to `0`. Wrapping compliance/authorization checks inside `if transfer_registry_address.is_non_zero() { ... }` causes the contract to **fail open** when the registry address is `0` (not yet set or cleared), turning restricted cross-account transfers into permissionless steals.
4. **Role Graph Cycles Across Accounts (`operator` vs `sub_operator`)**:
   - When a protocol uses custom account contracts (ISRC6) with `operator` and `sub_operator` delegation mappings, checking only `other_vault_operator.is_zero()` for a `sub_operator` candidate without checking whether that candidate is already a primary `operator` of another vault allows creating a 2-hop delegation cycle (`Vault1 -> Operator1 (as SubOperator of Vault2) <- Vault2`), enabling cross-vault collateral draining.

#### B. Step-by-Step Auditing Checklist & Code Smells (Starknet / Cairo)
- [ ] **Audit every `.into()` and `.try_into()` involving `felt252` and signed integers (`i8`, `i16`, `i32`, `i64`, `i128`)**:
  - **Code Smell 1**: `let x_felt: felt252 = signed_val.into();` followed anywhere downstream by `let x_u256: u256 = x_felt.into();` or `u128`. If `signed_val < 0`, `x_u256` will be $\approx P \approx 3.6 \times 10^{75}$!
  - **Code Smell 2**: `fn foo(amount: felt252)` $\rightarrow$ `let amount_i128: i128 = amount.try_into().unwrap();`. **Always** check if `assert(amount_i128 >= 0)` or `assert(amount_i128 > 0)` is enforced immediately after `.try_into()`.
- [ ] **Audit decimal scaling between external ERC20 (`u256`) and internal ledger (`felt252`/`i128`)**:
  - When dividing by `scale_factor` (`10^(internal_decimals - token_decimals)`), verify that either:
    1. Internal accounting credits `amount_scaled * scale_factor` (the exact amount transferred), OR
    2. `assert(amount % scale_factor == 0)` and `assert(amount_scaled > 0)` are enforced.
- [ ] **Check `ContractAddress::is_non_zero()` guards on security hooks**:
  - If a hook validates caller ownership or transfer restrictions (`_detect_account_transfer_restriction`), verify it **fails closed** (reverts) when the registry address is zero, or still enforces `caller == from` when no registry is configured.
- [ ] **Verify post-execution health checks after collateral movement**:
  - In perpetual/margin DEX contracts, whenever a function moves both positions and collateral (`account_transfer_partial`), verify `check_account_health` / `get_account_free_balance >= 0` runs **at the very end** of the state transition, not before the collateral transfer!

#### C. Concrete Cited Examples
- **Report `#46888`** (High — `iop-paradex`): *Missing sign validation on `amount_collateral: felt252` in `account_transfer_partial` allows infinite balance minting and vault drain*.
  - **Mechanism**: In `paraclear.cairo::account_transfer_partial`, the parameter `amount_collateral: felt252` is checked only with `if amount_collateral != 0` and passed to `transfer_internal(sender, recipient, asset, amount_collateral)`. Inside `transfer_internal`:
    ```cairo
    let amount_i128: i128 = amount.try_into().expect(Errors::INTEGER_OVERFLOW);
    assert(sender_balance >= amount_i128, Errors::INSUFFICIENT_BALANCE);
    update_token_asset_balance(..., sender, -amount_i128);
    update_token_asset_balance(..., recipient, amount_i128);
    ```
    Because `amount_i128` is never checked to be `> 0`, an attacker passes `amount_collateral = (-1_000_000_000_000_i128).into()` (which is $P - 10^{12}$ as a `felt252`). `amount.try_into()` succeeds and produces `-1_000_000_000_000_i128`! `sender_balance (0) >= -10^12` passes, and `-amount_i128` adds $+10^{12}$ to the attacker's balance while subtracting $10^{12}$ from a throwaway `recipient` account!
  - **PoC Method**: Starknet Foundry (`snforge test test_transfer_poc`) calling `account_transfer_partial` with `(-amount).into()` followed by `withdraw` to drain all USDC from Paraclear.
- **Report `#46997`, `#47309`, `#47310`** (Medium — `iop-paradex`): *Negative `i128` account value / free balance cast to `felt252` and then `u256` wraps to $P \approx 3.6 \times 10^{75}$*.
  - **Mechanism**:
    - In `paraclear.cairo`, `getAccountValue` and `get_account_free_balance` compute a signed `i128` value and return `val.into()` (`felt252`).
    - In `vault.cairo`, `_total_assets()` calls `paraclear.getAccountValue(this).into()` (converting `felt252` to `u256`). When the vault's account value is negative (e.g., `-42_i128` due to funding/losses), `getAccountValue` returns $P - 42$, and `_total_assets()` becomes $3.618 \times 10^{75}$ (`#46997`)!
    - In `vault.cairo::_is_liquidated()`, it checks `total_assets == 0`. Because a negative account value wraps to $P - |x| \neq 0$, `_is_liquidated()` returns `false` even when the vault is deeply insolvent (`#47310`).
    - In `vault.cairo::request_withdrawal()`, `let free_balance: u256 = paraclear.get_account_free_balance(this, token).into();`. When `free_balance` in Paraclear is negative (`< 0`), `free_balance: u256` wraps to $\sim 3.6 \times 10^{75} \ge requested_amount`, tricking the contract into taking the `WITHDRAWAL_MODE_FAST` branch and immediately reverting inside `paraclear.withdraw` instead of queuing a slow withdrawal (`#47309`).
- **Report `#46892`** (High — `iop-paradex`): *Precision loss in `_deposit` allows minting internal collateral for 0 ERC20 tokens*.
  - **Mechanism**: In `paraclear.cairo::_deposit`, for 6-decimal USDC (`scale_factor = 100`), the code computes `let amount_scaled: u256 = amount.into() / self._scale_factor(decimals);`, calls `IERC20Dispatcher.transferFrom(caller, this, amount_scaled)`, and then credits the unscaled `amount` via `update_token_asset_balance(..., caller, amount_i128)`. Passing `amount = 99` sets `amount_scaled = 0`, transfers `0` USDC, and credits `99` internal units.
- **Report `#46843` & `#47370`** (Critical — `iop-paradex`): *Missing caller authorization in `account_transfer_partial` when `transfer_registry_address` is zero*.
  - **Mechanism**: `account_transfer_partial(sender, recipient, ...)` delegates authorization to `_detect_account_transfer_restriction(sender, recipient)`, which only checks restrictions `if transfer_registry_address.is_non_zero()`. When `transfer_registry_address` is `0`, `account_transfer_partial` never checks `get_caller_address() == sender`, allowing anyone to steal collateral and profitable positions from any user account.
- **Report `#47314`** (Medium — `iop-paradex`): *`account_transfer_partial` checks `excess_balance >= 0` BEFORE transferring collateral, leaving sender account underwater*.
  - **Mechanism**: In `paraclear.cairo::account_transfer_partial`, when `positions_to_transfer.len() > 0`, the function transfers positions, checks `assert(excess_balance >= 0, Errors::BELOW_INITIAL_MARGIN)`, and **only after that check** executes `if amount_collateral != 0 { transfer_internal(sender, recipient, ...); }` without re-checking `excess_balance >= 0`! A trader can transfer a dust position along with 100% of their collateral out of an account holding a losing perpetual position, leaving the protocol with unbacked bad debt.

---

### 5. Stacks / Clarity (`stacks-i-attackathon`, `stacks-ii-attackathon`)

#### A. Execution Model & Asset Model Pitfalls
1. **Clarity Principal Encoding & Version Bytes**:
   - In Stacks/Clarity, a `standard-principal` consists of a **1-byte version** followed by a **20-byte Hash160** (c32check encoded on-chain/off-chain). Only 4 version bytes are valid in Clarity:
     - `22` (`0x16`, `SP...`): Mainnet single-sig
     - `20` (`0x14`, `SM...`): Mainnet multisig
     - `26` (`0x1a`, `ST...`): Testnet single-sig
     - `21` (`0x15`, `SN...`): Testnet multisig
   - In Rust off-chain/bridge components (such as sBTC Signer/Emily), `clarity::vm::types::StandardPrincipalData::new(version: u8, bytes: [u8; 20])` only checks if `version >= 32` (`InvalidPrincipalVersion`), **allowing versions `0..=31` that are NOT valid Stacks principals** (e.g., version `0` = `S0...`). If a Bitcoin deposit script parser accepts version `0`, the user's BTC is swept by the Signers, but `sbtc-deposit::complete-deposit` on Stacks cannot mint sBTC to a version-`0` principal!
2. **Bounded Clarity Iteration (`fold` / `map`) vs Off-Chain Indexer & Webhook Limits**:
   - Clarity is intentionally non-Turing-complete (no `while` or unbounded recursion), so developers assume a single Clarity transaction cannot cause resource exhaustion.
   - **False Assumption**: Using nested helper functions or `(fold func (list ... 550 items) init)` inside a single Clarity transaction allows amplifying operations exponentially (`8^4 = 4096` calls) within the Clarity block execution budget:
     - Emitting 4,096 `(print ...)` events of 1 KB each produces a **4 MB event payload** in a single Stacks block. Because `stacks-node` pushes all events in a block via a single HTTP POST to `/new_block` on the sBTC Signer, and the Signer's `axum::Router` uses the default **2 MB `DefaultBodyLimit`**, `axum` rejects the POST with HTTP 413. `stacks-node` then enters an infinite 1-second retry loop on that block, permanently halting all sBTC event delivery!
     - Calling `sbtc-withdrawal::initiate-withdrawal-request` 550 times via `fold` in a single transaction floods the Signers' PostgreSQL database and triggers a foreign-key race condition (`withdrawal_signers_request_id_block_hash_fkey`), halting block signing across all Signers.
3. **Non-Exhaustive Clarity Event Topic Parsers in Off-Chain Observers**:
   - When a Clarity contract (`sbtc-registry.clar`) emits `(print { topic: "..." })` events for multiple actions, the off-chain Rust observer (`RegistryEvent::try_new`) iterates over every event in the `/new_block` webhook using `?` early-return error propagation. If **one** contract function (`update-protocol-contract`) emits a `topic` string that wasn't added to the Rust `match` statement, `RegistryEvent::try_new` returns `Err(ClarityUnexpectedEventTopic)`, aborting the entire `/new_block` handler and **dropping every other deposit/withdrawal event in that Stacks block**!
4. **Stateless Pre-Sign Validation of Clarity Contract Calls**:
   - When sBTC Signers validate a coordinator's request to sign a Stacks transaction (`complete-deposit`, `accept-withdrawal-request`, `reject-withdrawal-request`), checking only that the request exists in the Signer's local DB without checking whether the request was **already completed on-chain** allows a malicious/buggy coordinator to repeatedly request signatures for already-completed Clarity calls and drain the Signers' multisig STX balance via failed transaction fees.

#### B. Step-by-Step Auditing Checklist & Code Smells (Stacks / Clarity)
- [ ] **Check every `StandardPrincipalData` / `PrincipalData` deserialization boundary**:
  - When parsing a 21-byte principal from raw bytes (e.g., Bitcoin Taproot scripts, cross-chain messages, or Clarity buffers), verify that the 1-byte version is strictly checked against `{20, 21, 22, 26}` **and** matches the active network (`is_mainnet()`).
- [ ] **Audit all `(print ...)` topics in `.clar` contracts against off-chain event parsers**:
  - List every `(print { topic: "..." })` in every `.clar` file.
  - Check the off-chain indexer/signer `match topic` blocks.
  - Check what happens when the off-chain parser encounters an unknown event or malformed print payload: does it `continue` (skip the single event), or does it `?` (abort processing of the entire block)?
- [ ] **Test `(fold ...)` and nested call amplification against off-chain DB/HTTP limits**:
  - Calculate the maximum number of `(print ...)` bytes or `contract-call?` invocations a single Clarity transaction can fit into the block execution cost limit. Compare that against the HTTP server body limit (`axum` 2MB default) and DB batch constraints.
- [ ] **Check idempotency of off-chain Stacks transaction signing**:
  - Before signing a Clarity `contract-call` payload, verify the signer checks whether the request ID is already marked completed/rejected in `sbtc-registry`.

#### C. Concrete Cited Examples
- **Report `#38740`** (High — `stacks-i-attackathon`): *Missing principal version validation in `DepositScriptInputs::parse` locks deposited BTC*.
  - **Mechanism**: In `sbtc/src/deposits.rs`, `DepositScriptInputs::parse` reads the 21-byte recipient slice from the Bitcoin Taproot deposit script and calls `StandardPrincipalData::new(version, hash_bytes)` (or `PrincipalData::parse_qualified_contract_principal`). Because `StandardPrincipalData::new` accepts any `version < 32`, a deposit with `version = 0` passes validation, gets swept by the Signers on Bitcoin, and then fails permanently when minting sBTC on Stacks.
- **Report `#38111`** (High — `stacks-i-attackathon`): *Clarity `(print ...)` amplification exceeds sBTC Signer `axum` 2MB body limit, permanently halting `stacks-node` event dispatcher*.
  - **Mechanism**: In `signer/src/api/mod.rs::get_routes()`, the `/new_block` `axum::Router` does not override `DefaultBodyLimit` (2 MB). An attacker deploys a Clarity contract with 4 levels of nested functions (`e1` calls `e2` 8 times $\rightarrow$ `e2` calls `e3` 8 times $\rightarrow$ `e3` calls `e4` 8 times $\rightarrow$ `e4` calls `(print "A" x 1024)`), emitting >2 MB of print events in a single transaction for $0.08. `stacks-node` POSTs the >2 MB block payload to `/new_block`, receives HTTP 413, and retries the failed block payload every 1 second forever, freezing all sBTC events.
- **Report `#40692`** (High — `stacks-ii-attackathon`): *Batching 550 `initiate-withdrawal-request` calls via Clarity `fold` crashes sBTC Signers and halts block production*.
  - **Mechanism**: `sbtc-withdrawal.clar::initiate-withdrawal-request` can be called repeatedly in a single Clarity transaction via `(fold abuse-withdrawal (list true ... 550 times) true)` with dust amounts (`u1000` sats, `u0` fee). The resulting flood of 550 withdrawal events in a single block triggers a PostgreSQL foreign key constraint error (`withdrawal_signers_request_id_block_hash_fkey`) in the Signer binary, halting block signing.
- **Report `#38160`** (Medium/Insight — `stacks-i-attackathon`): *Unhandled `update-protocol-contract` print event aborts entire `/new_block` webhook handler*.
  - **Mechanism**: `sbtc-registry.clar::update-protocol-contract` emits `(print { topic: "update-protocol-contract", ... })`. In `signer/src/stacks/events.rs`, `RegistryEvent::try_new` only matches `withdrawal-accept`, `withdrawal-create`, `withdrawal-reject`, `completed-deposit`, and `key-rotation`. When `update-protocol-contract` is emitted, `RegistryEvent::try_new` returns `Err(Error::ClarityUnexpectedEventTopic)`, and `new_block_handler` uses `.collect::<Result<Vec<_>, _>>()?`, aborting processing of **all** events in that block.

---

### 6. Algorand / PyTeal / AVM (`folks-finance`, `folks-liquid-staking`, `folks-finance-wormhole-ntt-on-algorand`, `folks-smart-contract-library`)

#### A. Execution Model & Asset Model Pitfalls
1. **Application ID (`uint64`) vs 32-Byte Application Address (`sha512_256("appID" || itob(app_id))`)**:
   - Every Algorand smart contract (Application) has **two completely different identifiers**:
     1. **Application ID (`uint64`)**: An 8-byte integer (e.g., `12345678`) used in `InnerTxnBuilder.Execute({ TxnField.application_id: ... })` and `Global.current_application_id()`.
     2. **Application Address (`bytes[32]`)**: A 32-byte public-key hash computed as `SHA512_256("appID" || itob(app_id))`, accessed via `Global.current_application_address()` and `Txn.sender()` when the application makes an inner transaction!
   - **Critical Cross-Chain / Multi-Contract Pitfall**: In cross-chain protocols (like Wormhole NTT) that represent addresses as `bytes32` (`UniversalAddress`), developers frequently store or transmit `Global.current_application_address()` (a full 32-byte SHA512_256 hash with non-zero upper 24 bytes) in one contract (`NttManager`), while the receiving contract (`TransceiverManager`) attempts to convert that `bytes32` into an Application ID via `BytesUtils.safe_convert_bytes32_to_uint64(addr)` (which asserts that the first 24 bytes are `0x00..00` and extracts the last 8 bytes as `uint64`)! This mismatch causes 100% of cross-chain calls to panic!
2. **Strict AVM `uint64` Underflow Panics in Global State & Dynamic Array Lengths**:
   - In the Algorand Virtual Machine (AVM / TEAL), all `uint64` arithmetic (`+`, `-`, `*`, `/`) is strictly checked by the VM and immediately panics the entire atomic transaction group (`logic eval error: - would result negative`) if any subtraction results in `< 0`.
   - **Principal vs Yield State Desync**: If a liquid staking contract tracks deposited ALGO principal in `total_active_stake_key` and staking rewards in `total_rewards_key`, but on withdrawal (`burn()`) calculates `algo_to_send` from `(total_active_stake + total_rewards)` and subtracts the full `algo_to_send` from `total_active_stake_key` (`App.globalPut(total_active_stake_key, App.globalGet(total_active_stake_key) - algo_to_send)`), `total_active_stake_key` is drained faster than xALGO is burned! When the remaining xALGO holders try to burn and `algo_to_send > total_active_stake_key`, the AVM panics with `- would result negative`, permanently locking all remaining ALGO in the contract!
   - **Eager Expression Evaluation in PyTeal**: In PyTeal, `Seq` / `If` expressions or scratch variable assignments like `last_idx.store(items.length() - Int(1))` placed *before* an empty-array check (`If(items.length() == Int(0))`) will evaluate `0 - 1` at runtime on an empty array and panic the AVM!

#### B. Step-by-Step Auditing Checklist & Code Smells (Algorand / PyTeal)
- [ ] **Check every conversion between `bytes32`, `ApplicationAddress`, and `ApplicationID` (`uint64`)**:
  - Grep for `safe_convert_bytes32_to_uint64`, `Btoi`, `Itob`, `Global.current_application_address()`, and `Global.current_application_id()`.
  - Whenever a contract stores a peer or handler address as `bytes32` or compares `Txn.sender()` against a stored `uint64` app ID, check whether the stored value is `Pad(24) || Itob(app_id)` (8-byte ID) or `sha512_256("appID" || Itob(app_id))` (32-byte address)!
- [ ] **Audit every subtraction (`-`) in PyTeal / TEAL for `uint64` underflow**:
  - For every `App.globalPut(k, App.globalGet(k) - x)`: verify that every component added into `x` (such as rewards, fees, or interest) was previously added into `App.globalGet(k)`!
  - For every `length - Int(1)` on ABI dynamic arrays or Box slices: verify it is strictly inside a branch where `length > Int(0)` is guaranteed at runtime.
- [ ] **Audit Atomic Transaction Group (`Gtxn`) Field Constraints**:
  - Verify `Gtxn[i].type_enum()`, `Gtxn[i].xfer_asset()`, `Gtxn[i].asset_receiver() == Global.current_application_address()`, and ensure no unvalidated inner/group transaction assumptions can be bypassed.

#### C. Concrete Cited Examples
- **Report `#56615`, `#57018`, `#57333`** (High — `folks-finance-wormhole-ntt-on-algorand`): *32-byte Algorand `ApplicationAddress` vs 8-byte `uint64` `ApplicationID` mismatch in `TransceiverManager` & `NttManager` bricks cross-chain transfers*.
  - **Mechanism**:
    1. In `NttManager._transfer`, the outgoing `TransceiverMessage` sets `handler_address` to the peer contract's 32-byte address (`Bytes32`). On Algorand, `MessageHandler.get_message_handler_address()` returns `Global.current_application_address()` (the 32-byte SHA512_256 hash of `"appID" || itob(app_id)`).
    2. When an incoming transfer arrives on Algorand in `TransceiverManager.py::attestation_received`, it executes:
       `message_handler_app_id = BytesUtils.safe_convert_bytes32_to_uint64(message.handler_address.get())`
       `safe_convert_bytes32_to_uint64` asserts `Extract(bytes, 0, 24) == BytesZero(24)`!
    3. If the sender encoded the Algorand `NttManager`'s 32-byte `ApplicationAddress`, `safe_convert_bytes32_to_uint64` reverts with `"Unsafe conversion of bytes32 to uint64"` because the first 24 bytes of a SHA512_256 hash are non-zero!
    4. Conversely, if the sender encoded the 8-byte `ApplicationID` left-padded with 24 zero bytes (`0x00..00 || itob(app_id)`), `safe_convert_bytes32_to_uint64` succeeds, but `MessageHandler.internal_attestation_received` then compares `message.handler_address == Global.current_application_address()`, which reverts with `"Message handler address mismatch"`! Either way, 100% of incoming cross-chain transfers to Algorand are permanently bricked.
  - **PoC Method**: Python `algosdk` / `pytest` test constructing a `TransceiverMessage` with both representations and showing both revert.
- **Report `#37660`, `#37661`, `#37863`, `#37889`** (High — `folks-liquid-staking`): *AVM `uint64` underflow on `total_active_stake_key` in `burn()` permanently freezes ~111,000 ALGO on Mainnet*.
  - **Mechanism**: In `contracts/consensus_v2.py`:
    - `immediate_mint()` and `claim_delayed_mint()` increment `total_active_stake_key` by the deposited ALGO principal (`algo_Mainnet_deposited`).
    - Staking rewards are added to `total_rewards_key` (via `propose_block` / `update`), **not** to `total_active_stake_key`.
    - When users call `burn()` to redeem `xALGO` for `ALGO`, `algo_to_send` is calculated pro-rata from `total_active_stake + total_rewards - total_unclaimed_fees` (so `algo_to_send = principal + rewards`).
    - However, `burn()` then executes:
      `App.globalPut(total_active_stake_key, App.globalGet(total_active_stake_key) - algo_to_send.load())`!
    - Because every `burn()` subtracts both principal **and** rewards from `total_active_stake_key`, `total_active_stake_key` hits `0` while users still hold `xALGO` backed by `total_rewards_key`. Every subsequent `burn()` call panics with AVM `uint64` underflow (`- would result negative`), permanently freezing all remaining ALGO in the contract (~111,000 ALGO on Algorand Mainnet at the time of the report).
- **Report `#48998`** (Low/DoS — `folks-smart-contract-library`): *Eager evaluation of `items.length - 1` in `UInt64SetLib.py::remove_item` panics on empty set*.
  - **Mechanism**: `UInt64SetLib.py::remove_item` computes `last_idx = items.length - 1` before checking if the dynamic array is empty; when `items.length == 0`, AVM panics on `0 - 1` underflow.

---

### 7. Chia / Chialisp (`circuitdaoiop`)

#### A. Execution Model & Asset Model Pitfalls
1. **Coin Set (UTXO) Model, CAT TAIL Programs, and Untrusted Solution Parameters**:
   - In Chia, every coin is an immutable UTXO with ID `coin_id = sha256(parent_coin_id ++ puzzle_hash ++ amount)`. There is no persistent global contract storage: state is carried forward by **currying** state variables into the child coin's `puzzle_hash` via `CREATE_COIN`.
   - Every spend supplies two inputs to the CLVM: the **Puzzle** (curried program logic) and the **Solution** (untrusted caller-supplied arguments).
   - **Chia Asset Tokens (CATs) & TAILs**: Any user can create an unissued CAT coin of **any arbitrary `amount`** (by wrapping an XCH coin in the `cat_v2.clsp` outer puzzle) and spend it with `delta = 0` by invoking the token's **TAIL** (Token And Issuance Limitations) program (`byc_tail.clsp`).
   - **The Fatal Chialisp Coin-ID & Solution-Amount Desync**:
     - Suppose a Vault coin (`vault_borrow.clsp`) authorizes minting `borrow_amount` of BYC CAT by sending a `SEND_MESSAGE 0x3f` (which binds sender coin ID and receiver coin ID) to `byc_issuing_coin_id`.
     - How does `vault_borrow.clsp` know `byc_issuing_coin_id`? The borrower passes `byc_issuing_coin_info = (parent_id amount inner_puzzle_hash)` in the Vault's **solution**, and `vault_borrow.clsp` hashes those three fields to get `byc_issuing_coin_id`—**without checking that `(f (r byc_issuing_coin_info))` (`amount`) equals `borrow_amount`**!
     - Then, in `byc_tail.clsp` (running inside the CAT coin whose actual coin amount in `Truths` is `10,000,000,000`), the TAIL receives an untrusted solution parameter `new_byc_coin_amount = 1`, verifies `(RECEIVE_MESSAGE 0x3f (concat ... new_byc_coin_amount) vault_coin_id)`—**and never asserts `(= new_byc_coin_amount (my_amount_cat_truth Truths))`**!
     - Result: Both coins agree on `byc_issuing_coin_id` (whose real UTXO amount is `10,000,000,000`) and both agree on the message payload (`borrow_amount = 1`), allowing the attacker to mint **10 billion BYC** while recording only **1 BYC** of debt in the Vault!
2. **Singleton Identity (`LAUNCHER_ID`) vs Mutable `INNER_PUZZLE_HASH`**:
   - In Chia, a Singleton coin's permanent, unforgeable identity across spends is its **`LAUNCHER_ID`** (stored in its `SingletonStruct`). Its **`INNER_PUZZLE_HASH`** (which represents the owner's public key/control puzzle) is **mutable** across spends (e.g., via `announcer_mutate.clsp`).
   - If a registry coin (`announcer_registry.clsp`) tracks registered singletons by storing their `INNER_PUZZLE_HASH` in `ANNOUNCER_REGISTRY` and checking `(not (contains ANNOUNCER_REGISTRY announcer_inner_puzzle_hash))` instead of keying on `LAUNCHER_ID`, a single approved singleton can endlessly loop: **Register (`hash_A`) $\rightarrow$ Mutate (`hash_A` $\rightarrow$ `hash_B`) $\rightarrow$ Register (`hash_B`) $\rightarrow$ Mutate (`hash_B` $\rightarrow$ `hash_C`)**, stuffing the registry with Sybil entries and stealing ~100% of governance token (`CRT`) mint rewards!
3. **Unconstrained Value Differences (Mojo Conservation in SpendBundles)**:
   - In Chia's UTXO SpendBundle model, the sum of all input coins minus the sum of all `CREATE_COIN` outputs across a SpendBundle equals the transaction fee (or can be claimed by any coin spend in the bundle that creates an output without burning input).
   - If a slashing/penalization operation (`announcer_penalize.clsp`) recreates a singleton coin with a smaller amount (`penalized_deposit < DEPOSIT`) **without** requiring a `CREATE_COIN` or announcement that locks the slashed difference (`DEPOSIT - penalized_deposit`) into a protocol treasury/burn puzzle hash, the "slashed" XCH mojos are simply left floating in the SpendBundle! The misbehaving announcer owner can immediately front-run or self-call `announcer_penalize` and sweep 100% of their own "slashed" XCH deposit back into their own wallet!

#### B. Step-by-Step Auditing Checklist & Code Smells (Chia / Chialisp)
- [ ] **Check every TAIL program (`*_tail.clsp`) against `cat_truths.clib` (`Truths`)**:
  - In a CAT TAIL program (`(mod (Truths parent_is_cat lineage_proof delta inner_conditions solution_params) ...)`), **never** trust a coin amount, coin ID, or inner puzzle hash passed inside `solution_params` unless it is explicitly asserted equal to `(my_amount_cat_truth Truths)`, `(my_id_cat_truth Truths)`, or `(my_inner_puzzle_hash_cat_truth Truths)`.
- [ ] **Check every `calculate_coin_id` / `SEND_MESSAGE 0x3f` / `CREATE_PUZZLE_ANNOUNCEMENT` across interacting coins**:
  - When Coin A computes `target_coin_id` from a caller-supplied tuple `(parent_id amount puzzle_hash)` in order to send a `0x3f` message to Coin B, check whether Coin A validates `amount` against the authorized protocol amount (e.g., `borrow_amount` or `withdraw_amount`).
- [ ] **Verify Singleton uniqueness uses `LAUNCHER_ID`, never `INNER_PUZZLE_HASH`**:
  - Whenever a puzzle maintains a list, map, or Merkle tree of unique protocol actors (Announcers, Vaults, Oracles), verify that the deduplication key is the immutable `LAUNCHER_ID` (verified via singleton lineage/puzzle hash calculation), not the mutable `INNER_PUZZLE_HASH`.
- [ ] **Trace Mojo Conservation on Slashing / Fee / Burn Operations**:
  - Whenever a puzzle reduces a coin's `CREATE_COIN` output amount (`new_amount < old_amount`), ask: *Where do the remaining `old_amount - new_amount` mojos go?* If there is no `CREATE_COIN` sending `old_amount - new_amount` to a burn/treasury puzzle hash, whoever constructs the SpendBundle steals those mojos!

#### C. Concrete Cited Examples
- **Report `#43705`** (Critical — `circuitdaoiop`): *Unconstrained CAT coin amount in `vault_borrow.clsp` and `byc_tail.clsp` allows minting unlimited BYC stablecoins with minimal collateral*.
  - **Mechanism**:
    1. In `puzzles/collateral_vault/vault_borrow.clsp`, the solution passes `borrow_amount` (e.g., `1`) and `byc_issuing_coin_info` (`(parent_id amount inner_puzzle_hash)`). `vault_borrow.clsp` checks that the vault's XCH collateral covers `borrow_amount = 1`, computes `byc_issuing_coin_id` using `(f (r byc_issuing_coin_info))` (which the attacker sets to `10_000_000_000`), and emits `(SEND_MESSAGE 0x3f (concat PROTOCOL_PREFIX ... borrow_amount ...) byc_issuing_coin_id)`.
    2. In `puzzles/byc_tail.clsp`, the solution passes `new_byc_coin_amount` (which the attacker sets to `1`, while the actual CAT coin being spent has `Truths` amount `10_000_000_000`). `byc_tail.clsp` checks `(RECEIVE_MESSAGE 0x3f (concat PROTOCOL_PREFIX ... new_byc_coin_amount ...) vault_coin_id)`—and **never checks `(= new_byc_coin_amount (my_amount_cat_truth Truths))`**!
  - **Attack Path & PoC**: Attacker creates an eve CAT coin of `10,000,000,000` BYC, borrows `1` BYC against minimal XCH collateral, and passes `new_byc_coin_amount = 1` in `byc_tail.clsp`'s solution. The `0x3f` message matches both coin IDs and payload `1`, legitimizing a 10 billion BYC CAT coin and draining all protocol stability pools and AMMs. Verified via `pytest` SpendBundle simulation.
- **Report `#44355`** (High — `circuitdaoiop`): *Sybil registration in `announcer_registry.clsp` via `INNER_PUZZLE_HASH` rotation steals CRT minting rewards*.
  - **Mechanism**: `announcer_registry.clsp` stores `announcer_inner_puzzle_hash` in `ANNOUNCER_REGISTRY` and checks `(not (contains ANNOUNCER_REGISTRY announcer_inner_puzzle_hash))` during `REGISTER`. Because an approved announcer singleton can change its `INNER_PUZZLE_HASH` at any time via `announcer_mutate.clsp` while keeping its `GOVERNANCE_ACTIVATION` status, a single announcer can repeatedly call `mutate(hash_i -> hash_{i+1})` and `register(hash_{i+1})` in a single block to occupy 99% of `ANNOUNCER_REGISTRY` slots and claim 99% of newly minted CRT governance tokens.
- **Report `#44324`** (Medium — `circuitdaoiop`): *Self-penalization in `announcer_penalize.clsp` allows announcers to evade slashing at zero cost*.
  - **Mechanism**: `announcer_penalize.clsp` slashes an expired/unresponsive announcer by recreating the announcer singleton with `penalized_deposit` instead of `DEPOSIT`, leaving `DEPOSIT - penalized_deposit` XCH mojos unclaimed in the SpendBundle as a keeper incentive. Because `announcer_penalize.clsp` is permissionless, the announcer owner can simply bundle `announcer_penalize` themselves and direct the `DEPOSIT - penalized_deposit` XCH change output right back to their own wallet.

---

## Section 2: Cross-Chain & VM-Agnostic Invariants That Repeatedly Fail When Porting EVM Mental Models

When developers port Solidity/EVM protocols (Liquity, Compound V3, MakerDAO, Seaport, ERC-4626, Wormhole NTT) to non-EVM chains, **seven specific EVM mental-model assumptions fail catastrophically**:

### Invariant 1: "Signed/Unsigned Casts and Sub-Word Arithmetic Behave Like 2's Complement Fixed-Width Types"
- **The EVM Mental Model**: In Solidity, `int128(-42)` has a fixed 128-bit two's complement representation, and `uint8`/`uint16`/`uint32` arithmetic overflows at $2^8 / 2^{16} / 2^{32}$ in Solidity $\ge 0.8.0$.
- **Why It Fails on Non-EVM**:
  - **Starknet (Cairo)**: The native word is a prime-field element `felt252` $\in [0, P-1]$. Casting a negative `i128` (`-42`) to `felt252` via `.into()` produces $P - 42 \approx 3.618 \times 10^{75}$; casting that `felt252` to `u256` turns a negative balance into an astronomical positive balance (`#46997`, `#47309`, `#47310`). Conversely, accepting `felt252` and calling `.try_into()` to `i128` allows passing $P - x$ to inject negative transfer amounts (`#46888`).
  - **Fuel (Sway)**: Registers are 64-bit (`u64`), so `u8`/`u16`/`u32` operations (`pow`) do not trigger hardware overflow at $2^8 / 2^{16} / 2^{32}$ unless manually guarded (`#33331`), and signed integers (`I8..I256`, `IFP64..IFP256`) are emulated in software via offset bias `indent = 2^(bits-1)` or boolean sign flags, leading to sign-inversion bugs (`#35758`, `#32706`, `#33039`, `#33248`).
  - **Algorand (AVM)**: All integers are `uint64` that hard-panic on `< 0` underflow—including when subtracting `principal + yield` from a global state variable that only tracked `principal` (`#37660`), or when evaluating `array.length - 1` before an `if array.length == 0` guard (`#48998`).

### Invariant 2: "Transferring `0` Value or Sending Excess Native/Token Value is Safe or Automatically Reverted/Refunded"
- **The EVM Mental Model**: In Solidity, ERC20 `transfer(to, 0)` is a harmless no-op, ERC20 `transferFrom(user, address(this), amount)` pulls the exact `amount` (never excess), and `msg.value` only applies to native ETH.
- **Why It Fails on Non-EVM**:
  - **Fuel (Sway)**: Every asset (fungible token or NFT) is a native UTXO asset attached to the call frame via `(msg_asset_id(), msg_amount())` and immediately credited to the contract's balance. Checking `msg_amount() >= amount` (instead of `==` or refunding `msg_amount() - amount`) permanently locks excess tokens inside the contract (`#35793`, `#35876`). Furthermore, FuelVM's `transfer(to, asset, 0)` (`TR`/`TRO` opcode) **panics and reverts the transaction** if `amount == 0`, breaking trades whenever `protocol_fee == 0` or `royalty_fee == 0` (`#34966`).
  - **Chia (Chialisp)**: Reducing a coin's output amount (`CREATE_COIN` with `penalized_deposit < DEPOSIT`) without explicitly creating an output coin for the difference (`DEPOSIT - penalized_deposit`) leaves the difference unconstrained in the SpendBundle for the transaction creator to pocket (`#44324`).

### Invariant 3: "A Contract's Identity and Its Asset Escrow Address Are the Same 20/32-Byte Value"
- **The EVM Mental Model**: In EVM, `address(this)` is both the target of `STATICCALL`/`CALL` and the address that holds ETH/ERC20 balances.
- **Why It Fails on Non-EVM**:
  - **Algorand (AVM)**: A contract is invoked by its 8-byte `uint64` **Application ID** (`Global.current_application_id()`), while its escrow account and inner-transaction `Txn.sender()` is its 32-byte **Application Address** (`SHA512_256("appID" || itob(app_id))`). Treating a 32-byte `UniversalAddress` as a zero-padded `uint64` Application ID in one module (`safe_convert_bytes32_to_uint64`) and as a 32-byte `ApplicationAddress` in another bricks 100% of Wormhole NTT messages (`#56615`, `#57018`, `#57333`).
  - **Chia (Chialisp)**: A singleton's permanent identity is its immutable `LAUNCHER_ID`, whereas its `INNER_PUZZLE_HASH` is mutable owner state. Keying a registry on `INNER_PUZZLE_HASH` allows infinite Sybil registration via state mutation (`#44355`).
  - **Aptos / Movement (Move)**: An account address can exist with a `CoinStore<AptosCoin>` while lacking an initialized `FungibleStore` (`#42480`).

### Invariant 4: "A Token/Coin Balance Cannot Change Without Executing the Vault Contract's Code, and Coin Amounts Are Self-Authenticating"
- **The EVM Mental Model**: In EVM, a contract tracks deposits via `transferFrom` and `msg.sender`.
- **Why It Fails on Non-EVM**:
  - **Solana (SVM)**: Anyone can `spl_token::transfer` SPL tokens directly into a vault's ATA (`vault_st_token_account.amount`) without calling the vault program. When a vault computes rewards as `vault_st_token_account.amount - vault.tokens_deposited` inside an `UpdateVaultBalance` crank without linear vesting or pre-deposit syncing, attackers deposit right after the SPL transfer (before the crank) and steal the epoch's yield (`#36903`, `#37295`, `#37311`, `#37315`).
  - **Chia (Chialisp)**: Anyone can wrap XCH into an unissued CAT coin of arbitrary `amount` and spend it through the CAT's TAIL program. If the TAIL program verifies a cross-coin `RECEIVE_MESSAGE` against a **solution argument** `new_byc_coin_amount` instead of the coin's actual UTXO amount `(my_amount_cat_truth Truths)`, the attacker mints 10 billion tokens while borrowing 1 token (`#43705`).

### Invariant 5: "Storage Reentrancy Locks and Early Returns Are Scoped or Safe, and `constructor` Only Runs Once at Deploy Time"
- **The EVM Mental Model**: In Solidity, `constructor()` is stripped from runtime bytecode, and OpenZeppelin `nonReentrant` modifiers always run their post-body `_status = NOT_ENTERED` cleanup even if an internal helper returns early.
- **Why It Fails on Non-EVM**:
  - **Fuel (Sway)**: Sway has no language-level modifiers or deploy-only constructors. `fn constructor()` is a public runtime function unless locked (`#35853`), ABI supertrait methods are compiled into public dispatch entrypoints even though omitted from `contract-abi.json` (`#33351`), and manual storage locks (`storage.lock_internal_...write(true)`) that hit an early `return` statement before `.write(false)` remain `true` in persistent storage forever, permanently bricking the protocol (`#37323`).

### Invariant 6: "`block.timestamp` Is Always Unix Epoch Seconds"
- **The EVM Mental Model**: `block.timestamp` is Unix epoch seconds ($\sim 1.7 \times 10^9$), directly comparable to Pyth, Redstone, and Chainlink timestamps.
- **Why It Fails on Non-EVM**:
  - **Fuel (Sway)**: `std::block::timestamp()` returns **TAI64** ($2^{62} + \text{unix\_seconds} + 10 \approx 4.61 \times 10^{18}$). Comparing it directly to a Unix timestamp from Redstone or Pyth skews every freshness check by $2^{62}$ seconds (`#37276`).

### Invariant 7: "If a Transaction Fits Within On-Chain VM Gas/Execution Limits, Off-Chain Bridge Signers and Indexers Can Process It"
- **The EVM Mental Model**: Indexers pull logs via paginated RPC queries (`eth_getLogs`), and gas limits bound log sizes.
- **Why It Fails on Non-EVM**:
  - **Stacks (Clarity)**: Clarity's `fold` and nested function calls allow a single valid transaction to emit >2 MB of `(print ...)` events or 550 withdrawal contract calls within Clarity's block limit. When `stacks-node` pushes the entire block's event payload in one HTTP POST to the sBTC Signer's `/new_block` endpoint, it exceeds `axum`'s 2 MB body limit (`#38111`) or triggers DB foreign-key errors (`#40692`), or a single unhandled event topic aborts the entire block parser via Rust's `?` operator (`#38160`), halting the bridge.
