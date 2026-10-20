# Web3 dApp, Wallet, Name Resolution, Migration & Off-Chain Infrastructure Auditing Playbook + Immunefi Severity Escalation & Triage Engineering

**Corpus Basis:**
- Full analysis of `web3_apps_mhc.md` (461 Medium/High/Critical reports across `ens-or-audit-competition` [380], `quantus-or-audit-competition` [30], `shardeum-ancillaries` I/II/III [41], `zano-trade-iop` [7], and `zano-iop` [3]).
- Comparative analysis of `low_and_insight.md` (1,618 Low and Insight reports across Web3 apps, smart contracts, and blockchain/DLT targets).
- Deep-dive inspection of **41 full individual `.md` report files** spanning Critical, High, Medium, Low, and Insight severities.

---

# PART A — Web3 dApp, Wallet, Name Resolution, Migration & Off-Chain Infrastructure Auditing Playbook

## 1. Displayed-vs-Signed Transaction / Identity Divergence & ENSIP-15 Normalization Mismatches

### Core Vulnerability Mechanics
A Web3 frontend or wallet derives **displayed trust signals** (owner, expiry, warning banners, recipient preview, decoded extrinsic summary) from one normalization/decoding pipeline while constructing **signed transaction calldata or identity lookups** from a second pipeline. When on-chain registries permit raw/unnormalized labels (`ETHRegistrar` hashes raw bytes via `LibLabel.id(label)` and `StandardRentPriceOracle.isValid` checks length without ENSIP-15 normalization) or when a runtime decoder differs from a client SDK decoder by even 1 byte, an attacker crafts an input that splits the two pipelines.

### Sub-Patterns & Exact Code Signatures

#### 1A. Split-Node Profile Resolution (Normalized Owner/Expiry vs. Raw Address/Records)
- **Representative Reports:** `#89270` (Crit), `#89360` (Crit), `#89408` (Crit), `#89535` (Crit), `#91869` (High), `#92128` (High), `#92740` (High), `#92775` (High), `#92925` (High), `#91202` (Med).
- **Exact Code Pattern (`apps/manager`):**
  - `apps/manager/src/routes/$name/index.tsx:51-56` prefetches `profileOwnerQuery(name)`, `profileExpiryQuery(name)`, `profileRegistrationQuery(name)`, `profileRecordsQuery(name)`, and `profileEthAddressQuery(name)` from the raw route param `$name`.
  - `profileOwner.ts:25` (`normalizeEthName`), `profileExpiry.ts:77` (`normalizeEth2LdName`), and `profileRegistration.ts:66` normalize the input before calling `packetToBytes`, resolving to canonical `vitalik.eth`.
  - `profileRecords.ts:49,82,122-132` (`fetchIndexerDomain` and `getRecords(client, { name })`) and `profileEthAddress.ts:29-49` pass raw `name` directly to `packetToBytes(name)` without normalization.
  - Furthermore, `addFavorite.ts:18-21` stores the raw route param in localStorage (`favorites.names`), persisting the spoofed link on the victim's Home dashboard (`#89270`).
- **Exploit Vectors:**
  1. **Uppercase ASCII (`Vitalik.eth`):** `namehash("Vitalik.eth") != namehash("vitalik.eth")`.
  2. **Default-Ignorable Format Characters (`vita\u00ADlik.eth`, `vitalik\uFE0F.eth`, `vitalik\u200B.eth`):** ENSIP-15 strips `U+00AD` (Soft Hyphen), `U+FE0F` (Variation Selector-16), and `U+200B` (Zero-Width Space) during `ens_normalize`, mapping `vita\u00ADlik.eth` -> `vitalik.eth`. In `ProfileView.tsx:266`, `<Typo.Title className="truncate">` applies `white-space: nowrap`, under which CSS Text Level 3 renders `U+00AD` with zero width (`181.23px === 181.23px` in Chromium, `#89360`), making the header pixel-identical to `vitalik.eth` while `Main receiving address` (`coinType 60`) displays the attacker's wallet.

#### 1B. Raw Ownership Guard vs. Normalized Token ID Execution (Direct NFT Theft)
- **Representative Reports:** `#89278` (Crit), `#89474` (Crit), `#90691` (Crit), `#90794` (Crit), `#91004` (Crit), `#91044` (Crit), `#91967` (High), `#92111` (High), `#92300` (High), `#92353` (High), `#92416` (High), `#92762` (High).
- **Exact Code Pattern (`apps/portal`):**
  - `apps/portal/src/routes/$name/index.tsx:438-460` validates `isValidEnsName(name)`, but child routes `apps/portal/src/routes/$name/ownership/transfer.tsx:21-28,103-110` and `roles.tsx` do **not** run `isValidEnsName`.
  - The route guard checks ownership via `resolveEnsOwner({ name })` (`resolveEnsOwner.ts:62` -> `packetToBytes("ALICE.eth")`, raw bytes).
  - However, `useCanTransferName.ts:55-73` and `useTransferName.ts:106-119` compute the ERC-1155 token ID via `getEnsTokenId({ label: getLabel(name), registryAddress })`, where `getLabel.ts:10-19` runs `ens_normalize("ALICE")` -> `"alice"`.
- **Exploit Scenario:** Victim owns valuable `alice.eth`. Attacker registers worthless unnormalized `ALICE.eth` on `ETHRegistrar` and transfers `ALICE.eth` to the victim. Victim visits `/ALICE.eth/ownership/transfer` to discard or return `ALICE.eth`. The UI heading says `Transfer ALICE.eth`, the ownership check passes against raw `ALICE.eth`, and `transferToken.ts:39-47` encodes `erc1155Abi.safeTransferFrom(victim, attacker, tokenId("alice"), 1n, "0x")`, stealing the canonical `alice.eth` NFT.

#### 1C. Renewal Route Normalization Splits (Victim Pays to Renew Attacker's Raw Name or Vice Versa)
- **Representative Reports:** `#89456` (Crit), `#89467` (Crit), `#89548` (Crit), `#90820` (Crit), `#90883` (Crit), `#91080` (Crit), `#91098` (Crit), `#92028` (High), `#92043` (High), `#92121` (High), `#92155` (High), `#92278` (High), `#92345` (High), `#92761` (High), `#92794` (High), `#92840` (High), `#92850` (High).
- **Exact Code Pattern:**
  - **Manager single renew (`apps/manager/src/routes/renew/$name.tsx:20-61`):** `profileExpiryQuery(name)` normalizes via `normalizeEth2LdName(name)` (stripping `U+200B` ZWSP), displaying the victim's real `vitalik.eth` expiry and suppressing the third-party owner warning (`profileOwnerQuery` also normalizes so `isSelfOwned === true`). Meanwhile, `parseRenewableName(name)` calls `name-parser.ts:46` (`name.trim().toLowerCase()`), which preserves `U+200B` (`vitalik\u200B`), and `registration.actors.ts:1020-1031` encodes `ETHRegistrar.renew("vitalik\u200B", duration, ...)` (`#89456`).
  - **Manager bulk renew (`NamesTable.tsx:83-90`, `#92345`, `#92794`) & Portal renew (`#92043`, `#92840`):** The inverse split—the UI table renders raw `domain.name` (e.g., an expiring name the user selected) while the transaction builder deduplicates or signs using `domain.normalizedName`.

#### 1D. Encoded-Labelhash (`[<64-hex>]`) & Dotted-Label Reparsing Confusion
- **Representative Reports:** `#90725` (Crit), `#90749` (Crit), `#90874` (Crit), `#92110` (High), `#92411` (High), `#92764` (High), `#92817` (High), `#92818` (High), `#92842` (High).
- **Exact Code Pattern (`apps/portal`):**
  - `apps/portal/src/features/subnames/hooks/useSubnames.ts:48-84` fetches authoritative `{ name, labelName, labelhash }` from the Panoptes indexer, where a literal bracketed subname `[75aaf377...cb81f].ens.eth` has `labelhash = keccak256("[75aaf377...cb81f]") = 0x20dc...`.
  - `apps/portal/src/routes/$name/subnames.tsx:370-390` discards `sub.labelhash` when building `SubnameRow`.
  - When the parent owner clicks Delete on `[75aaf377...cb81f].ens.eth`, `getLabel(subname.name)` strips `.${name}` and passes `label = "[75aaf377...cb81f]"` into `deleteSubnameWriteParameters` (`delete-subname.helpers.ts:24-54`), which delegates to viem/ENSjs `labelhash()`. Because `label` matches `/^\[[0-9a-fA-F]{64}\]$/`, viem strips the brackets instead of hashing the literal string, returning `0x75aaf377...cb81f` (`labelhash("valuable")`) and encoding `UserRegistry.unregister(uint256(0x75aaf377...cb81f))`!
  - **Privilege Escalation Chain (`#90725`):** A delegated subname operator with only `ROLE_REGISTRAR` (`1 << 0`) and **not** `ROLE_UNREGISTER` (`1 << 4`) cannot overwrite `valuable.parent.eth` (`UserRegistry.register` reverts with `NameAlreadyRegistered`). Instead, the operator registers the literal spam subname `[<hex(labelhash("valuable"))>].parent.eth`. When the parent admin deletes that spam row in Portal, the admin's wallet executes `unregister(labelhash("valuable"))`, burning the real `valuable.parent.eth` so the `ROLE_REGISTRAR` operator can immediately register `valuable.parent.eth` to themselves.
  - **Dotted Flat Labels (`#90749`, `#90874`, `#92411`, `#92818`):** Registering a single label containing a literal dot (`"a.b"` under `parent.eth`) creates a 1-hop child `keccak256("a.b")`, whereas `packetToBytes("a.b.parent.eth")` and `findResolver` split on `.` into a 2-hop path `a` -> `b` -> `parent.eth`, causing role grants, resolver edits, and record updates to target the wrong node.

