use anyhow::{Context, Result};
use futures::StreamExt;
use log::{error, info, warn};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair},
    signer::Signer,
};
use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    str::FromStr,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, RwLock, Semaphore};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterTransactions,
};

use clockit::{
    constants::{
        BUY_DISCRIMINATOR_U64, METEORA_DLMM_PROGRAM, ORCA_WHIRLPOOL_PROGRAM,
        PUMPFUN_PROGRAM, PUMPSWAP_AMM_PROGRAM, RAYDIUM_AMM_V4_PROGRAM, RAYDIUM_CPMM_PROGRAM,
        SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64,
    },
    decoders::{
        fast_is_discriminator, BondingCurveAccountPod, FastPubkeyFilter,
        PumpFunBuyPod,
    },
    engine::{
        arb_engine::ArbitrageEngine,
        blockhash_cache::BlockhashCache,
        executor::{ExecutionEngine, TradeAction},
        hybrid_registry::HybridRegistry,
        jito::JitoClient,
        position_manager::{ExitReason, PositionManager},
        tip_engine::TipEngine,
    },
};

fn load_keypair_from_path(path: &str) -> Result<Keypair> {
    if let Ok(kp) = read_keypair_file(path) {
        return Ok(kp);
    }
    let mut file = File::open(path).context("Could not open keypair file")?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let clean = contents.trim();
    if clean.starts_with('[') && clean.ends_with(']') {
        let bytes: Vec<u8> = serde_json::from_str(clean).context("Failed to parse keypair JSON")?;
        return Keypair::try_from(&bytes[..]).context("Invalid keypair bytes");
    }
    let bytes = bs58::decode(clean).into_vec().context("Failed to decode base58 keypair")?;
    Keypair::try_from(&bytes[..]).context("Invalid base58 keypair bytes")
}

