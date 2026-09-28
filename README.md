# Clockit ⚡
> Ultra-Low Latency Solana MEV, Copy-Trading & Sniping Engine for Pump.fun powered by **Triton Yellowstone gRPC** and **Jito Block Engine**.

---

## 🚀 Key Features

- **Triton Yellowstone gRPC Ingestion:**
  - Streams real-time `Processed` slot transactions and account state changes directly from Triton One's validator infrastructure with microsecond latency.
  - Authenticates via Triton's official `x-token` metadata header.
- **Nanosecond Parsing & Zero-Copy Architecture:**
  - **1-Cycle Discriminator Matching:** Uses precomputed 64-bit integer constants (`BUY_DISCRIMINATOR_U64`, `SELL_DISCRIMINATOR_U64`, `CREATE_DISCRIMINATOR_U64`) loaded into native CPU registers for single-cycle opcode checks.
  - **Zero-Copy POD Deserialization:** Bypasses standard Borsh overhead using unaligned pointer dereferencing (`std::ptr::read_unaligned`).
  - **Zero-Allocation String Views:** Parses token metadata directly over packet slices without heap allocations (`String`).
  - **SIMD Pubkey Filtering:** Accelerates account matching using x86_64 AVX2 instructions (`_mm256_cmpeq_epi8`, `_mm256_movemask_epi8`).
- **Dynamic Bonding Curve Math ($x \cdot y = k$):**
  - Accurately computes token output, SOL expenditure, and slippage tolerances with `u128` overflow protection.
  - Real-time graduation tracking (detecting curve completion at ~85 SOL for migration to PumpSwap/Raydium).
- **Atomic Jito MEV Bundles:**
  - Bypasses public mempools to prevent adversarial front-running.
  - Constructs atomic transactions with priority compute budgets and Jito tip transfers.
  - Dynamically calculates Jito tips based on Expected Value (EV) and competitor aggression.
- **Risk Management & Fast Exit Engine:**
  - **Automated Trailing Stop-Loss:** Continuously monitors high-water mark prices and exits if price drops by configured percentage (e.g. 15%).
  - **Automated Take-Profit:** Locks in gains when reaching target multiples (e.g. 2.5x).
  - **Dev Dump Front-Running:** Intercepts large developer sales in the `Processed` state and sends emergency high-tip Jito bundles to sequence exits before the dev crashes the reserves.

---

## 🛠️ Architecture

```
                    ┌───────────────────────────────┐
                    │    Triton Yellowstone gRPC    │
                    │   (Processed Slot Updates)    │
                    └───────────────┬───────────────┘
                                    │
                                    ▼
                    ┌───────────────────────────────┐
                    │  Zero-Copy / SIMD Ingestion   │
                    │ (64-Bit Regs & AVX2 Filters)  │
                    └───────────────┬───────────────┘
                                    │
                        ┌───────────┴───────────┐
                        │                       │
                        ▼                       ▼
            ┌───────────────────────┐  ┌───────────────────────┐
            │   Whale Trade Signal  │  │ Real-Time Curve State │
            │    (Buy/Sell Detect)  │  │   (x*y=k Reserves)    │
            └───────────┬───────────┘  └───────────┬───────────┘
                        │                          │
                        └───────────┬──────────────┘
                                    │
                                    ▼
                    ┌───────────────────────────────┐
                    │    Decoupled Tokio Workers    │
                    │  (Dynamic EV Tip Calculation) │
                    └───────────────┬───────────────┘
                                    │
                                    ▼
                    ┌───────────────────────────────┐
                    │     Jito Block Engine POST    │
                    │ (Atomic Bundle Landed in Slot)│
                    └───────────────────────────────┘
```

---

## 📋 Getting Started with Triton Yellowstone

### 1. Obtain Your Triton One Access
1. Sign up on the Triton Customer Portal at [customers.triton.one](https://customers.triton.one).
2. Grab your dedicated or shared **gRPC Endpoint** (e.g., `https://solana-grpc.triton.one:443`).
3. Copy your **Secret Token** (this authenticates the `x-token` gRPC header).

### 2. Configure Environment (`.env`)
Copy the example file and enter your Triton credentials:
```bash
cp .env.example .env
```
Edit `.env`:
```env
YELLOWSTONE_GRPC_URL=https://solana-grpc.triton.one:443
YELLOWSTONE_X_TOKEN=YOUR_TRITON_SECRET_TOKEN_HERE
KEYPAIR_PATH=/home/rootkit/.config/solana/id.json
COPY_TRADE_AMOUNT_SOL=0.2
```

### 3. Build & Run in Release Mode
```bash
# Compile optimized release binary with native CPU extensions
cargo build --release

# Run Clockit
cargo run --release
```

---

## ⚙️ Configuration Reference

| Variable | Description | Default |
| :--- | :--- | :--- |
| `YELLOWSTONE_GRPC_URL` | Triton Yellowstone gRPC endpoint | `https://solana-grpc.triton.one:443` |
| `YELLOWSTONE_X_TOKEN` | Triton authentication secret token (`x-token`) | *None* |
| `SOLANA_RPC_URL` | Standard RPC for blockhash caching | Helius / Mainnet |
| `KEYPAIR_PATH` | Path to local Solana trader keypair | `~/.config/solana/id.json` |
| `JITO_BLOCK_ENGINE_URL`| Regional Jito Block Engine endpoint | `mainnet.block-engine.jito.wtf` |
| `MIN_TIP_LAMPORTS` | Minimum Jito tip (lamports) | `100_000` (0.0001 SOL) |
| `MAX_TIP_LAMPORTS` | Maximum Jito tip cap (lamports) | `50_000_000` (0.05 SOL) |
| `TIP_AGGRESSION_FACTOR`| EV fraction allocated to tip | `0.65` (65%) |
| `COPY_TRADE_AMOUNT_SOL`| SOL investment per copy-trade | `0.2` |
| `SLIPPAGE_BPS` | Slippage tolerance (basis points) | `500` (5%) |
| `TRAILING_STOP_PCT` | Trailing stop-loss threshold | `0.15` (15%) |
| `TAKE_PROFIT_PCT` | Take-profit price multiple | `2.5` (2.5x) |
| `DEV_DUMP_THRESHOLD_PCT` | Dev holdings sold trigger for panic exit | `0.20` (20%) |
| `TARGET_WALLETS` | Comma-separated list of whale wallets | *Configured in .env* |