#### 1E. Unsafe Address-Resolution Fallback to Stale V1 Controller (`resolveAddressOrName`)
- **Representative Reports:** `#89395` (Crit), `#89398` (Crit), `#89585` (Crit), `#90852` (Crit), `#91063` (Crit), `#91900` (High), `#92880` (High), `#92902` (High), `#92246` (Med).
- **Exact Code Pattern (`apps/portal`):**
  - `apps/portal/src/features/permissions/handlers/addUser.handlers.ts:11-38` (`resolveAddressOrName`) calls `getEnsAddress({ name })`; when `getEnsAddress` returns `null`, it falls back to `getEnsOwner({ name })`.
  - `resolveEnsOwner.ts:116-127` checks the V2 registry first, and on V1 queries `NameWrapper.ownerOf`, `BaseRegistrar.ownerOf` (`registrant`), and `ENSRegistry.owner(node)` (`registryOwner`), returning ` { owner: registryOwner }` while **dropping `registrant`**!
  - Because `useAddressResolution` (`useAddressResolution.ts:55`) is shared by `SendNameForm.tsx:81` (NFT Transfer), `create-subname.tsx:109` (Subname Minting), and `AddUserSidebar` (Role Grants), and `AddressNameInput.tsx` renders `"Resolved: 0x..."` identically for forward-resolved and fallback-owner addresses:
    1. **Stale V1 Seller Hijack (`#89395`, `#91063`):** When an unwrapped V1 `.eth` name is bought on OpenSea (`BaseRegistrar.transferFrom`), `registrant` updates to the buyer (`bob.eth`) while V1 `ENSRegistry.owner(node)` remains the seller (`0xSeller`) and `ethAddress` is unset. Any user transferring a V2 name or minting a subname to `"bob.eth"` sends ownership directly to `0xSeller`.
    2. **Unmigrated V1 Name Hijacked by V2 Squat (`#89398`, `#92880`):** Because `resolveEnsOwner` checks V2 before V1 without checking V1 migration/exclusivity state, an attacker who registers an unmigrated V1 name's label on V2 hijacks `resolveAddressOrName` fallback when the V1 name has no `addr` record.

#### 1F. Substrate / SCALE Runtime-vs-Client Decoder Width & Fallback Mismatches (Quantus)
- **Representative Reports:** `#88483` (High), `#89808` (High), `#89987` (High), `#88667` (High), `#88914` (High), `#89008` (High), `#89011` (High), `#89151` (High), `#89283` (High), `#89657` (High), `#89798` (High), `#89806` (High), `#90351` (High).
- **Exact Code Patterns (`quantus-chain` + `quantus-apps`):**
  1. **Prefix `Decode::decode` on-chain vs. strict `DecodeAll` (`hasMore`) in Dart SDK (`#88483`, `#89657`):**
     - `pallets/multisig/src/lib.rs:599-600,1125-1126` decodes stored `call_data` with `RuntimeCall::decode(&mut &call[..])`, which ignores trailing bytes (`0x00`).
     - `packages/quantus_sdk/lib/src/services/call_decoder.dart:52-59` calls `RuntimeCall.codec.decode(input)` and then checks `if (input.hasMore) throw FormatException(...)`.
     - `mobile-app/lib/services/multisig_service.dart:315-320` (`_withOnChainCallData`) catches that throw and returns `tx.copyWith(callHash: onChain.callHash, recipient: '', amount: BigInt.zero)`—wiping the indexer-supplied recipient and amount while keeping `callData` non-null!
     - `multisig_action_confirm_sheet.dart:361-362` sets `blocked = _submitting || _loadingCallBytes || _callBytesFailed`. Because `_callBytesFailed` is only true when `tx.callData == null`, **Confirm Approval remains enabled** while the UI hides the recipient row and shows `0 QUAN`!
  2. **`MultiAddress::Index` Width Divergence (`()` 0-byte in runtime vs. `Compact<BigInt>` >=1-byte in Dart SDK, `#89808`, `#89987`):**
     - `runtime/src/lib.rs:134-135` defines `pub type Address = sp_runtime::MultiAddress<AccountId, ()>;` where variant `0x01` (`Index(())`) consumes **1 byte total**.
     - Generated `packages/quantus_sdk/lib/generated/schrodinger/types/sp_runtime/multiaddress/multi_address.dart:152-174` decodes `Index(CompactBigIntCodec.codec.decode(input))`, consuming `0x01` + a compact integer (`>=2 bytes`).
     - An attacker crafts an 88-byte `Utility.if_else(main, fallback)` call (`0x0105...`) with **zero trailing bytes**:
       - **On-chain runtime decode:** `main` is `Balances.transfer_keep_alive(dest: MultiAddress::Index(()), value: 33285...)`, which fails with `CannotLookup`; `Utility.if_else` then executes `fallback` = `Utility.batch_all([Balances.transfer_all(dest: Id(attacker), keep_alive: true), ...])`, draining 100% of the multisig's transferable balance (999.999 QUAN).
       - **Dart SDK decode (`CallDecoder.decodeBytes`):** Because `MultiAddress.Index` eats extra bytes, the boundary shifts so `main` decodes as a tiny `130 planck` (`0.00000000013 QUAN`) transfer to the attacker and `fallback` decodes as `Recovery.as_recovered(System.remark(...))` consuming all 88 bytes with `input.hasMore == false`!
  3. **`Utility.if_else` Primary-Only Summary & Spec-136 vs. Spec-143 Pallet Index Shift (`Vesting` = 22) (`#88914`, `#89151`, `#89798`, `#89806`, `#90351`):**
     - `CallDecoder._summarize` inspects only the primary branch of `Utility.if_else` (`if_else.main`), hiding a `Balances.transfer_all` fallback when `main` is set to fail on-chain (e.g., self-transfer or slightly over-balance transfer).
     - The generated SDK codecs come from spec 136 while the chain runtime is at spec 143 (which inserted `Vesting` at pallet index 22, shifting every subsequent pallet index by +1).

---

## 2. EAC / PermissionedResolver Role Scope Collapse, Schema Confusion & Transfer Survival

### Core Vulnerability Mechanics
ENSv2 uses EnhancedAccessControl (`EAC`) across both `PermissionedRegistry` (`UserRegistry`) and `PermissionedResolver`, where roles are stored per `(bytes32 resource, address account)` in `roles[resource][account]`. `ROOT_RESOURCE` is `bytes32(0)` (global across all names), while name-scoped resources are `keccak256(abi.encode(namehash(name), version))`. Admin role bits sit at `(role << 128)`. Frontend bugs arise when:
1. The UI groups or edits roles by `account` alone rather than `(account, resource)`.
2. An unresolvable `resource` hash defaults to `name = ''`, which the transaction builder interprets as `scope: 'root'`.
3. A `PermissionedResolver` address is loaded under the `/registry/$address/roles` route and interpreted using the `UserRegistry` role schema.
4. Root roles granted on a shared per-HCA `PermissionedResolver` during registration/migration are never revoked when a name NFT is transferred.

### Sub-Patterns & Exact Code Signatures

