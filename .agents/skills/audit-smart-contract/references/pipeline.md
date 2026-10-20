# Audit pipeline (chain-agnostic)

Each stage produces evidence that feeds the report. Stages can loop: a fuzzing
counterexample becomes a PoC; a failed PoC refines the threat model.

## Stage 0 — Intake & authorization

- Record target identity, chain + chain-id, commit or bytecode SHA-256.
- Record the authorization basis (own code / bounty scope + link / CTF /
  engagement). If unclear, **stop and ask**.
- If the target is a bounty program with a scope reference (for example
  `references/programs/hedera-immunefi.md`), check the asset and commit against
  its in-scope list and apply its severity rules.
- Choose the substrate: source repo build, fork of deployed state, or local
  devnet/testnet. Mainnet is never a broadcast target — see `safety.md`.

## Stage 1 — Recon & threat modeling

- Enumerate contracts/programs/modules, entrypoints, roles/authorities,
  upgradeability (proxies, program upgrade authority, migration paths).
- Map external calls, oracles, cross-program/cross-contract invocations, and all
  paths that move value or change authority.
- Identify trust boundaries and attacker-controlled inputs.
- Write an explicit threat model: assets, actors, assumptions, and the invariants
  that must hold. Everything downstream tests these invariants.

## Stage 2 — Static analysis

Run the chain's static tools (see `chains.md`). For each raw finding:

- Classify evidence tier (observation of a pattern vs. inference of exploitability).
- Suppress known false positives with a recorded reason, not silently.
- Promote plausible high/critical candidates to Stage 3/4 for confirmation.

A clean static run is **not** proof of safety; record it as coverage, not a verdict.

## Stage 3 — Formal verification

Pick properties worth proving: access control, arithmetic safety, solvency /
conservation-of-value invariants, state-machine legality, re-entrancy freedom.

- Encode each as a checkable property for the chain's prover (see `chains.md`).
- A **proof** closes the property for the bounded assumptions — record the bounds.
- A **counterexample** is a concrete input trace → carry it to Stage 5 as a PoC seed.
- Record what was assumed away (unrolling bounds, external-call models, loop limits).

## Stage 4 — Fuzzing & dynamic analysis

- Write invariant/property tests and fuzz them (Echidna/Medusa, Anchor fuzz,
  custom harnesses) against a local fork or local validator.
- Prefer the project's own invariants; add adversarial ones from the threat model.
- A broken invariant with a shrunk input is a confirmed-candidate → Stage 5.
- Record campaign config (runs, seqLen, corpus) so coverage is reproducible.

## Stage 5 — Exploit PoC

For each confirmed high/critical candidate:

1. Spin up the execution substrate (local fork / local validator / devnet) via the
   sandbox manifest. **Run the mainnet guard on the endpoint first.**
2. Write a minimal, deterministic PoC that triggers the bug from an
   attacker-controlled position (no privileged cheats unless modeling a trusted
   actor, which must be stated).
3. Run it. Capture: the exact command, full trace/logs, pre/post state diff, and
   quantified impact (funds drained, invariant violated, authority seized).
4. Make it reproducible: pinned toolchain versions, fork block / slot, seeds.
5. If it cannot be made to fire, downgrade to **unconfirmed candidate** with the
   specific blocker.

## Stage 6 — Report assembly

- One finding record per issue (see `report/REPORT_TEMPLATE.md` and
  `schemas/finding.schema.json`).
- Severity = Immunefi class + CVSS-style vector + the quantified impact from the PoC.
- Include root cause at the producing boundary, not just the symptom.
- Include remediation and, where possible, a fixed-code sketch and a regression
  test that the PoC would now fail against.
- State coverage and limitations: what was examined, what was assumed, what
  remains unverified.