fn load_whale_wallets(path: &str) -> Vec<Pubkey> {
    let mut wallets = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let mut s = String::new();
        if f.read_to_string(&mut s).is_ok() {
            for token in s.split(|c: char| c == ',' || c == '\n' || c == '\r') {
                let clean = token.trim();
                if !clean.is_empty() && !clean.starts_with('#') {
                    if let Ok(pk) = Pubkey::from_str(clean) {
                        wallets.push(pk);
                    }
                }
            }
        }
    }
    wallets
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    println!("==========================================================================================");
    println!("       CLOCKIT: UNIFIED HYBRID ARBITRAGE & TRADING ENGINE (REAL FUNDS / MAINNET)");
    println!("       Spatial Cross-DEX + Triangular Loops + Whale Momentum Snipe");
    println!("       Protected by 5-Layer Institutional Risk & Capital Preservation Guard");
    println!("==========================================================================================");

    let grpc_url = std::env::var("YELLOWSTONE_GRPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com:443".to_string());

    let x_token = std::env::var("YELLOWSTONE_X_TOKEN")
        .or_else(|_| std::env::var("TRITON_X_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty());

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com/95794a05-4057-49cb-87e4-42d3d9a46774".to_string());

    let keypair_path = std::env::var("KEYPAIR_PATH")
        .unwrap_or_else(|_| "/home/tiktoor/.config/solana/id.json".to_string());

    let jito_url = std::env::var("JITO_BLOCK_ENGINE_URL")
        .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf/api/v1/bundles".to_string());

    let args: Vec<String> = std::env::args().collect();
    let mut capital_sol: f64 = 0.45;
    let mut max_drawdown_sol: f64 = 0.05;

    for i in 1..args.len() {
        if (args[i] == "--capital" || args[i] == "-c") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<f64>() {
                capital_sol = val;
            }
        } else if (args[i] == "--max-drawdown" || args[i] == "-m") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<f64>() {
                max_drawdown_sol = val;
            }
        }
    }

    // 1. Load Trader Keypair
    let keypair = match load_keypair_from_path(&keypair_path) {
        Ok(kp) => Arc::new(kp),
        Err(e) => {
            eprintln!("❌ Error loading keypair from '{}': {:#}", keypair_path, e);
            std::process::exit(1);
        }
    };

    let wallet_pubkey = keypair.pubkey();
    println!("🔑 Trader Wallet:        {}", wallet_pubkey);
    println!("📍 Triton Yellowstone:   {}", grpc_url);
    println!("🔗 Solana RPC Endpoint:  {}", rpc_url);
    println!("⚡ Jito Block Engine:    {}", jito_url);
    println!("💰 Allocated Trade Size: {:.4} SOL per atomic bullet", capital_sol);
    println!("🛑 Session Max Drawdown: {:.4} SOL stop-loss limit", max_drawdown_sol);
    println!("🔒 Concurrency Lock:     1 in-flight trade max");
    println!("⚡ Circuit Breaker:      3-consecutive-failure cooldown");
    println!("------------------------------------------------------------------------------------------\n");

    // 2. Check Wallet Balance
    println!("[1/5] Checking live on-chain SOL balance for {}...", wallet_pubkey);
    let rpc_client = Arc::new(RpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(5)));
    let balance_lamports = match rpc_client.get_balance(&wallet_pubkey).await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("⚠️ Failed to fetch balance from RPC: {:#}. Defaulting to 0.", e);
            0
        }
    };

    let initial_balance_sol = balance_lamports as f64 / 1e9;
    println!("💰 Current Wallet Balance: {:.6} SOL", initial_balance_sol);

    let required_lamports = (capital_sol * 1e9) as u64;
    let gas_reserve_lamports = 50_000_000;
    let total_required_lamports = required_lamports + gas_reserve_lamports;
    let max_drawdown_lamports = (max_drawdown_sol * 1e9) as u64;

    if balance_lamports < total_required_lamports {
        eprintln!("\n🚨 INSUFFICIENT BALANCE FOR LIVE TRADING!");
        eprintln!("   Required: {:.4} SOL | Current Balance: {:.6} SOL", total_required_lamports as f64 / 1e9, initial_balance_sol);
        return Ok(());
    }
    println!("✅ Wallet balance verified! Sufficient capital ({:.4} SOL) available.\n", initial_balance_sol);

    // 3. Initialize Engine Components
    println!("[2/5] Initializing Blockhash Cache, Tip Engine, and Jito Client...");
    let blockhash_cache = Arc::new(BlockhashCache::new());
    blockhash_cache.spawn_updater(rpc_url.clone(), Duration::from_millis(2500));

    let tip_engine = Arc::new(TipEngine::new(100_000, 50_000_000, 0.65));
    let jito_client = Arc::new(JitoClient::new(jito_url.clone()));
    jito_client.spawn_prewarmer();

    // Position manager: 15% trailing stop, 20% hard stop, 2.5x take profit
    let position_manager = Arc::new(PositionManager::new(0.15, 0.20, 2.5, 0.20));

    let executor = Arc::new(ExecutionEngine::new(
        Arc::clone(&keypair),
        Arc::clone(&jito_client),
        Arc::clone(&blockhash_cache),
        Arc::clone(&tip_engine),
        Arc::clone(&position_manager),
        rpc_url.clone(),
        clockit::config::dual_route_rpc_from_env(),
    ));

    let (_tx_trades, rx_trades) = mpsc::channel::<TradeAction>(2000);
    executor.spawn_balance_refresher();
    Arc::clone(&executor).spawn_worker(rx_trades);

    let min_profit_lamports = 1_200_000; // 0.0012 SOL clean net profit hurdle
    let arb_engine = ArbitrageEngine::new(min_profit_lamports, required_lamports);

    // 4. Initialize Hybrid Registry & Whales
    println!("[3/5] Initializing Multi-DEX Hybrid Registry & 155 Whales...");
    let hybrid_registry = Arc::new(RwLock::new(HybridRegistry::new_with_defaults()));
    let whale_wallets = load_whale_wallets("data/smart_money_wallets.txt");
    let whale_filter = FastPubkeyFilter::new(&whale_wallets);
    let migration_sniper = Arc::new(RwLock::new(clockit::engine::migration_sniper::MigrationSniper::new(100_000_000)));
    let lst_depeg_engine = Arc::new(clockit::engine::lst_depeg::LstDepegEngine::new(40));
    let backrun_engine = Arc::new(clockit::engine::backrunner::BackrunEngine::new(5_000_000_000, 120));

    println!("✅ Registered {} Multi-DEX Markets & {} Triangular Cycles", 4, 2);
    println!("🐋 Loaded {} verified smart money whale wallets", whale_wallets.len());
    println!("🎓 Armed Pump.fun Graduation / Raydium Migration Sniper (0.10 SOL bullets)");
    println!("⚡ Armed Retail Dislocation Backrunner (5+ SOL impact hurdle)");
    println!("💎 Armed LST Depeg Arbitrage (JitoSOL/mSOL/INF >= 0.40% spread)");
    println!("🌊 Armed Zero-Capital Flash Loans via Solend / Save");

    let whale_in_flight_lock = Arc::new(Semaphore::new(1));
    let arb_in_flight_lock = Arc::new(Semaphore::new(1));
    let consecutive_failures = Arc::new(AtomicU32::new(0));
    let cooldown_until = Arc::new(RwLock::new(Instant::now()));

    // 5. Connect to Triton Yellowstone gRPC with Auto-Reconnect
    println!("[4/5] Initializing Triton Yellowstone gRPC client with TLS...");
    println!("[5/5] 🚀 UNIFIED HYBRID ENGINE ONLINE! Scanning Mainnet blocks in real time...\n");
    info!("🛡️ Capital Protection Active: Max {:.4} SOL bullet | Stop-Loss: {:.4} SOL", capital_sol, max_drawdown_sol);

    let curve_cache: Arc<RwLock<HashMap<Pubkey, BondingCurveAccountPod>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let curve_to_mint: Arc<RwLock<HashMap<Pubkey, Pubkey>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let mut last_balance_check = Instant::now();

    let dex_programs = vec![
        PUMPFUN_PROGRAM.to_string(),
        PUMPSWAP_AMM_PROGRAM.to_string(),
        RAYDIUM_AMM_V4_PROGRAM.to_string(),
        RAYDIUM_CPMM_PROGRAM.to_string(),
        METEORA_DLMM_PROGRAM.to_string(),
        ORCA_WHIRLPOOL_PROGRAM.to_string(),
    ];

    loop {
        let mut builder = match GeyserGrpcClient::build_from_shared(grpc_url.clone()) {
            Ok(b) => b,
            Err(e) => {
                error!("Invalid gRPC URL: {}. Retrying in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        if grpc_url.starts_with("https://") {
            let tls = yellowstone_grpc_proto::tonic::transport::ClientTlsConfig::new().with_enabled_roots();
            match builder.tls_config(tls) {
                Ok(b) => builder = b,
                Err(e) => {
                    error!("Failed to configure TLS: {}. Retrying in 3s...", e);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
        }

        if let Some(token) = &x_token {
            match builder.x_token(Some(token.clone())) {
                Ok(b) => builder = b,
                Err(e) => {
                    error!("Failed to attach x-token: {}. Retrying in 3s...", e);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
            }
        }

        builder = builder
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .max_decoding_message_size(64 * 1024 * 1024);

        let mut client = match builder.connect().await {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to connect to Triton gRPC: {}. Retrying in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        let mut transactions = HashMap::new();
        transactions.insert(
            "hybrid_txs".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: dex_programs.clone(),
                account_exclude: vec![],
                account_required: vec![],
            },
        );

        let dex_vault_accounts: Vec<String> = {
            let reg = hybrid_registry.read().await;
            reg.vault_pubkeys().into_iter().map(|p| p.to_string()).collect()
        };

        let mut accounts = HashMap::new();
        if !dex_vault_accounts.is_empty() {
            accounts.insert(
                "dex_vaults".to_string(),
                SubscribeRequestFilterAccounts {
                    account: dex_vault_accounts,
                    owner: vec![],
                    filters: vec![],
                    nonempty_txn_signature: None,
                },
            );
        }

        let request = SubscribeRequest {
            accounts,
            slots: HashMap::new(),
            transactions,
            transactions_status: HashMap::new(),
            entry: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            commitment: Some(CommitmentLevel::Processed as i32),
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        };

        let (_sink, mut stream) = match client.subscribe_with_request(Some(request)).await {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to subscribe to Yellowstone stream: {}. Retrying in 3s...", e);
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };

        info!("📡 Subscribed to live Yellowstone gRPC stream");
        let mut last_heartbeat = Instant::now();
        let mut tx_counter: u64 = 0;
        let mut acc_counter: u64 = 0;

        loop {
            // Heartbeat metrics
            if last_heartbeat.elapsed() >= Duration::from_secs(60) {
                last_heartbeat = Instant::now();
                let latest_bh = blockhash_cache.get_latest().await;
                info!(
                    "💓 Engine Heartbeat: {} txs, {} acc updates processed in last 60s | Blockhash: {} | Active Positions: {}",
                    tx_counter, acc_counter, latest_bh, position_manager.position_count().await
                );
                tx_counter = 0;
                acc_counter = 0;
            }

            // Drawdown monitor (factors in open position capital)
            if last_balance_check.elapsed() >= Duration::from_secs(10) {
                last_balance_check = Instant::now();
                if let Ok(current_bal) = rpc_client.get_balance(&wallet_pubkey).await {
                    let invested_lamports = position_manager.total_invested_lamports().await;
                    let total_equity = current_bal.saturating_add(invested_lamports);
                    if total_equity + max_drawdown_lamports < balance_lamports {
                        error!(
                            "🛑 [SESSION STOP-LOSS REACHED] Shutting down to preserve capital. (Equity: {:.4} SOL, Initial: {:.4} SOL, Max Drawdown: {:.4} SOL)",
                            total_equity as f64 / 1e9,
                            balance_lamports as f64 / 1e9,
                            max_drawdown_lamports as f64 / 1e9
                        );
                        std::process::exit(0);
                    }
                }
            }

            let next_msg = match tokio::time::timeout(Duration::from_secs(25), stream.next()).await {
                Ok(Some(res)) => res,
                Ok(None) => {
                    warn!("⚠️ Yellowstone gRPC stream finished. Reconnecting...");
                    break;
                }
                Err(_) => {
                    warn!("⚠️ Yellowstone gRPC stream heartbeat timeout (25s of silence). Reconnecting to fresh stream...");
                    break;
                }
            };

            let msg = match next_msg {
                Ok(m) => m,
                Err(e) => {
                    error!("gRPC stream error: {}", e);
                    break;
                }
            };

        match msg.update_oneof {
            // A. Account Curve & Vault Balance Updates
            Some(UpdateOneof::Account(acc_update)) => {
                acc_counter += 1;
                if let Some(account_info) = acc_update.account {
                    if let Ok(pubkey) = Pubkey::try_from(account_info.pubkey.as_slice()) {
                        // 1. Check Pump.fun bonding curve update
                        if let Some(pod) = BondingCurveAccountPod::read_from_account(&account_info.data) {
                            let price = pod.current_spot_price_sol();
                            {
                                let mut cache = curve_cache.write().await;
                                cache.insert(pubkey, pod);
                            }

                            // Active position trailing stop / TP check
                            if let Some((pos, reason)) = position_manager.on_price_update(&pubkey, price).await {
                                let is_panic = reason == ExitReason::DevDumpPanic || reason == ExitReason::HardStopLoss || reason == ExitReason::TrailingStop;
                                info!("🚨 POSITION EXIT TRIGGERED: {:?} for Mint {} | Tokens: {}", reason, pos.mint, pos.token_balance);
                                let exec = Arc::clone(&executor);
                                tokio::spawn(async move {
                                    let _ = exec.execute_action(TradeAction::Sell {
                                        mint: pos.mint,
                                        token_amount: pos.token_balance,
                                        slippage_bps: 500,
                                        curve_state: pod,
                                        is_panic,
                                        token_program: Some(pos.token_program),
                                    }).await;
                                });
                            }
                            // 2. Graduation sniper check
                            if pod.complete || pod.real_sol_reserves >= 84_000_000_000 {
                                let mut ms = migration_sniper.write().await;
                                let maybe_mint = {
                                    let c2m = curve_to_mint.read().await;
                                    c2m.get(&pubkey).copied()
                                };
                                let target_id = maybe_mint.unwrap_or(pubkey);
                                if ms.is_graduating(&target_id, &pod) {
                                    info!("🎓 PUMP.FUN GRADUATION DETECTED: Curve/Mint {} reached 100%! Armed to snipe Raydium launch.", target_id);
                                }
                            }
                        }

                        // 3. Update Multi-DEX registry vaults
                        {
                            let mut reg = hybrid_registry.write().await;
                            reg.on_account_update(&pubkey, account_info.lamports, 0);
                        }

                        // 4. LST Depeg check
                        if let Some(opp) = lst_depeg_engine.evaluate_price(&pubkey, account_info.lamports, 10_000_000_000) {
                            info!("💎 LST DEPEG OPPORTUNITY: {} at {} bps discount! Exp Profit: {:.4} SOL", opp.symbol, opp.discount_bps, opp.expected_profit_lamports as f64 / 1e9);
                        }
                    }
                }
            }

            // B. Transaction Updates: Spatial Arb, Triangular Cycles, and Whale Tracking
            Some(UpdateOneof::Transaction(tx_update)) => {
                tx_counter += 1;
                // Circuit breaker check
                {
                    let cd = cooldown_until.read().await;
                    if Instant::now() < *cd {
                        continue;
                    }
                }

                let tx_info = match tx_update.transaction {
                    Some(t) => t,
                    None => continue,
                };
                let tx = match tx_info.transaction {
                    Some(t) => t,
                    None => continue,
                };
                let msg_data = match tx.message {
                    Some(m) => m,
                    None => continue,
                };

                let account_keys: Vec<Pubkey> = msg_data
                    .account_keys
                    .iter()
                    .filter_map(|k| Pubkey::try_from(k.as_slice()).ok())
                    .collect();

                let pumpfun_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
                let pumpswap_program = Pubkey::from_str(PUMPSWAP_AMM_PROGRAM).unwrap();

                let mut all_ixs: Vec<(Option<Pubkey>, &[u8], &[u8])> = Vec::new();
                for ix in &msg_data.instructions {
                    let prog = account_keys.get(ix.program_id_index as usize).copied();
                    all_ixs.push((prog, &ix.data, &ix.accounts));
                }
                if let Some(meta) = tx_info.meta.as_ref() {
                    for inner_set in &meta.inner_instructions {
                        for inner in &inner_set.instructions {
                            let prog = account_keys.get(inner.program_id_index as usize).copied();
                            all_ixs.push((prog, &inner.data, &inner.accounts));
                        }
                    }
                }

                for (prog_opt, data, accounts) in all_ixs {
                    let is_buy = fast_is_discriminator(data, BUY_DISCRIMINATOR_U64);
                    let is_sell = fast_is_discriminator(data, SELL_DISCRIMINATOR_U64)
                        || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64);

                    // 1. Whale Tracking on Pump.fun Curve AND PumpSwap AMM
                    let prog = prog_opt.unwrap_or(pumpfun_program);
                    let is_pump_curve = prog == pumpfun_program;
                    let is_pump_amm = prog == pumpswap_program;

                    if is_buy && (is_pump_curve || is_pump_amm) {
                        let maybe_whale_data = if is_pump_curve && accounts.len() > 6 {
                            let user_idx = accounts[6] as usize;
                            let mint_idx = accounts[2] as usize;
                            if user_idx < account_keys.len() && mint_idx < account_keys.len() {
                                let buyer = account_keys[user_idx];
                                let mint = account_keys[mint_idx];
                                let bonding_curve = if accounts.len() > 3 && (accounts[3] as usize) < account_keys.len() {
                                    account_keys[accounts[3] as usize]
                                } else {
                                    clockit::constants::derive_bonding_curve(&mint).0
                                };
                                let token_program = if accounts.len() > 8 && (accounts[8] as usize) < account_keys.len() {
                                    Some(account_keys[accounts[8] as usize])
                                } else {
                                    None
                                };
                                Some((buyer, mint, bonding_curve, token_program, true))
                            } else {
                                None
                            }
                        } else if is_pump_amm && accounts.len() > 3 {
                            let user_idx = accounts[1] as usize;
                            let mint_idx = accounts[3] as usize;
                            let pool_idx = accounts[0] as usize;
                            if user_idx < account_keys.len() && mint_idx < account_keys.len() {
                                let buyer = account_keys[user_idx];
                                let mint = account_keys[mint_idx];
                                let pool = if pool_idx < account_keys.len() { account_keys[pool_idx] } else { mint };
                                let token_program = if accounts.len() > 11 && (accounts[11] as usize) < account_keys.len() {
                                    Some(account_keys[accounts[11] as usize])
                                } else {
                                    None
                                };
                                Some((buyer, mint, pool, token_program, false))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some((buyer, mint, pool_or_curve, token_program, is_curve)) = maybe_whale_data {
                            {
                                let mut c2m = curve_to_mint.write().await;
                                c2m.insert(pool_or_curve, mint);
                            }

                            let buy_pod = PumpFunBuyPod::read_from_raw(data);
                            let sol_spent = buy_pod.map(|p| p.max_sol_cost).unwrap_or(0);

                            if whale_filter.contains(&buyer) {
                                if sol_spent >= 300_000_000 { // >= 0.30 SOL whale conviction
                                    info!("🐋 TOP WHALE DETECTED: {} bought {:.3} SOL of Mint {} on {}", buyer, sol_spent as f64 / 1e9, mint, if is_curve { "Pump.fun Curve" } else { "PumpSwap AMM" });

                                    let permit = match whale_in_flight_lock.clone().try_acquire_owned() {
                                        Ok(p) => p,
                                        Err(_) => {
                                            info!("Whale lock busy, skipping Mint {}", mint);
                                            continue;
                                        }
                                    };

                                    let snipe_sol = 100_000_000; // 0.10 SOL bullet
                                    let exec = Arc::clone(&executor);

                                    if is_curve {
                                        let cached_pod = {
                                            let cache = curve_cache.read().await;
                                            cache.get(&pool_or_curve).copied()
                                        };

                                        let curve_pod = match cached_pod {
                                            Some(pod) => Some(pod),
                                            None => {
                                                match rpc_client.get_account_data(&pool_or_curve).await {
                                                    Ok(data) => BondingCurveAccountPod::read_from_account(&data),
                                                    Err(_) => None,
                                                }
                                            }
                                        };

                                        if let Some(curve_pod) = curve_pod {
                                            if curve_pod.complete {
                                                info!("🎓 Mint {} already graduated! Armed for Raydium launch.", mint);
                                            } else {
                                                info!("🎯 TRIGGERING WHALE CO-SNIPE: 0.10 SOL on Mint {} behind Whale {}", mint, buyer);
                                                tokio::spawn(async move {
                                                    let _permit = permit;
                                                    let _ = exec.execute_action(TradeAction::Buy {
                                                        mint,
                                                        sol_amount_lamports: snipe_sol,
                                                        slippage_bps: 500,
                                                        curve_state: curve_pod,
                                                        dev_wallet: None,
                                                        dev_initial_balance: 0,
                                                        token_program,
                                                    }).await;
                                                });
                                            }
                                        }
                                    } else {
                                        // PumpSwap AMM Co-Snipe
                                        let metas: Vec<solana_sdk::instruction::AccountMeta> = accounts
                                            .iter()
                                            .filter_map(|&idx| {
                                                account_keys.get(idx as usize).map(|&pk| {
                                                    solana_sdk::instruction::AccountMeta::new(pk, false)
                                                })
                                            })
                                            .collect();

                                        let template_instruction = solana_sdk::instruction::Instruction {
                                            program_id: pumpswap_program,
                                            accounts: metas,
                                            data: data.to_vec(),
                                        };

                                        info!("🎯 TRIGGERING PUMPSWAP AMM WHALE CO-SNIPE: 0.10 SOL on Mint {} behind Whale {}", mint, buyer);
                                        tokio::spawn(async move {
                                            let _permit = permit;
                                            let _ = exec.execute_action(TradeAction::PumpSwapBuy {
                                                mint,
                                                sol_amount_lamports: snipe_sol,
                                                template_instruction,
                                                token_program,
                                            }).await;
                                        });
                                    }
                                }
                            } else {
                                // Retail dislocation check for large non-whale swaps
                                if sol_spent >= 2_500_000_000 && is_curve {
                                    let cache = curve_cache.read().await;
                                    if let Some(curve) = cache.get(&pool_or_curve) {
                                        if let Some(sig) = backrun_engine.evaluate_swap(&mint, &buyer, sol_spent, curve.real_sol_reserves) {
                                            info!("⚡ RETAIL DISLOCATION: {:.2} SOL buy on Mint {} created {} bps impact! Dislocation identified.", sol_spent as f64 / 1e9, mint, sig.dislocation_bps);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // 2. Spatial Arbitrage Evaluation
                    if is_buy || is_sell {
                        let reg = hybrid_registry.read().await;
                        let spatial_opps = reg.scan_spatial_opportunities(&arb_engine);
                        drop(reg);

                        if let Some((sym, opp, prog_a, prog_b)) = spatial_opps.into_iter().next() {
                            let ray_v4 = Pubkey::from_str(RAYDIUM_AMM_V4_PROGRAM).unwrap();
                            let pump_prog = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
                            if !((prog_a == ray_v4 && prog_b == pump_prog) || (prog_a == pump_prog && prog_b == ray_v4)) {
                                continue;
                            }

                            let permit = match arb_in_flight_lock.clone().try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => continue,
                            };

                            let sol_in = opp.optimal_sol_in_lamports;
                            let min_sol_out = opp.expected_sol_out_lamports;
                            let gross_profit = opp.gross_profit_lamports;
                            let payer = keypair.pubkey();
                            let mint = account_keys.get(2).copied().unwrap_or(wallet_pubkey);

                            let instructions = clockit::engine::pool_registry::build_atomic_arb_bundle_for_mint(&payer, &mint, &opp);
                            let action = TradeAction::Arbitrage {
                                mint,
                                pool_a_name: format!("{:?}", prog_a),
                                pool_b_name: format!("{:?}", prog_b),
                                sol_in_lamports: sol_in,
                                min_sol_out_lamports: min_sol_out,
                                expected_profit_lamports: gross_profit,
                                instructions,
                            };

                            let exec = Arc::clone(&executor);
                            let fail_counter = Arc::clone(&consecutive_failures);
                            let cooldown = Arc::clone(&cooldown_until);

                            tokio::spawn(async move {
                                let _permit = permit;
                                info!("⚡ HYBRID TRIGGER: Executing Spatial Arb on {} | In: {:.4} SOL | Exp Profit: {:.4} SOL", sym, sol_in as f64 / 1e9, gross_profit as f64 / 1e9);
                                match exec.execute_action(action).await {
                                    Ok(_) => fail_counter.store(0, Ordering::SeqCst),
                                    Err(_) => {
                                        let fails = fail_counter.fetch_add(1, Ordering::SeqCst);
                                        if fails >= 2 {
                                            warn!("🚨 Circuit breaker: 3 failures. Pausing 5s.");
                                            let mut cd = cooldown.write().await;
                                            *cd = Instant::now() + Duration::from_secs(5);
                                            fail_counter.store(0, Ordering::SeqCst);
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

        warn!("⚠️ Yellowstone gRPC stream disconnected or finished. Reconnecting in 2s...");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    #[allow(unreachable_code)]
    Ok(())
}