#### 2A. Account-Only Role Grouping Collapses Global + Name Scopes into a Single-Name Revocation
- **Representative Reports:** `#89253` (Crit), `#89293` (Crit), `#91095` (Crit), `#89413` (Crit), `#90814` (Crit), `#90967` (Crit), `#91005` (Crit), `#92089` (High), `#92746` (High), `#92796` (High), `#93005` (High).
- **Exact Code Pattern (`apps/portal`):**
  - `apps/portal/src/features/permissions/queries/resolverRoles.ts:179-220` (`groupRolesByAccount`) keys its `Map` solely by `row.account.toLowerCase()`, merging all `RoleEntry` items across `ROOT_RESOURCE` (`0x00..00`, resolved to `'(root)'`) and specific name resources (`'sub.alice.eth'`) via bitwise OR (`existing.roles | BigInt(row.roles)`).
  - `ResolverRolesTable.tsx:61` labels the merged row `"Global roles (all names)"` whenever `resolvedNames.includes('(root)')`.
  - When the owner clicks **Remove user** or unchecks a role in `ResolverRolesSidebar.tsx:167-169`, it picks a single target scope:
    ```ts
    const roleName = resolvedNames.find((n) => n !== '(root)') ?? ''
    ```
    If the account has **any** name-scoped role alongside root roles, `roleName` evaluates to that first non-root name (e.g., `'sub.alice.eth'`).
  - `revokeResolverRoles.ts:45-61` then sees `name !== ''` (`scope: 'name'`) and encodes a single `authorizeNameRoles(dnsEncode('sub.alice.eth'), mergedBitmap, account, false)` call (`0xbbd9abb5`)—never calling `revokeRootRoles` (`0x3b8f2885`) and never touching any second name resource! The attacker retains full `ROOT_RESOURCE` privileges (`ROLE_SET_ADDR`, `ROLE_SET_TEXT`, `ROLE_UPGRADE`) across all names on the resolver.

#### 2B. Unresolved Name Resource Silently Escalates Edit/Grant to `grantRootRoles` (`ROOT_RESOURCE`)
- **Representative Reports:** `#89483` (Crit), `#90843` (Crit), `#91062` (Crit), `#89476` (Crit), `#90707` (Crit), `#91032` (Crit), `#91908` (High), `#91944` (High), `#92313` (High), `#92774` (High).
- **Exact Code Pattern (`apps/portal`):**
  - `resolverRoles.ts:147-161` (`buildResourceToNameMap`) builds its lookup map only from `resolverData.nodes` currently pointing at the resolver on-chain, computing only `computeNameResource(node.name)` (`keccak256(namehash(name), 0)`).
  - A non-zero `resource` fails lookup (`resourceToName.get(resource) === undefined`, leaving `resolvedNames = []`) whenever:
    1. The delegate was granted a name-scoped role on `old.alice.eth` and that subname was later deleted or repointed to another resolver (`#89483`).
    2. The role was granted via record-specific `authorizeAddrRoles` / `authorizeTextRoles` (`keccak256(nameResource, coinType/keyHash)`) or has a non-zero 32-bit version mask (`#90707`, `#91032`).
  - In `ResolverRolesSidebar.tsx:169`, `resolvedNames.find((n) => n !== '(root)') ?? ''` returns `''`. At line 391, `{row.resolvedNames.length > 0 && (...)}` hides the scope caption completely.
  - When the owner checks one additional box (e.g., `Set Text`) and clicks **Save**, `grantResolverRoles.ts:45` sees `name === ''` -> `scope = 'root'` and encodes `PermissionedResolver.grantRootRoles(uint256,address)` (`0x072d5d77`), promoting the stale single-name delegate to **global root access across every name on the resolver**!

#### 2C. Cross-Contract Type Confusion (`PermissionedResolver` Loaded in `/registry/$address/roles`)
- **Representative Reports:** `#92112` (Crit), `#92771` (High).
- **Exact Code Pattern (`apps/portal`):**
  - Both `UserRegistry` and `PermissionedResolver` inherit `EnhancedAccessControl` and share identical function selectors `grantRootRoles(uint256,address)` (`0x072d5d77`) and `revokeRootRoles(uint256,address)` (`0x3b8f2885`), but their bit positions mean completely different things:
    - Bit 0 (`1 << 0`): `UserRegistry.ROLE_REGISTRAR` ("Registrar — Can register new labels") vs. `PermissionedResolver.ROLE_SET_ADDR` ("Set Address").
    - Bit 20 (`1 << 20`): `UserRegistry.ROLE_UPGRADE` vs. `PermissionedResolver.ROLE_UPGRADE`.
  - `apps/portal/src/routes/registry/$address/roles.tsx:31` renders the Registry Roles management UI for any `:address` without calling `useIsPermissionedResolver` (which checks the EIP-1967 implementation slot in `isPermissionedResolver.ts`).
  - Furthermore, an attacker can poison the Panoptes indexer into listing the victim's `PermissionedResolver` `P` as a `UserRegistry` simply by calling `ETHRegistrar.register(..., subregistry: P)` on any throwaway name (`ETHRegistrar` does not call `supportsInterface` on `subregistry` and emits `SubregistryUpdated(..., P)`, which `registry.ts:62` indexes as a registry).
  - When the victim opens `/registry/P/roles` and grants bit 0 ("Registrar") to a collaborator, the wallet calls `P.grantRootRoles(1, collaborator)` on their `PermissionedResolver`, granting global `ROLE_SET_ADDR` across every name using `P`.

#### 2D. Persistent Root Roles on Shared Per-HCA `PermissionedResolver` Surviving Name Transfer
- **Representative Reports:** `#89264` (Crit), `#89279` (Crit), `#92174` (Crit), `#90944` (Crit), `#90981` (Crit), `#91977` (High), `#92913` (High), `#93051` (High).
- **Exact Code Pattern (`apps/manager` + `apps/portal`):**
  - During registration (`apps/manager/src/features/registration/utils/registration-calls.ts:171-196`, Step 6) and V1->V2 migration (`buildWalletCoAdminCall.ts`), Manager deploys or reuses **one deterministic `PermissionedResolver` per HCA** (`DedicatedResolverFactory` with `DEFAULT_SALT = 0`) and calls `authorizeNameRoles(0x00, ROLES_ALL, walletAddress, true)`—granting the registrant's EOA root `ROLES_ALL` (`ROLE_SET_ADDR | ROLE_SET_TEXT | ROLE_UPGRADE | ...` plus all admin bits) on `ROOT_RESOURCE` (`0x00`).
  - When the owner transfers `name1.eth` (while keeping `name2.eth` on the same resolver, or leaving `Detach Resolver` unchecked in `SendNameForm.tsx`), `transferToken.ts` executes `ETHRegistry.safeTransferFrom`. While `PermissionedRegistry._update` regenerates the token's registry resource ID (wiping registry roles), it **never touches `PermissionedResolver`**.
  - Because the seller still holds `ROLES_ALL` on `ROOT_RESOURCE` of that `PermissionedResolver` (and in fact needs it for their remaining names `name2.eth`), the seller can immediately call `setAddr(namehash("name1.eth"), 60, attackerBytes)` or `upgradeToAndCall` on the resolver after selling `name1.eth` (`#89264`, `#92174`). Even more insidiously, if the seller transfers a parent name whose child `sub.parent.eth` still points to the seller's shared resolver, `SendNameForm`'s "Detach Resolver" only detaches the parent node's resolver and leaves `sub.parent.eth` bound to the seller's resolver (`#89279`).

---

## 3. V1-to-V2 Migration Stale State & Unverified Subgraph Authority

### Core Vulnerability Mechanics
In ENSv1, an unwrapped `.eth` second-level domain has two separate ownership records:
- **Registrant (`BaseRegistrar.ownerOf(uint256(labelhash))`)**: the ERC-721 token holder (updated on secondary-market sales like OpenSea or standard ERC-721 transfers).
- **Controller (`ENSRegistry.owner(namehash)`)**: the V1 registry manager who controls `resolver(node)` and subnames until the new ERC-721 holder explicitly calls `BaseRegistrar.reclaim(tokenId, buyer)`.

### Sub-Patterns & Exact Code Signatures

