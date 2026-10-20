# Per-chain toolchains & provisioning

All tools are installed **inside the disposable sandbox** from a declared manifest
(`experiments/smart-contract-audit/sandbox/*.manifest.json`), never on the host.
Validate a manifest against `schemas/sandbox-manifest.schema.json`. Pin versions.
Tear the sandbox down after the engagement.

Each chain family maps to: build/test harness · static · formal · fuzz · PoC/fork.

## EVM (Solidity / Vyper / Yul)

- **Build/test:** Foundry (`forge`, `cast`, `anvil`), Hardhat.
- **Fork/local:** `anvil --fork-url <state-source>` — executes locally; the fork
  URL may read mainnet **state**, but `anvil` is the only broadcast endpoint.
- **Static:** Slither, Mythril, Aderyn, Semgrep (smart-contract rulesets), 4naly3er.
- **Decompile/bytecode:** REA `inspect_evm_interface` (offline), Heimdall, Panoramix.
- **Formal/symbolic:** Halmos, hevm symbolic, SMTChecker (solc), Certora Prover.
- **Fuzz:** Echidna, Medusa, Foundry invariant tests.
- **PoC:** Foundry test forked at a pinned block; `vm.createSelectFork`.

## SVM (Solana / Anchor)

- **Build/test:** Anchor, `cargo build-sbf`, `solana` CLI, LiteSVM / BanksClient.
- **Local/dev:** `solana-test-validator`, surfpool, amman; **devnet/testnet**
  (airdrop ok). Cluster `mainnet-beta` is never a send target.
- **Static:** `cargo audit`, `cargo clippy`, Audit-Anchor, X-Ray / Sec3 patterns.
- **Formal:** Kani (Rust), property reasoning on instruction handlers.
- **Fuzz:** Trident, Anchor fuzz, custom BanksClient harnesses.
- **PoC:** program deployed to local validator / devnet; attacker tx sequence.

## Move (Aptos / Sui)

- **Build/test:** Aptos CLI / Sui CLI, `move` package manager.
- **Local:** `aptos node run-local-testnet`, Sui localnet; devnet/testnet faucet.
- **Static/lint:** Move lint, package analyzers.
- **Formal:** Move Prover (specs on module functions) — a first-class fit.
- **Fuzz/dynamic:** Move unit tests + adversarial scenarios on localnet.
- **PoC:** attacker module/script against localnet or devnet.

## CosmWasm (Cosmos)

- **Build/test:** `cargo`, `cosmwasm-std`, `cw-multi-test`.
- **Local:** wasmd local chain, testnet.
- **Static:** `cargo audit`, clippy, `cosmwasm-check` (both run by the `cosmwasm` stage).
- **Fuzz/dynamic:** `cw-multi-test` scenarios, property tests.
- **PoC:** message sequence on a local wasmd chain / testnet (generic `run_command`,
  e.g. `cargo test`, confirmed by exit code or an output marker).
- Manifest: `sandbox/cosmwasm.manifest.json` (rust base + wasm target + cargo-audit
  + cosmwasm-check).

## Other DLTs / L1 clients

- A blockchain **node/client binary** or custom VM is a **binary RE** target.
  Use REA's native stack (Ghidra/IDA/objdump/GDB/Rizin/Frida) via the
  `reverse-engineer-anything` skill, plus protocol/pcap tools for p2p/RPC
  surfaces. Feed findings into the same report and finding schema.
- For an unfamiliar chain, record it as a provider gap: add a manifest + a minimal
  static/dynamic path before claiming coverage. Do not assert coverage you did not run.

## Provisioning model

A manifest declares: chain family, required tools + pinned versions, the base
sandbox image, network policy (which RPC/clusters are reachable, enforced by the
mainnet guard), and teardown. Build → approve → use → destroy. Nothing persists on
the host; every installed tool and its version is recorded as evidence.
