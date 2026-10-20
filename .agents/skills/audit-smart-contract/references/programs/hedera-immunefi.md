# Program scope: Hedera (Immunefi Blockchain/DLT bounty)

Use this file at Stage 0 (authorization and intake) and again at Stage 6 (severity).
Re-check the live program page before submitting: scope, impacts, and rules can change.

- **Program:** Hedera, Immunefi Blockchain/DLT bounty. Maximum bounty $30,000.
- **Live since:** 2025-02-05. **Last updated (as captured):** 2026-09-30.
- **Requires:** PoC, vault program, KYC.
- **Authorization basis:** published Immunefi program scope. Record the program
  link and the in-scope asset for each finding.

## In-scope assets (repositories)

| Repository | Notes |
| :--- | :--- |
| `hiero-ledger/hiero-consensus-node` | Hedera Services codebase (consensus, HTS, system contracts) |
| `hiero-ledger/hiero-mirror-node` | Hedera Mirror Node codebase |
| `hiero-ledger/hiero-json-rpc-relay` | JSON-RPC relay |
| `hiero-ledger/hiero-cryptography` | Cryptography codebase (last updated 2026-07-23) |

The scope page lists 8 assets in total. The four repositories above are the
ones that appear in the captured scope text. Confirm the rest on the live page
before treating any other repository as in scope.

## In-scope impacts (max payout per impact)

| Severity | Impact |
| :--- | :--- |
| Critical | Unintended permanent freezing of funds |
| Critical | Network not being able to confirm new transactions (total network shutdown) |
| Critical | Direct theft of funds |
| Critical | Network partition caused outside of design parameters |
| Critical | Any material impact caused by tampering/manipulating Hashgraph history |
| High | Temporary freezing of network transactions by delaying one block by 500% or more of the average block time of the preceding 24 hours beyond standard difficulty adjustments |
| High | Preventing gossip of a transaction or multiple transactions |
| High | Reorganizing transaction history without direct theft of funds |
| High | Authorizing transactions without approval from signers/owners |
| Medium | Increasing network processing node resource consumption by at least 30% without brute force actions, compared to the preceding 24 hours |
| Medium | Shutdown of greater than or equal to 30% of network processing nodes without brute force actions, but does not shut down the network |
| Medium | Incorrect or missing records exported to mirror nodes (see the limit below) |

## Program rules that change severity or scope

1. **Feature flags.** If a feature flag nullifies the bug on mainnet, expect at
   least one severity level of downgrade or out-of-scope.
2. **Not active in production.** Bugs in functionality that is gated, not yet
   enabled, or depends on a future release are in scope, but may be downgraded
   by at least one level. Check the flag state on mainnet and record it.
3. **Network-wide impact only.** Impact on a single node, including a node in a
   reconnecting or transitional state, is out of scope. Show network-wide effect.
4. **Privileged attack vectors.** Attacks that need a Hedera-team account, an
   approval-gated account, a roster-admitted consensus node peer, or an
   authenticated gossip connection may be downgraded or out of scope. Treat these
   as privileged unless you can show otherwise.
5. **Attacker-induced action required.** A report with no attacker-induced action
   is out of scope.
6. **Mirror node records.** The Medium "incorrect or missing records exported to
   mirror nodes" impact is limited to consensus-critical or financially
   significant records (account balances, token balances, authoritative ledger
   state). Derived, informational, or statistical records are out of scope.
7. **Consensus shutdown.** The Critical "network cannot confirm transactions" impact
   applies only if the fix requires a manual rollback. Otherwise it may be
   downgraded by at least one level.
8. **Economic viability.** Severity is calibrated to realistic attacker cost and
   realizable gain. Attacks with no net gain, or with only negligible value at
   risk (for example, transaction fees only), may be downgraded or out of scope.
   Include the cost and realizable-gain analysis in the report.

## Out of scope (program-wide and program-specific)

- Oracle data errors. Oracle manipulation and flash-loan attacks are not excluded.
- Basic economic and governance attacks (for example, a 2/3 attack).
- Lack of liquidity, Sybil attacks, centralization risks.
- Impacts that need a malicious or compromised node.
- Attacks that need leaked keys or credentials, or privileged addresses without
  added modifications.
- Impacts from attacks the reporter already exploited themselves, causing damage.
- Depegs of external stablecoins not caused by a bug in code.
- Secrets in GitHub without proof they are in use in production.
- Best-practice recommendations, feature requests, and impacts on test or
  configuration files unless the program says otherwise.
- Phishing or social engineering against employees or customers.
- Sensitive information leakage. Public keys are excluded from this scope. Private
  keys and credentials are in scope.

## Example starting points from the program

Ability to execute system commands; signing transactions for other users;
redirection of deposits or withdrawals; tampering with hashgraph history or with
submitted transactions; authorizing transactions without required signers;
preventing consensus or gossip of transactions; crashes of an in-scope service
(non-network DoS); RCE; probabilistic or reconnected-node consensus failures;
non-bandwidth DDoS (transaction hammering); HTS misuse through system contracts;
system smart contract modifiers not respected; economic-system fraud (for example,
avoiding fees to full nodes); preventing a node from accessing the network;
incorrect mirror-node records; incorrect transaction fees; unauthorized HTS
activity; over- or underpayment of staking rewards; theft of unpaid staking rewards.

## How to use this in the pipeline

- **Stage 0:** record the program link, the repository and commit SHA, and that
  the target is a published bounty asset. Stop and ask if the commit or asset is
  not in the scope list above.
- **Stage 1 threat model:** tag each actor as attacker, privileged (Hedera team,
  approval-gated, roster peer, authenticated gossip), or node. Only unprivileged
  or network-level attacker actors support a submittable finding.
- **Stage 5 PoC:** show network-wide effect. Single-node reproduction is out of
  scope. Run the PoC on a local network or devnet only. Never test against Hedera
  mainnet.
- **Stage 6 severity:** check each row above and record any feature flag, activity
  state, privilege requirement, and economic-viability analysis that applies.
