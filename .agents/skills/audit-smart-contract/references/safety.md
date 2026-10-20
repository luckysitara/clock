# Safety model: authorization and the no-mainnet guard

These rules are structural, not advisory. If a step cannot satisfy them, stop.

## Authorization

Audit only a target the operator is authorized to test:

- their own code / infrastructure;
- a published bug-bounty scope (Immunefi, Code4rena, Sherlock, cantina, …) whose
  rules permit local PoC development — record the program link and in-scope asset;
- a CTF or training target;
- a written engagement.

Record the basis in the report intake. If the authorization is ambiguous or the
target looks like a third party's production system outside any program, **stop and
ask the operator** before analysis — never assume.

## The no-mainnet invariant

> Forking mainnet **state** into a local node is allowed and expected.
> Broadcasting a transaction to mainnet is forbidden.

Fork mode (`anvil --fork-url`, `vm.createSelectFork`, a forked localnet) copies
on-chain state into a **local** VM; all transactions execute locally and nothing is
submitted to the real network. That is the normal, safe PoC substrate.

The danger is submitting a real transaction to a mainnet endpoint. Forbidden
patterns include: `forge script --broadcast` / `cast send` against a mainnet RPC;
`solana ... --url mainnet-beta` sends; any signed tx to a mainnet chain-id/cluster;
and using a real funded key.

## Enforcement — the mainnet guard

Before any execution step that could broadcast, validate the endpoint and action:

```sh
python3 -I experiments/smart-contract-audit/safety/mainnet_guard.py \
  --action broadcast --endpoint http://127.0.0.1:8545 --chain-id 31337
```

The guard (`experiments/smart-contract-audit/safety/mainnet_guard.py`, config
`safety/allowlist.json`) enforces:

1. **RPC allowlist** — broadcast endpoints must be localhost or an explicitly
   allowlisted devnet/testnet. A mainnet RPC is permitted **only** as a read-only
   `fork-source`, never as a broadcast target.
2. **Chain-id / cluster denylist** — known mainnets (Ethereum 1, BSC 56, Polygon
   137, Arbitrum 42161, Optimism 10, Base 8453, Avalanche 43114, Solana
   `mainnet-beta`, …) are rejected for broadcast.
3. **Verb gating** — `broadcast` / `send` actions require a passing endpoint and
   chain-id; `read` / `fork-source` actions are allowed against mainnet state.
4. **No real keys** — refuse a broadcast configured with a non-ephemeral key.

The guard exits non-zero and prints the reason on any violation. Treat a non-zero
exit as a hard stop. Record the guard's decision as evidence for each execution
step, so "never touched mainnet" is provable, not asserted.

## Exploit handling

- PoCs demonstrate impact on the fork/testnet/devnet only. Do not retain or use
  live exploit infrastructure against real systems.
- Quantify impact from the local run (funds at risk, invariant broken); do not
  extrapolate to a real exploit that was never executed as if it had been.
- Follow responsible-disclosure norms for the relevant program; the report is a
  submission artifact, not a public release.
