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
        BUY_DISCRIMINATOR_U64, PUMPFUN_PROGRAM,
        RAYDIUM_AMM_V4_PROGRAM, SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64,
    },
    decoders::{
        arb_math::ConstantProductPool,
        fast_is_discriminator, BondingCurveAccountPod, PumpFunBuyPod, PumpFunSellPod,
    },
    engine::{
        arb_engine::ArbitrageEngine,
        blockhash_cache::BlockhashCache,
        executor::{ExecutionEngine, TradeAction},
        jito::JitoClient,
        position_manager::PositionManager,
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

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    println!("==========================================================================================");
    println!("       CLOCKIT: LIVE CROSS-DEX ATOMIC ARBITRAGE ENGINE (REAL FUNDS / MAINNET)");
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

    // CLI args:
    // --capital <SOL> (default: 0.50)
    // --max-drawdown <SOL> (default: 0.05)
    let args: Vec<String> = std::env::args().collect();
    let mut capital_sol: f64 = 0.50;
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
    println!("🔒 Concurrency Lock:     1 in-flight trade max (No balance collision)");
    println!("♻️ Rent Refund:          Automated CloseAccount in-tx (0 SOL rent locked)");
    println!("⚡ Circuit Breaker:      3-consecutive-failure 5s cooldown");
    println!("------------------------------------------------------------------------------------------\n");

    // 2. Check Wallet Balance on Mainnet
    println!("[1/4] Checking live on-chain SOL balance for {}...", wallet_pubkey);
    let rpc_client = Arc::new(RpcClient::new(rpc_url.clone()));
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
    let gas_reserve_lamports = 50_000_000; // 0.05 SOL reserve for rent/gas/tips
    let total_required_lamports = required_lamports + gas_reserve_lamports;
    let max_drawdown_lamports = (max_drawdown_sol * 1e9) as u64;

    if balance_lamports < total_required_lamports {
        let needed_sol = (total_required_lamports.saturating_sub(balance_lamports)) as f64 / 1e9;
        eprintln!("\n🚨 INSUFFICIENT BALANCE FOR LIVE TRADING!");
        eprintln!("   You requested a trade bullet of {:.4} SOL (+ 0.05 SOL gas reserve).", capital_sol);
        eprintln!("   Required: {:.4} SOL | Current Balance: {:.6} SOL", total_required_lamports as f64 / 1e9, initial_balance_sol);
        eprintln!("   Please fund this wallet to start live trading:");
        eprintln!("   ➡️  WALLET ADDRESS: {}\n", wallet_pubkey);
        eprintln!("   Waiting needed deposit: ~{:.4} SOL\n", needed_sol);
        return Ok(());
    }

    println!("✅ Wallet balance verified! Sufficient capital ({:.4} SOL) available.\n", initial_balance_sol);

    // 3. Initialize Engines & Background Workers
    println!("[2/4] Initializing Blockhash Cache, Tip Engine, and Jito Client...");
    let blockhash_cache = Arc::new(BlockhashCache::new());
    blockhash_cache.spawn_updater(rpc_url.clone(), Duration::from_millis(1200));

    let tip_engine = Arc::new(TipEngine::new(
        100_000,     // 0.0001 SOL min tip
        50_000_000,  // 0.05 SOL max tip
        0.65,        // 65% profit share to validators
    ));

    let jito_client = Arc::new(JitoClient::new(jito_url.clone()));
    jito_client.spawn_prewarmer();

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

    let min_profit_lamports = 1_500_000; // 0.0015 SOL minimum clean net profit hurdle
    let arb_engine = ArbitrageEngine::new(min_profit_lamports, required_lamports);

    // Concurrency Lock: Semaphore of 1
    let in_flight_lock = Arc::new(Semaphore::new(1));

    // Circuit Breaker State
    let consecutive_failures = Arc::new(AtomicU32::new(0));
    let cooldown_until = Arc::new(RwLock::new(Instant::now()));

    // 4. Connect to Triton Yellowstone gRPC
    println!("[3/4] Connecting to Triton Yellowstone gRPC with TLS...");
    let mut builder = GeyserGrpcClient::build_from_shared(grpc_url.clone())
        .context("Invalid gRPC URL")?;

    if grpc_url.starts_with("https://") {
        let tls = yellowstone_grpc_proto::tonic::transport::ClientTlsConfig::new().with_enabled_roots();
        builder = builder.tls_config(tls).context("Failed to configure TLS")?;
    }

    if let Some(token) = &x_token {
        builder = builder
            .x_token(Some(token.clone()))
            .context("Failed to attach x-token")?;
    }

    let mut client = builder
        .connect()
        .await
        .context("Failed to connect to Triton gRPC")?;

    // Subscriptions
    let mut transactions = HashMap::new();
    transactions.insert(
        "live_arb_txs".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: Some(false),
            signature: None,
            account_include: vec![
                PUMPFUN_PROGRAM.to_string(),
                RAYDIUM_AMM_V4_PROGRAM.to_string(),
            ],
            account_exclude: vec![],
            account_required: vec![],
        },
    );

    let mut accounts = HashMap::new();
    accounts.insert(
        "live_curves".to_string(),
        SubscribeRequestFilterAccounts {
            account: vec![],
            owner: vec![PUMPFUN_PROGRAM.to_string()],
            filters: vec![],
            nonempty_txn_signature: None,
        },
    );

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

    let (_sink, mut stream) = client
        .subscribe_with_request(Some(request))
        .await
        .context("Failed to subscribe to Yellowstone stream")?;

    println!("[4/4] 🚀 REAL-FUNDS ARBITRAGE ENGINE ACTIVE! Listening for Mainnet blocks...\n");
    info!("🛡️ Capital Protection Active: All bundles are atomic (revert if profit hurdle not met).");

    let curve_cache: Arc<RwLock<HashMap<Pubkey, ConstantProductPool>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let completed_curves: Arc<RwLock<HashMap<Pubkey, bool>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let mut last_balance_check = Instant::now();

    while let Some(msg_res) = stream.next().await {
        // Periodic Session Drawdown / Stop-Loss Check (every 10 seconds)
        if last_balance_check.elapsed() >= Duration::from_secs(10) {
            last_balance_check = Instant::now();
            if let Ok(current_bal) = rpc_client.get_balance(&wallet_pubkey).await {
                if current_bal + max_drawdown_lamports < balance_lamports {
                    let total_loss_sol = (balance_lamports.saturating_sub(current_bal)) as f64 / 1e9;
                    error!("🛑 [SESSION STOP-LOSS REACHED] Initial: {:.4} SOL | Current: {:.4} SOL | Loss: {:.4} SOL",
                        initial_balance_sol, current_bal as f64 / 1e9, total_loss_sol);
                    error!("🚨 Maximum allowable drawdown of {:.4} SOL breached. Shutting down bot to protect capital.", max_drawdown_sol);
                    std::process::exit(0);
                }
            }
        }

        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => {
                error!("gRPC stream error: {}", e);
                break;
            }
        };

        match msg.update_oneof {
            // Account curve update
            Some(UpdateOneof::Account(acc_update)) => {
                if let Some(account_info) = acc_update.account {
                    if let Ok(curve_pubkey) = Pubkey::try_from(account_info.pubkey.as_slice()) {
                        if let Some(pod) = BondingCurveAccountPod::read_from_account(&account_info.data) {
                            if pod.complete {
                                let mut comp = completed_curves.write().await;
                                comp.insert(curve_pubkey, true);
                                continue;
                            }

                            if pod.virtual_sol_reserves > 0 && pod.virtual_token_reserves > 0 {
                                let pool = ConstantProductPool::new(
                                    pod.virtual_sol_reserves,
                                    pod.virtual_token_reserves,
                                    100,
                                );
                                let mut cache = curve_cache.write().await;
                                cache.insert(curve_pubkey, pool);
                            }
                        }
                    }
                }
            }

            // Transaction update
            Some(UpdateOneof::Transaction(tx_update)) => {
                // Check circuit breaker cooldown
                {
                    let cd = cooldown_until.read().await;
                    if Instant::now() < *cd {
                        continue; // Cooldown active, protect gas
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

                let account_keys: Vec<[u8; 32]> = msg_data
                    .account_keys
                    .iter()
                    .filter_map(|k| k.as_slice().try_into().ok())
                    .collect();

                let mut all_ixs: Vec<(&[u8], &[u8])> = Vec::new();
                for ix in &msg_data.instructions {
                    all_ixs.push((&ix.data, &ix.accounts));
                }
                if let Some(meta) = tx_info.meta.as_ref() {
                    for inner_set in &meta.inner_instructions {
                        for inner in &inner_set.instructions {
                            all_ixs.push((&inner.data, &inner.accounts));
                        }
                    }
                }

                for (data, accounts) in all_ixs {
                    let is_buy = fast_is_discriminator(data, BUY_DISCRIMINATOR_U64);
                    let is_sell = fast_is_discriminator(data, SELL_DISCRIMINATOR_U64)
                        || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64);

                    if (is_buy || is_sell) && accounts.len() > 2 {
                        let mint_idx = accounts[2] as usize;
                        if mint_idx < account_keys.len() {
                            let mint = Pubkey::new_from_array(account_keys[mint_idx]);
                            let (bonding_curve, _) = clockit::constants::derive_bonding_curve(&mint);

                            // Honeypot Filter: check completed curves
                            {
                                let comp = completed_curves.read().await;
                                if comp.contains_key(&bonding_curve) {
                                    continue; // Migrated curve, skip
                                }
                            }

                            let cache = curve_cache.read().await;
                            let base_pool = cache.get(&bonding_curve).copied().unwrap_or_else(|| {
                                ConstantProductPool::new(
                                    clockit::constants::INITIAL_VIRTUAL_SOL_RESERVES,
                                    clockit::constants::INITIAL_VIRTUAL_TOKEN_RESERVES,
                                    100,
                                )
                            });
                            drop(cache);

                            // Honeypot Filter: Sanity check spot price
                            let spot_price = base_pool.spot_price_sol();
                            if spot_price < 1e-12 || spot_price > 10.0 {
                                continue;
                            }

                            let trade_sol = if is_buy {
                                PumpFunBuyPod::read_from_raw(data).map(|p| p.max_sol_cost).unwrap_or(500_000_000)
                            } else {
                                PumpFunSellPod::read_from_raw(data).map(|p| p.min_sol_output).unwrap_or(500_000_000)
                            };

                            if trade_sol >= 100_000_000 {
                                let pool_amm = ConstantProductPool::new(
                                    base_pool.sol_reserves,
                                    base_pool.token_reserves,
                                    25,
                                );

                                let mut pool_curve = base_pool;
                                if is_buy {
                                    if let Some(tokens_out) = pool_curve.swap_sol_for_tokens(trade_sol) {
                                        pool_curve.sol_reserves += trade_sol;
                                        pool_curve.token_reserves = pool_curve.token_reserves.saturating_sub(tokens_out);
                                    }
                                } else {
                                    let tokens_in = (trade_sol as f64 / spot_price.max(1e-12)) as u64;
                                    if let Some(sol_out) = pool_curve.swap_tokens_for_sol(tokens_in) {
                                        pool_curve.token_reserves += tokens_in;
                                        pool_curve.sol_reserves = pool_curve.sol_reserves.saturating_sub(sol_out);
                                    }
                                }

                                {
                                    let mut w_cache = curve_cache.write().await;
                                    w_cache.insert(bonding_curve, pool_curve);
                                }

                                if let Some(opp) = arb_engine.evaluate_pair(&pool_curve, &pool_amm) {
                                    // Sanity filter: Ignore migration outliers (> 1000% ROI or > 50 SOL)
                                    if opp.roi_bps < 100_000 && opp.gross_profit_lamports < 50_000_000_000 {
                                        // Concurrency Lock: Try acquiring in-flight permit
                                        let permit = match in_flight_lock.clone().try_acquire_owned() {
                                            Ok(p) => p,
                                            Err(_) => {
                                                // Trade already in flight, skip to avoid double-spend
                                                continue;
                                            }
                                        };

                                        let sol_in = opp.optimal_sol_in_lamports;
                                        let min_sol_out = opp.expected_sol_out_lamports;
                                        let expected_profit = opp.gross_profit_lamports;

                                        let payer = keypair.pubkey();

                                        // Build 100% complete 5-instruction atomic bundle (Both legs + ATA close rent refund)
                                        let instructions = clockit::engine::pool_registry::build_atomic_arb_bundle_for_mint(&payer, &mint, &opp);

                                        let action = TradeAction::Arbitrage {
                                            mint,
                                            pool_a_name: "Pump.fun".to_string(),
                                            pool_b_name: "Raydium AMM".to_string(),
                                            sol_in_lamports: sol_in,
                                            min_sol_out_lamports: min_sol_out,
                                            expected_profit_lamports: expected_profit,
                                            instructions,
                                        };

                                        // Dispatch directly with semaphore lifecycle guard
                                        let exec = Arc::clone(&executor);
                                        let fail_counter = Arc::clone(&consecutive_failures);
                                        let cooldown = Arc::clone(&cooldown_until);

                                        tokio::spawn(async move {
                                            let _permit = permit; // Holds permit until execution and confirmation finish
                                            match exec.execute_action(action).await {
                                                Ok(_) => {
                                                    fail_counter.store(0, Ordering::SeqCst);
                                                }
                                                Err(e) => {
                                                    warn!("⚠️ Arbitrage attempt resolved: {:#}", e);
                                                    let fails = fail_counter.fetch_add(1, Ordering::SeqCst);
                                                    if fails >= 2 {
                                                        warn!("🚨 Circuit breaker triggered: 3 consecutive failures. Pausing 5 seconds.");
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
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