#### 3A. Unconditional Former-Owner Backdoor on Gifted/Purchased Unwrapped V1 `.eth` Names
- **Representative Reports:** `#89314` (Crit), `#89336` (Crit), `#89363` (Crit), `#89516` (Crit), `#90702` (Crit), `#90907` (Crit), `#92042` (High), `#92776` (High).
- **Exact Code Pattern (`apps/manager/src/features/migration`):**
  1. **Preflight ignores V1 controller/resolver:** `preflightChecks.ts:110-127` (`verify2LdState`) checks only `BaseRegistrar.ownerOf(tokenId) == migrationOwner`. It never checks whether `ENSRegistry.owner(node)` (controller) or `ENSRegistry.resolver(node)` still belongs to the former owner (`0xSeller`).
  2. **Classifier trusts unverified V1 subgraph controller & resolver:** `classifyNames.ts:84-87` sets `resolverPlan = isKnownPublicResolver(domain.resolver.address) ? 'migrate-records' : 'keep-v1'`. At `classifyNames.ts:137,162-166`:
     ```ts
     const ownerAddress = domain.owner.id
     const registrantAddress = domain.registrant?.id ?? null
     const hasSeparateManager =
       tokenType === 'unwrapped-2ld' &&
       registrantAddress !== null &&
       ownerAddress.toLowerCase() !== registrantAddress.toLowerCase()
     ```
     When true, `managerAddress` is set to `domain.owner.id` (`0xSeller`) and `requiresPostGrant` is set to `true` (`classifyNames.ts:346`).
  3. **Batch builder encodes attacker resolver + V2 `ROLE_SET_RESOLVER` grant:**
     - `buildAtomicMigrationBatches.ts:481-491` (`resolverFor`) sets `MigrationData.resolver = classified.currentResolver` (`0xSellerResolver`) whenever `resolverPlan === 'keep-v1'`.
     - `buildAtomicMigrationBatches.ts:608-614` (`buildRestoreManagerCalls`) appends:
       ```ts
       ETHRegistry.grantRoles(canonicalId, ROLE_SET_RESOLVER /* 1 << 24 */, classified.managerAddress /* 0xSeller */)
       ```
       inside the victim's `StandaloneHCA.executeByOwner` atomic batch!
  4. **UI Step Descriptor hides the address & Post-Verifier certifies the hijack:**
     - `buildStepDescriptors.ts:13-31` (`MigrationStepDescriptor`) has **no target address field**; `StepList.tsx` renders only `"Restore the existing managers for your names"`.
     - `verifyAtomicMigrationBatch.ts:67-76,184-199` checks that `ETHRegistry.hasRoles(canonicalId, ROLE_SET_RESOLVER, 0xSeller)` is `true` and marks the hijacked migration `"verified"`!
- **Impact:** After V1->V2 migration, the former V1 seller holds `ROLE_SET_RESOLVER` (`1 << 24`) on the victim's V2 `.eth` name in `ETHRegistry` and can overwrite the victim's V2 resolver at any time forever, plus their custom V1 resolver is installed directly into V2.

#### 3B. Stale Cached Migration Plan Across Focus Refresh (Query Key Missing Resolver Fingerprint)
- **Representative Report:** `#93130` (Med, resubmitted from closed report with full Playwright + deployed-fork Anvil proof).
- **Exact Code Pattern:**
  - `useMigrationGasEstimate` keys its TanStack Query solely by `['migration-gas-estimate', ownerAddress, hcaAddress, domainIds, selectionRevisionRef.current.revision]`, omitting `domain.resolver.address` and `domain.owner.id`.
  - Meanwhile, `useV1Names` passes `staleTime: 5 * 60 * 1000` into `resultQueryOptions`, which destructures `staleTime` without returning it—so `staleTime: 0` applies.
  - When a user repairs their V1 resolver from attacker resolver `A` to safe resolver `B` in another tab and refocuses the Manager tab, `useV1Names` refreshes to `B`, while `useMigrationGasEstimate` retains the same query key and keeps plan `A` in `status: 'ready'`. Clicking **Upgrade** submits `plan.classified` with resolver `A` into V2 without re-verifying live V1 state.

---

## 4. HCA / ERC-1271 Session Key Scope Bypass & Quote / Permit / Allowance Failures

### Core Vulnerability Mechanics
ENSv2 Manager uses Rhinestone Warp / Hybrid Smart Accounts (`StandaloneHCA`), ERC-7579 `ExperimentalSessionValidator`, EIP-2612 USDC `permit`, and off-chain orchestrator quotes (`/intents/route`). Vulnerabilities occur when:
1. A local patch to `@rhinestone/sdk` changes the signature mode of a scoped session from `0x0205` (execution-emissary validated) to `0x0201` (`SIG_MODE_ERC1271` owner mode) or `0x03` (`PermitBatchWitnessTransferFrom` validating only `destOps` while leaving `preClaimOps` unvalidated).
2. The frontend sizes an EIP-2612 `permit` or `transferFrom` directly from an untrusted orchestrator response (`quotedFeeUsdc`) without enforcing a local ceiling against the displayed price, or swallows a budget-fetch failure (`catch(() => null)`) so the UI renders base rent as an exact price while the state machine independently quotes and debits a much larger amount.

### Sub-Patterns & Exact Code Signatures

#### 4A. Patched SDK Downgrades Scoped HCA Session to `SIG_MODE_ERC1271` (`0x0201`) or Dest-Only Mode (`0x03`)
- **Representative Reports:** `#91014` (Crit), `#92413` (High), `#92665` (High), `#92779` (High), `#93046` (High).
- **Exact Code Patterns:**
  1. **Same-Chain Standalone HCA Downgraded from `0x0205` to `0x0201` (`#91014`):**
     - `packages/transaction-manager` configures `experimental_session` with `verifyExecutions: true`, which upstream `@rhinestone/sdk@1.8.0` encodes as session mode `0x0205` (requiring `IntentExecutor` -> `Paymaster.settleGasRefund_requireCallback()` so the emissary verifies target/selector policies).
     - The repository's patch (`patches/@rhinestone%2Fsdk@1.8.0.patch`) overrides same-chain standalone HCA calls to `SIG_MODE_ERC1271` (`0x0201`).
     - On-chain, `IntentExecutor` treats `0x0201` as a trusted owner signature and calls `Paymaster.settleGasRefund()` (no callback!), while `StandaloneHCA.isValidSignature` falls back to the enabled session when the owner check fails.
     - **Exploit:** Because every policy in the registration session (`getRegistrationSessionActions()`) includes an unconditional ERC-20 `approve(Permit2, ...)` execution alongside `MAX_REFUND_AMOUNT = 50_000_000` (50 USDC), anyone holding the scoped browser session key can submit repeated approval-only intents with `refundAmount = 50 USDC` and `refundRecipient = attacker`, draining 100% of the HCA's USDC balance without ever registering a name.
  2. **Missing `settlementLayers: ['INTENT_EXECUTOR']` + Mode `0x03` Unchecked `preClaimOps` (`#92413`):**
     - `packages/transaction-manager/src/core/transports/warp-transport.actor.ts:147-164` (`submitWarpTransaction`) calls `prepareTransaction` without pinning `settlementLayers: ['INTENT_EXECUTOR']`.
     - If the orchestrator returns `fundingMethod: 'PERMIT2'` with a malicious `preClaimOps` (`MockUSDC.transfer(attacker, 19_000_000)`) and keeps `destOps` as the allowed registration call, `@rhinestone/sdk` (`dist/src/orchestrator/utils.js:694-719`) builds a `PermitBatchWitnessTransferFrom` digest (`mode = 0x03`).
     - In `patches/@rhinestone%2Fsdk@1.8.0.patch:347-409` (`packExperimentalSessionSignatureForSingleChainIntent`), `operationData` encodes **only `destOps`** and omits `preClaimOps`.
     - On-chain, `ExperimentalSessionValidator` validates only `destOps` against `_checkRegistrationExecutions` (passing!), while `IntentExecutor.executePreClaimOpsWithPermit2Stub` (`0x00000000005c1378...`) executes the unvalidated `preClaimOps` on the HCA via `SigMode.ERC1271`, transferring 19 USDC from the HCA to the attacker!

