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
  - Constructs atomic transactions with priority compute budgets and Jito tip transfers, broadcast to 6 regional block engines in parallel.
  - Optional dual-routing (`DUAL_ROUTE_RPC=true`, the default) also sends the same signed transaction through public RPC `sendTransaction` for a higher landing rate. That copy is visible to the public network and can be front-run; set `DUAL_ROUTE_RPC=false` for Jito-only, private submission.
  - Tip policy: entries and normal exits pay `MIN_TIP_LAMPORTS`; panic exits and sell retries pay `MAX_TIP_LAMPORTS`. (`TipEngine::calculate_optimal_tip` implements an EV-based tip and is used by the backtesters, not by live execution.)
- **Risk Management & Fast Exit Engine:**
  - **Automated Trailing Stop-Loss:** Once a position is up 30%, exits if price drops the configured percentage from its peak (e.g. 15%).
  - **Hard Stop-Loss:** Exits if price falls the configured percentage below entry (e.g. 20%).
  - **Automated Take-Profit:** Locks in gains when reaching target multiples (e.g. 2.5x).
  - **Dev Dump Exit:** Watches creator sells in the `Processed` state and sends a high-tip panic exit when the dev sells at least `DEV_DUMP_THRESHOLD_PCT` of the balance they held when we entered (taken from the create tx for snipes, fetched via RPC for copy-trades). If that balance is unknown, any dev sell triggers the exit.
  - **Capital Reservation:** Buys reserve capital before they are sent, so repeated whale signals on the same mint, or many signals at once, cannot exceed `MAX_POSITION_SOL` or open duplicate positions.
  - **Restart-Safe Positions:** Entry price, peak price, invested SOL and dev balance are saved to `POSITIONS_FILE`, so stops and take-profit survive a restart.

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
| `COPY_TRADE_AMOUNT_SOL`| SOL investment per copy-trade / snipe | `0.035` |
| `MAX_POSITION_SOL` | Max SOL committed across open + in-flight positions | `0.07` |
| `SLIPPAGE_BPS` | Slippage tolerance (basis points; copy-trades use at least 1500, snipes at least 2500) | `500` (5%) |
| `TRAILING_STOP_PCT` | Trailing stop-loss threshold (armed after +30%) | `0.15` (15%) |
| `HARD_STOP_LOSS_PCT` | Hard stop-loss below entry | `0.20` (20%) |
| `TAKE_PROFIT_PCT` | Take-profit price multiple | `2.5` (2.5x) |
| `DEV_DUMP_THRESHOLD_PCT` | Share of the dev's entry balance sold that triggers a panic exit | `0.20` (20%) |
| `MIN_WHALE_BUY_SOL` | Minimum SOL a target whale must spend (copy-trade, or its own buy inside a launch tx) | `0.60` |
| `DUAL_ROUTE_RPC` | Also send transactions via public RPC (`false` = Jito bundles only) | `true` |
| `POSITIONS_FILE` | Where open-position entry data is persisted | `data/positions.json` |
| `MAX_CURVE_CACHE_ENTRIES` | Bonding curves kept in memory before the cache is cleared | `50000` |
| `TARGET_WALLETS` | Comma-separated list of whale wallets (falls back to `data/smart_money_wallets.txt`) | *Configured in .env* |