#### 4B. Uncapped EIP-2612 Permit & Swallowed Budget Failure (`TokenPickerContent` vs. `registration.machine.ts`)
- **Representative Reports:** `#89329` (High), `#91648` (High), `#93021` (High), `#93053` (High), `#92310` (Med).
- **Exact Code Patterns (`apps/manager`):**
  1. **Uncapped Orchestrator `quotedFeeUsdc` Drains Full EOA USDC Balance on Owner-Signed Path (`#89329`, `#91648`):**
     - `registration.hca.actors.ts:122-141` (`readUsdcSpend`) parses `accountAccessList.unallocatedTokens` from the untrusted orchestrator quote as `quotedFeeUsdc`.
     - `hca-intent-funding.ts:234-269` (`planHcaIntentFunding`) sets `requiredHcaUsdc = rentUsdc + quotedFeeUsdc` and sizes the EIP-2612 `permit(wallet, HCA, shortfall)` and `transferFrom(wallet, HCA, shortfall)` directly to that sum, gated only by `walletBalance < input.value` (`:627`).
     - On `setPrimaryNameWithHca` (`setPrimaryName.ts:181-198`), the intent is **owner-signed** (not session-signed, so the session validator's 50 USDC `MAX_REFUND_AMOUNT` does not apply). A compromised/MITM orchestrator sets `quotedFeeUsdc = walletBalance`, and the user's single signature permits and drains 100% of their wallet's USDC (5,000 USDC demonstrated on Anvil).
  2. **Swallowed Budget Error Renders Base Rent as Exact While Debiting Full Relayer Fee (`#93021`):**
     - In `TokenPickerContent.tsx:229-233`:
       ```ts
       const budget = await queryClient
         .fetchQuery(budgetQueryOptions)
         .catch(() => null)
       ```
     - When `budgetQueryOptions` fails (`budget === null`), `walletDebitRaw` is `null` (`:238-249`), which skips the `InsufficientFundingError` guard (`:298-307`), falls back `displayTotal` to `pricingData` (`$8.00` base rent at `:438`), and sets `isEstimate={!!funding}` to `false` at `<PaymentTotalRow isEstimate={!!funding} total={displayTotal} />` (`:575`).
     - The user sees `"Total $8.00 USD"` (without even the `"up to"` estimate prefix) and clicks **Register**, whereupon `registration.machine.ts:721-725` runs its own independent `fetchHcaRegistrationBudget` actor and prompts a single EIP-2612 permit + intent signature that debits `20.20 USDC` (`registration.hca.actors.ts:839-843`).

---

## 5. Multi-Step State Machine Race Conditions, Stale Closures & Actor ID Collisions

### Core Vulnerability Mechanics
Modern Web3 dApps use XState actors (`@xstate/react`), TanStack Query mutations, and custom `TransactionManager` registries to coordinate multi-step flows (e.g., 4-step Name Transfer: `set-eth-addr` -> `detach-resolver` -> `detach-registry` -> `transfer-token`; 2-step Registration: `commit` -> `wait` -> `register`; Sidebar Role Editing). Bugs occur when:
1. Transaction actor IDs are static (`tx-revoke-roles`) or deterministic per `(name, step)` without evicting completed actors from the global `TransactionManager` map.
2. Sidebar or table state (`editedPermissions`) is keyed by positional row index (`"0"`, `"1"`) instead of a stable account/resource key.
3. A singleton state machine (`registrationMachine`) is mounted without keying to the route's `?name=` parameter.

### Sub-Patterns & Exact Code Signatures

#### 5A. Static / Non-Evicted `TransactionManager` Actor IDs Short-Circuit Subsequent Transactions or Skip Cleanup Steps
- **Representative Reports:** `#89431` (Crit), `#89433` (Crit), `#92452` (Med).
- **Exact Code Patterns (`apps/portal` + `packages/transaction-manager`):**
  1. **Static `tx-revoke-roles` ID Blocks All Subsequent Role Revocations (`#89431`):**
     - `packages/transaction-manager/src/core/transactionManager.ts:290-340` (`createTransaction`) returns the existing actor from `this.transactions.get(options.id)` whenever that ID is already present, and completed actors are never evicted.
     - `TransactionModal.tsx:54-65` calls `transactionManager.remove(id)` **only** when `txState.error` is truthy.
     - `buildRoleTransactionDescriptors.ts:12-13` hardcodes `id: 'tx-revoke-roles'` for every role revocation across the entire Portal app.
     - After revoking User A once (`status === 'success'`), clicking **Remove user** on malicious User B opens `TransactionModal`, which immediately reads the cached `'success'` state for `'tx-revoke-roles'`, renders **Done** with User A's transaction hash, and never invokes `onStart` / `revokeRoles()`—leaving User B with full on-chain permissions while telling the owner the removal succeeded!
  2. **Deterministic `transfer-${name}-${step}` IDs Without `clear()` Skip Privacy/Authority Cleanup Steps on Retry (`#89433`):**
     - `apps/portal/src/features/transfer/hooks/useTransferName.ts:116-120` resets `startedStepsRef.current = new Set()` in `prepareMutation.onSuccess`, but never calls `transactionManager.clear()` or removes previous `transfer-${name}-${step}` actors.
     - If Attempt 1 completes Step 1 (`transfer-vitalik.eth-set-eth-addr` -> `'success'`) and the user closes the modal (or fails on `transfer-token` and retries with a new recipient or with `Detach Resolver` / `Detach Subnames` newly checked), Attempt 2's `getActiveTransaction.ts:16-21` sees Step 1 is already `'success'` and either jumps straight to `transfer-token` or auto-advances (`useAutoAdvanceTransaction.ts:20-28`) past the cleanup steps—transferring the ERC-1155 NFT while leaving the victim's `eth` payout address, `PermissionedResolver`, and `UserRegistry` attached!

#### 5B. Positional Table Row IDs (`"0"`, `"1"`) Cause Stale Sidebar State to Grant `ROLE_UPGRADE` to Collaborators
- **Representative Report:** `#90931` (Crit).
- **Exact Code Pattern (`apps/portal`):**
  - `apps/portal/src/components/ui/DataTable.tsx:33-47` calls `useReactTable({ data, columns, getCoreRowModel: getCoreRowModel() })` without passing `getRowId`. TanStack Table therefore assigns positional index strings `"0"`, `"1"`, `"2"`.
  - `apps/portal/src/features/permissions/components/ResolverRolesSidebar.tsx:155-159` synchronizes `editedPermissions` with the selected row using:
    ```ts
    if (row && row.id !== prevRowId) {
      setPrevRowId(row.id)
      setEditedPermissions(row.roles)
    }
    ```
  - **Exploit Chain:** Row `"0"` is a trusted admin (`0xAlice`) with `ROLE_SET_TEXT | ROLE_UPGRADE` (`0x100004`). Row `"1"` is a low-privilege collaborator (`0xMallory`) with `ROLE_SET_TEXT` (`0x4`). The owner opens Row `"0"` (`prevRowId = "0"`, `editedPermissions = 0x100004`) and removes `0xAlice`. When `rolesQuery` refetches, `0xMallory` shifts from index `"1"` into index `"0"`. The owner clicks `0xMallory` (now Row `"0"`) to grant `ROLE_SET_ABI` (`0x20`). Because `row.id === "0" === prevRowId`, `ResolverRolesSidebar` **does not reset `editedPermissions`**, leaving `ROLE_UPGRADE` (`0x100000`) checked in state! Clicking **Save** diffs `editedPermissions` (`0x100024`) against `0xMallory`'s actual `row.roles` (`0x4`) and calls `grantRootRoles(ROLE_SET_ABI | ROLE_UPGRADE, 0xMallory)` behind a generic modal titled `Update roles for 0xMallory`, letting `0xMallory` call `PermissionedResolver.upgradeToAndCall(maliciousImpl, "0x")` and hijack every name on the shared resolver.

#### 5C. Unkeyed Registration State Machine Retains Previous Name's Context Across Route Navigation
- **Representative Report:** `#92461` (High).
- **Exact Code Pattern (`apps/manager`):**
  - `useRegistrationTransactions.ts` instantiates `useActorRef(registrationMachine, { input: { chainId } })` without keying the actor to the route's `name` parameter.
  - If a user starts registering a 4-character name (`qzm2.eth`, `$160.11`), closes the transaction modal before signing Step 2, and navigates to `/register?name=low-price-mturyzm2.eth` (`$8.01`), the page header and total display `low-price-mturyzm2.eth` (`Total $8.01`), while clicking **Register** resumes the retained `registrationMachine` context for `qzm2.eth` and debits `160.109598 USDC`.

---

## 6. Off-Chain Nodes, Mobile Wallets, POS Screens, Workers, Archivers & RPC Infrastructure

### Sub-Patterns & Exact Code Signatures

#### 6A. Mobile Wallet Lock Screen & Signing/Discovery Bypasses (Quantus)
- **Representative Reports:** `#89007` (Crit), `#89802` (Crit), `#89999` (Crit), `#90460` (High).
- **Exact Code Patterns (`quantus-apps/mobile-app`):**
  1. **Missing `BlockSemantics` on Lock-Screen Overlay + Unauthenticated Wormhole Claim (`#89007`):**
     - `mobile-app/lib/app.dart:48-49` renders the lock overlay inside `MaterialApp.builder` as `Stack(children: [child!, const AuthWrapper()])` without wrapping `AuthWrapper` in `BlockSemantics` or excluding semantics on `child!`.
     - While Flutter's hit-test pipeline blocks raw touch events on `child!`, Android's `AccessibilityService` pipeline traverses the semantics tree of the underlying navigator (`SendScreen`, `RedeemAddressScreen`) and dispatches `SemanticsAction.tap` directly to background widgets!
     - Every standard transfer path gates signing behind `LocalAuthService().authenticate()`, **except** `redeem_address_screen.dart:82` (`_claimRewards()` -> `WormholeSendService.claimRewards()`), which reads the mnemonic directly from storage and submits the wormhole transfer while the lock screen is active!
  2. **Hot Wallet Trusts Unvalidated RPC Genesis Hash & Runtime Version for Signing (`#89802`):**
     - While the air-gapped Cold Wallet validates `genesisHash` against `knownNetworks` (`sign_transaction_screen.dart:48`), the main hot wallet's `SubstrateService` (`substrate_service.dart:214-255,365-371`) fetches `chain_getBlockHash(0)` and `state_getRuntimeVersion` directly from the active HTTP RPC endpoint and signs extrinsics with them—allowing a MITM/compromised RPC to obtain signatures valid on any sibling Quantus network where the user shares the same mnemonic/address.
  3. **Multisig Discovery Trusts Unverified Indexer `record['id']` Without Recomputing `predictMultisigAddress` (`#89999`):**
     - On the creation path (`multisig_service.dart:105`), the app deterministically computes `predictMultisigAddress(signers, threshold, nonce)`.
     - On the discovery path (`multisig_service.dart:164-179`, `multisigAccountFromIndexerRecord`), it accepts `address: record['id'] as String` verbatim from the Subsquid GraphQL response without verifying that `record['id'] == predictMultisigAddress(...)`, letting a compromised indexer substitute an attacker EOA as the user's discovered multisig deposit address.
  4. **POS Screen Treats Pre-Execution Mempool Notification (`txWatch_transfer`) as Final Payment (`#90460`):**
     - `node/src/txwatch.rs` emits `txWatch_transfer` as soon as an extrinsic enters the ready pool (`import_notification_stream()`), before block inclusion or execution.
     - `mobile-app/lib/v2/screens/pos/pos_qr_screen.dart` permanently switches to `Payment Received` on receiving a matching `txWatch_transfer`, cancels the payment timeout, and has no handler for `usurped`/dropped transactions. A payer submits an invoice transfer with `nonce = N, tip = 0` (triggering `Payment Received` on the merchant's POS) and immediately broadcasts a replacement transfer to themselves with `nonce = N, tip > 0`, which usurps and evicts the invoice payment from the production transaction pool before block inclusion.

#### 6B. Cloudflare Worker SSRF / RPC Quota Exhaustion via Unguarded Viem CCIP-Read (`OffchainLookup`) & Cursor Pagination Drops
- **Representative Reports:** `#92723` (Med), `#92816` (Med).
- **Exact Code Patterns (`apps/portal/src/worker` & `workers/api-worker`):**
  1. **Missing `ccipRead: false` Bypassing `safe-fetch.ts` in Explorer OG Worker (`#92723`):**
     - `apps/portal/src/worker/safe-fetch.ts:74-93` hardens the Worker's avatar fetch (enforcing `https:`, rejecting IP literals, blocking `selfHost` recursion, capping redirects at 3 and timeout at 20s).
     - However, `apps/portal/src/worker/clients.ts:9-24` calls `createPublicClient({...})` without `ccipRead: false`. When an unauthenticated `GET /og/<name>.png` or `GET /<name>` resolves a name whose resolver reverts with ERC-3668 `OffchainLookup(..., urls, ...)`, `viem@2.52.2` (`_esm/utils/ccip.js:85-141`) loops over the entire `urls[]` array and issues raw, unguarded `fetch()` calls with no scheme check, no IP-literal check, no `selfHost` check, and no timeout. Pointing `urls[]` back at the Worker's own `/og/` route caused 1 unauthenticated GET to trigger **8,122 upstream `eth_call`s**, exhausting the Worker's metered RPC quota.
  2. **Non-Unique Timestamp Cursor Pagination (`expiry_gt: $cursor`) Dropping Tied Domains at Page Boundary (`#92816`):**
     - `workers/api-worker/src/services/expiry-discovery/indexer.ts:13-28` queries `domains(where: { expiry_gt: $cursor, expiry_lte: $upper_bound }, orderBy: expiryDate, orderDirection: asc, first: 1000)` with no secondary tie-breaker key (`id`), and `index.ts:136` sets `cursorEnd = lastExpiryDate`. Whenever a group of domains sharing an identical `expiryDate` (created naturally by bulk renewal or same-block registrations) straddles the 1,000th row, the next tick's strict `expiry_gt: cursorEnd` permanently and silently skips all remaining domains at that timestamp across all future cron runs.

#### 6C. Shardeum Archiver & Validator Node Attacks (Unverified Receipt Gossip, Signature/Shard Split, SQLi, `safeStringify` OOM)
- **Representative Reports:** `#34508` (Crit), `#40004` (Crit), `#39434` (Crit), `#35903` (High).
- **Exact Code Patterns:**
  1. **Unverified `/gossip-data` Missing-Receipt Fetch Overwrites Global Network Account (`#34508`):** Any node can join as an active archiver, send `/gossip-data` with a fake `RECEIPT` `txId`, and when victim archivers call `collectMissingReceipts` -> `/receipt`, return a receipt with `isGlobal: true, globalModification: true` (`Collector.ts` skips `verifyReceiptData` when `globalModification === true`!) to overwrite the network global configuration account (`1000...0001`) in SQLite.
  2. **Decoupled Signature-Count vs. Shard-Membership Checks (`#40004`):** In `src/Data/Collector.ts`, `verifyAppliedReceiptSignatures` (`:823`) keys `goodSignatures` Map by case-sensitive `signature.owner` and verifies cryptographic signatures **without** checking whether `signature.owner` belongs to the execution shard; separately, `verifyReceiptData` (`:574`) checks whether `signature.owner` is in the execution group **without** verifying the cryptographic signature! An attacker with 1 node sends 1 valid signature duplicated with uppercase/lowercase hex variations of their pubkey (passing the signature count threshold) plus fake signatures carrying honest execution-shard node pubkeys (passing the shard membership loop), forging arbitrary account balances across all archivers.
  3. **Dynamic Column-Key SQL Injection in `bulkInsertReceipts` / `bulkInsertOriginalTxsData` (`#35903`):** `src/dbstore/receipts.ts` builds `const fields = Object.keys(receipts[0]).join(', ')` directly from the JSON object returned by a peer archiver and interpolates `fields` into `INSERT OR REPLACE INTO receipts (${fields}) VALUES (...)`.
  4. **Pre-Auth OOM via `@shardus/types` `safeStringify` Buffer Allocation (`#39434`):** `stringify.ts:122` checks `val['dataType'] === 'bb'` and calls `Buffer.from(val['data'])`. Passing `{ data: { length: 1e10 }, dataType: 'bb' }` in any request body or query parameter that gets logged or signed immediately allocates a 10 GB buffer and crashes the archiver process.

#### 6D. Zano Trade SSR Host-Header Config Poisoning, DoS & Wallet RPC JWT Salt Unit Mismatch
- **Representative Reports:** `#47740` (Crit), `#48436` (Crit), `#40530` (High).
- **Exact Code Patterns:**
  1. **Next.js `_app.tsx` `getInitialProps` Host-Header SSRF & `__NEXT_DATA__` Config Poisoning (`#47740`):** `src/pages/_app.tsx:61-83` runs `const host = context.ctx.req?.headers.host` and fetches `http://${host}/api/config/` server-side, embedding the response into `__NEXT_DATA__.props.config`. Supplying `Host: attacker.com` causes the SSR server to fetch attacker-controlled runtime configuration (or probe internal `127.0.0.1` ports).
  2. **Unbounded Scientific Notation in `Decimal.toFixed()` (`#48436`):** `orders.controller.ts` validates price/amount strings with `!Number.isNaN(+value)` and passes `"1e10000"` to `new Decimal(price).toFixed()`, freezing the Node.js event loop and crashing PostgreSQL `numeric` bounds.
  3. **Milliseconds vs. Seconds Unit Mismatch in `wallet_rpc_server::auth_http_request` (`#40530`):** `src/wallet/wallet_rpc_server.cpp` computes `uint64_t ticks_now = epee::misc_utils::get_tick_count();` (which returns **milliseconds**) and calls `m_jwt_used_salts.add(salt, ticks_now + JWT_TOKEN_EXPIRATION_MAXIMUM)` where `JWT_TOKEN_EXPIRATION_MAXIMUM = 60 * 60` (**seconds**, i.e., `3600`). Used JWT salts are therefore purged after **3.6 seconds** instead of 1 hour, re-enabling captured JWT replay against the wallet RPC.

---

# PART B — Severity Escalation, Triage & Winning Immunefi Report Engineering

## 1. Why Findings Get Graded Critical vs. High vs. Medium vs. Downgraded to Low/Insight

Across the 461 Medium/High/Critical reports and 1,618 Low/Insight reports in the corpus, Immunefi triage applies **four strict boundary tests** to every submission:

| Boundary Dimension | **Critical** | **High** | **Medium** | **Low / Insight (Downgrade Zone)** |
| :--- | :--- | :--- | :--- | :--- |
| **1. Asset / Authority Impact** | Direct loss/theft of user funds or NFTs (`#89278`, `#89456`); permanent/unconditional takeover of name resolution or V2 registry roles (`#89253`, `#89314`, `#90725`, `#90931`); draining HCA/wallet balances (`#89007`, `#91014`). | Severe fund/fee overcharge or theft requiring a specific transport/indexer/RPC failure or multi-step setup (`#89329`, `#92413`, `#93021`); UI-vs-calldata divergence on secondary routes where a primary route was already fixed or duplicated (`#91869`, `#91967`); POS pre-execution payment spoofing (`#90460`). | Server-side DoS / RPC quota exhaustion (`#92723`); silent background cron drops affecting notifications (`#92816`); stale cache plans requiring multi-tab user repair race (`#93130`); non-financial profile/state corruption with ≤1 click. | UI-only display glitches with no wrong transaction signed; self-inflicted races requiring user to edit an input mid-flight (`#91822`); off-chain media/avatar staleness where user can manually re-save (`#91233`); theoretical smart contract bugs relying on fake/admin-deployed mocks (`#57596`). |
| **2. Victim Interaction & Trust Model** | **0-click or normal 1-click flow** where every visible UI indicator looks completely legitimate (or warns of nothing) and the victim acts rationally (`#89270`, `#89278`, `#89360`, `#89395`). | Normal user flow combined with a realistic external condition (e.g., transient budget query failure in `#93021`, or closing and reopening a modal in `#92461`). | Up to 1 click of user interaction for non-sensitive user details (`#92816`), or a multi-tab race where the victim remains owner and can recover (`#93130`). | **"Significant user interaction"** (e.g., 5+ steps in `#91233`, or typing a wrong address, clicking submit, and racing the `Preparing...` spinner in `#91822`) or requiring the victim to ignore an explicit warning banner. |
| **3. Attacker Privilege Preconditions** | **Unprivileged attacker**: any external party who registers a name, sells/transfers a V1 name on a secondary market, or holds a low-privilege delegated role (`ROLE_REGISTRAR` in `#90725`). | Unprivileged attacker, or a compromised untrusted boundary that the architecture explicitly intends to constrain (e.g., Rhinestone orchestrator quote in `#89329`/`#92413`, Subsquid indexer in `#89999`). | Unprivileged external request (`GET /og/<name>.png` in `#92723`) or organic system state (`#92816`). | Requires **admin/owner/deployer compromise** (e.g., `#57596` where the PoC initializes `BelongCheckIn` with a `MaliciousEscrow` contract), or requires the user to attack themselves. |
| **4. PoC Fidelity & Boundary Honesty** | Exercises the **real in-scope UI/client and real/forked contracts** end-to-end, capturing the exact EIP-1193 wallet payload and proving the post-state on Anvil/runtime (`#89278`, `#89314`, `#91014`). | Exercises the real UI/service and real runtime/pool (e.g., `#90460` pumping `PosQrScreen` in Flutter + running `sc-transaction-pool` in Rust). | Exercises the real unmodified worker/page against a local mock upstream with measured metrics (`#92723`, `#92816`, `#93130`). | Calls internal helper functions with hand-crafted invalid inputs unreachable from the UI, or uses `vm.etch`/mock contracts that contradict production token behavior (`#57596`). |

### Concrete Case Studies of Downgrades vs. Upgrades from the Corpus

1. **Why `#57596` ("Critical" Reentrancy Claim) Was Downgraded to Low:**
   - The reporter claimed Critical "Direct theft of any user funds" via reentrancy in `BelongCheckIn.distributePromoterPayments()` because `_storage.contracts.promoterToken.burn()` happens after `escrow.distributeVenueDeposit()` and `_swapUSDCtoLONG()`.
   - **Why Triage Downgraded It to Low:** In production, `usdc` and `long` are standard ERC-20 tokens without ERC-777 hooks, and `escrow` is a trusted protocol contract deployed by the admin. The reporter's PoC deployed a custom `MaliciousEscrow` contract and initialized `BelongCheckIn` with it. Replacing a core protocol contract requires privileged owner/admin keys, violating the unprivileged attacker requirement.
2. **Why `#91822` (Portal Transfer Recipient Race) Was Graded Low While `#89278` / `#89433` Were Graded Critical:**
   - Both `#91822` and `#89278` end with `transferToken.ts` encoding `safeTransferFrom(victim, wrongRecipientOrToken, ...)`.
   - In `#91822`, the victim must first type the wrong address `A`, click **Transfer name**, and then—within the ~200ms RPC window while `getEnsTokenId` is in flight (`isPreparing === true`)—manually edit the input box to `B`. That is a self-inflicted UI race requiring user error followed by a narrow timing window (**Low**).
   - In `#89278`, an external attacker gifts `ALICE.eth` to the victim; when the victim transfers `ALICE.eth`, the UI displays `ALICE.eth` from start to finish while `getEnsTokenId(getLabel(name))` normalizes `"ALICE"` -> `"alice"` and steals the victim's canonical `alice.eth` (**Critical**).
   - In `#89433`, the victim aborts or fails a transfer and later starts a new transfer; stale `TransactionManager` actors (`transfer-${name}-${step}`) silently skip `set-eth-addr`, `detach-resolver`, and `detach-registry` without any mid-flight typing race (**Critical**).
3. **Why `#89314` / `#89336` (V1-to-V2 Migration Hijack) Were Critical While `#93130` (Stale Migration Plan) Was Medium:**
   - In `#89314` and `#89336`, **every** unwrapped V1 `.eth` name bought or gifted via standard ERC-721 `transferFrom` has `registrant = victim` and `controller = formerOwner`. When the victim runs the normal V1->V2 migration in Manager, `buildAtomicMigrationBatches` unconditionally grants V2 `ROLE_SET_RESOLVER` to `formerOwner` and preserves `formerOwner`'s custom resolver behind a step labeled `"Restore the existing managers for your names"` (**Critical**).
   - In `#93130`, the victim had already called `reclaim()`, then opened the Manager migration tab while resolver `A` was set, then switched to a second tab to repair the resolver to `B`, then refocused the Manager tab and clicked **Upgrade** before the query key invalidated. Furthermore, the victim remained the sole V2 owner (no `ROLE_SET_RESOLVER` granted to the attacker) and could immediately overwrite resolver `A` in V2 (**Medium**).

---

## 2. How to Escalate Raw Bug Patterns into Confirmed Medium / High / Critical Impacts

When you discover a raw code defect during an audit, **never stop at the first symptom**. Run this systematic 5-step escalation checklist before writing the report:

### Step 1: Escalate "UI Caching / State Bug" -> "Silent Privilege Escalation or Unintended Calldata Signing"
- **Don't report:** *"DataTable uses array index as `row.id`, causing `ResolverRolesSidebar` to show stale checkboxes after deleting a row (Low UI bug)."*
- **Escalation Path (`#90931` -> Critical):** Trace what happens when the user makes a benign 1-checkbox edit on the shifted row and clicks **Save**. Because `editedPermissions` retained `ROLE_UPGRADE` (`1 << 20`) from the deleted row `"0"` while `row.roles` belongs to the new row `"0"` (a low-privilege collaborator), the diff engine (`editedPermissions & ~row.roles`) silently includes `ROLE_UPGRADE` in `grantRootRoles(bitmap, collaborator)`. Next, show that `ROLE_UPGRADE` on `PermissionedResolver` authorizes `upgradeToAndCall(maliciousImpl, "0x")`, compromising **every name sharing that resolver**.
- **Don't report:** *"Completed transactions are not removed from `TransactionManager`, so clicking Remove User a second time shows the old modal (Low UX bug)."*
- **Escalation Path (`#89431` -> Critical, `#89433` -> Critical):** Show that `buildRoleTransactionDescriptors` uses a global static ID `tx-revoke-roles` (so an owner revoking a compromised operator sees **Done** with a green checkmark while zero transactions are sent, leaving the attacker's on-chain roles intact), and show that `useTransferName` uses `transfer-${name}-${step}` (so a retried transfer skips `set-eth-addr`, `detach-resolver`, and `detach-registry` and transfers the NFT with the victim's payout address and resolver still attached).

### Step 2: Escalate "Parser / Normalization Inconsistency" -> "Full Asset Theft or Access-Control Bypass"
- **Don't report:** *"`delete-subname.helpers.ts` passes a string label instead of `subname.labelhash` to viem `labelhash()`, so bracketed subnames `[hex]` fail to delete properly (Low)."*
- **Escalation Path (`#90725` -> Critical):** Recognise that viem's `labelhash("[<64-hex>]")` strips the brackets and returns `0x<64-hex>` raw! Therefore, a delegated subname registrar holding only `ROLE_REGISTRAR` (`1 << 0`)—who is explicitly forbidden by the smart contract from calling `unregister` (`ROLE_UNREGISTER = 1 << 4`) or overwriting existing subnames—can register the literal spam label `[<hex(labelhash("valuable"))>].parent.eth`. When the parent owner clicks **Delete** on that spam row in Portal, the owner's wallet executes `unregister(labelhash("valuable"))`, deleting the real `valuable.parent.eth` so the `ROLE_REGISTRAR` delegate can immediately register `valuable.parent.eth` to themselves!

### Step 3: Escalate "Decoder Throw / Catch Fallback" -> "Blind Malicious Extrinsic Execution"
- **Don't report:** *"`CallDecoder.decodeBytes` throws `FormatException` when on-chain multisig call data has trailing zero bytes, causing the UI to show blank recipient and 0 QUAN (Low/Medium display issue)."*
- **Escalation Path (`#88483` -> High):** Check the confirmation button guard in `multisig_action_confirm_sheet.dart:361-362`: `blocked = _submitting || _loadingCallBytes || _callBytesFailed` where `_callBytesFailed` is only `tx.callData == null`. Because `MultisigService._withOnChainCallData` catches the `FormatException` and returns `tx.copyWith(recipient: '', amount: BigInt.zero)` **without** clearing `callData`, `_callBytesFailed` stays `false` and the **Confirm Approval** button remains active while hiding the recipient and displaying `0 QUAN`!
- **Even Stronger Escalation (`#89808` -> High):** Instead of relying on trailing bytes that trigger the catch block, find a **type-width divergence** (`MultiAddress::Index(())` = 1 byte in Rust runtime vs. `MultiAddress::Index(Compact<BigInt>)` >= 2 bytes in Dart SDK) so that an 88-byte `Utility.if_else` payload decodes with **zero trailing bytes and zero errors** in the Dart SDK as a 130-planck transfer, while the Rust runtime decodes the fallback `Balances.transfer_all` and drains 999.999 QUAN!

### Step 4: Escalate "Missing Validation on External / Off-Chain Data" -> "On-Chain Authority Injection or Uncapped Permit Drain"
- **Don't report:** *"V1-to-V2 migration reads `domain.owner.id` and `domain.resolver.address` from the subgraph without checking if the user called `reclaim()` (Medium)."*
- **Escalation Path (`#89314` / `#89336` -> Critical):** Trace the exact calldata built by `buildAtomicMigrationBatches.ts`: prove that `hasSeparateManager` causes the victim's own `StandaloneHCA.executeByOwner` batch to execute `ETHRegistry.grantRoles(canonicalId, ROLE_SET_RESOLVER, formerOwner)` on V2, and prove that `buildStepDescriptors.ts` omits the target address while `verifyAtomicMigrationBatch.ts` marks the attacker's role grant as `"verified"`.
- **Don't report:** *"`TokenPickerContent.tsx` catches budget query errors and falls back to `pricingData` (Low)."*
- **Escalation Path (`#93021` -> High):** Show that `budget === null` simultaneously (a) bypasses `InsufficientFundingError`, (b) sets `displayTotal = $8.00`, (c) flips `isEstimate={!!funding}` to `false` (removing the `"up to"` warning!), and (d) hands execution to `registration.machine.ts`, which re-runs `fetchHcaRegistrationBudget` independently and prompts an EIP-2612 `permit` that debits `20.20 USDC`.

### Step 5: Escalate "Mobile UI Overlay / Pre-Execution Notification" -> "Bypass of OS/Protocol Security Boundary"
- **Don't report:** *"`AuthWrapper` lock screen is rendered in a `Stack` without `BlockSemantics` (Low accessibility issue)."*
- **Escalation Path (`#89007` -> Critical):** Audit every transaction-signing path in the Flutter app to find which screen omits a secondary `LocalAuthService().authenticate()` call (`redeem_address_screen.dart:82` -> `WormholeSendService.claimRewards()`). Then write a Flutter widget test proving that while `AuthWrapper` is actively locking the screen and blocking all touch gestures, `tester.binding.pipelineOwner.semanticsOwner!.performAction(claimButtonSemanticsId, SemanticsAction.tap)` executes `_claimRewards()` and sweeps the wallet's funds.

---

## 3. Anatomy of an Undisputed Immunefi PoC & Submission That Survives Triage

Studying the resubmitted reports that overturned initial closures (`#90460`, `#93130`) and the cleanest first-try Critical/High acceptances (`#89278`, `#89360`, `#90725`, `#90931`, `#91014`, `#92723`) reveals the exact engineering rules that prevent triage pushback or severity downgrades:

### Rule 1: Map Explicitly to a Verbatim In-Scope Impact Category in the First 15 Lines
- State the exact **Target Asset** URL and the **verbatim Impact string** from the competition's scope table at the very top of the report.
- If the finding is a Low or Medium, **state its severity ceiling honestly** (as in `#91233` and `#92816` *"Upgrade-hunt: why this stays a Medium, not a Critical/High"* and `#92723` *"Boundaries of the claim"*). Triagers immediately trust a researcher who explicitly rules out non-reproducible overclaims (e.g., `#92723` testing and withdrawing an isolate OOM claim in favor of measured 8,122-call RPC amplification).

### Rule 2: Never Test a Mock When You Can Test the Production Pipeline (The `#90460` & `#93130` Resubmission Lesson)
- **Why initial reports get closed:**
  - In `#90460` (closed as `#89358`), the initial submission did not execute the Flutter `PosQrScreen` widget and did not prove remote mempool eviction through the production Substrate pool. The winning resubmission supplied **two runnable tests**: (A) a Flutter test pumping the real `PosQrScreen` over a local WebSocket and asserting `find.text('Payment Received')`, plus a negative control with the wrong amount; and (B) a Rust Cargo test (`sc-transaction-pool`) submitting two real ML-DSA-65-signed extrinsics as `TransactionSource::External` through the frozen Quantus runtime validator and asserting `usurped=(invoice_hash, replacement_hash)`.
  - In `#93130`, the initial submission failed triage (`Foundry 0/4`) because the triager ran `forge test` directly without fixture env vars and there was no browser execution capture. The winning resubmission made a **Playwright browser + Anvil Sepolia-fork test** the primary reproduction, asserting on the mined wallet transaction (`WALLET_CALL ... inputContainsA=true inputContainsB=false`), and provided a single self-contained `run.sh` that exports the fixture hashes and runs Vitest + Foundry offline.

### Rule 3: Always Include Both a Positive Exploit Assertion and a Control Assertion
- Every top-tier PoC in the corpus pairs the exploit run with a **control arm** in the same test file:
  - `#90460`: Test 1 proves `PosQrScreen` latches `Payment Received` on a matching `txWatch_transfer`; Test 2 proves `PosQrScreen` ignores a `txWatch_transfer` with a wrong amount.
  - `#92723`: Control arm (benign gateway) = `2 eth_calls, HTTP 200 in 1s`; Exploit arm (self-referential `/og/` gateway) = `8,122 eth_calls, quota exhausted`.
  - `#93130`: Stale-plan path encodes resolver `A` (`inputContainsA=true, inputContainsB=false`); fresh-control path encodes resolver `B`.

### Rule 4: Prove Both Sides of Any Displayed-vs-Signed Divergence
- Whenever reporting a UI/signer mismatch, your PoC must **simultaneously assert on**:
  1. **What the user sees in the DOM** (e.g., `expect(page.getByText('Transfer ALICE.eth')).toBeVisible()`, `expect(modalText).not.toContain(attackerAddress)`, or `getBoundingClientRect().width` pixel equality `181.23px === 181.23px` in `#89360`).
  2. **What the wallet actually signs or mines on-chain** (capturing the EIP-1193 `eth_sendTransaction` / `eth_signTypedData_v4` request parameters, decoding the ABI calldata selector and arguments, executing the transaction against Anvil/deployed contracts, and asserting the final on-chain owner/balance/role bitmask).

### Rule 5: Pre-Empt Known Issues, Duplicate Families & Reviewer Objections with a Comparison Table
- If the competition has a published **Known Issues** list or related bug families, include a dedicated **"Why This Is Not Known Issue X"** table (like `#92723`'s 4-row table contrasting `R2-04 (avatar text record -> safe-fetch.ts)` vs. `CCIP gateway (OffchainLookup revert -> viem internal fetch)`, or `#91233` and `#93130`'s known-issue boundary sections).
- Explicitly show:
  1. **Where the untrusted input enters** (different source/call site).
  2. **Why existing guards or known-issue patches do not reach it**.
  3. **Exact commit SHA, Node/pnpm/Foundry/Flutter versions, and copy-pasteable 3-step reproduction commands** from a clean `git clone` with zero manual file editing required.
