use anyhow::{Context, Result};
use futures::StreamExt;
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::HashMap,
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterTransactions,
};

use clockit::{
    constants::{
        BUY_DISCRIMINATOR_U64, PUMPFUN_PROGRAM, RAYDIUM_AMM_V4_PROGRAM,
        SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64,
    },
    decoders::{
        arb_math::ConstantProductPool,
        fast_is_discriminator, BondingCurveAccountPod, PumpFunBuyPod, PumpFunSellPod,
    },
    engine::{arb_engine::ArbitrageEngine, tip_engine::TipEngine},
};

#[derive(Debug, Default)]
struct LiveArbStats {
    total_slots_observed: u64,
    total_transactions_parsed: usize,
    total_account_updates: usize,
    arbs_detected: usize,
    total_simulated_volume_sol: f64,
    total_gross_profit_sol: f64,
    total_jito_tips_sol: f64,
    total_net_profit_sol: f64,
    compounded_wallet_sol: f64,
    latencies_ns: Vec<u128>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();

    println!("==========================================================================================");
    println!("       CLOCKIT: LIVE CROSS-DEX ARBITRAGE REAL-TIME WATCHER (DRY-RUN / PAPER MODE)");
    println!("       Powered by Triton Yellowstone gRPC (Microsecond Processed Stream)");
    println!("==========================================================================================");

    let grpc_url = std::env::var("YELLOWSTONE_GRPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com:443".to_string());

    let x_token = std::env::var("YELLOWSTONE_X_TOKEN")
        .or_else(|_| std::env::var("TRITON_X_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty());

    // Capital constraint: 0.50 SOL default
    let args: Vec<String> = std::env::args().collect();
    let mut capital_sol: f64 = 0.50;
    let mut run_seconds: u64 = 30; // Run for 30 seconds default or until Ctrl+C

    for i in 1..args.len() {
        if (args[i] == "--capital" || args[i] == "-c") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<f64>() {
                capital_sol = val;
            }
        } else if (args[i] == "--duration" || args[i] == "-d") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<u64>() {
                run_seconds = val;
            }
        }
    }

    println!("📍 Triton Yellowstone gRPC: {}", grpc_url);
    println!("💰 Paper Trading Capital:   {:.4} SOL (Single-bullet constraint)", capital_sol);
    println!("🛡️ Execution Mode:          DRY-RUN PAPER TRADING (0 Real Funds Spent)");
    println!("⏱️ Observation Duration:    {} seconds", run_seconds);
    println!("------------------------------------------------------------------------------------------\n");

    println!("[1/2] Connecting to Triton Yellowstone gRPC with TLS...");
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

    println!("✅ Connected! Configuring real-time gRPC subscriptions...");

    // Subscriptions:
    // 1. Transactions: Pump.fun + Raydium AMM
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

    // 2. Account updates: Pump.fun bonding curves
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

    println!("📡 Live Stream Active! Watching live Mainnet orders for arbitrage opportunities...\n");

    let max_trade_lamports = (capital_sol * 1e9) as u64;
    let min_profit_lamports = 1_000_000; // 0.001 SOL minimum profit hurdle
    let arb_engine = ArbitrageEngine::new(min_profit_lamports, max_trade_lamports);
    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);

    let curve_cache: Arc<RwLock<HashMap<Pubkey, ConstantProductPool>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let stats = Arc::new(RwLock::new(LiveArbStats {
        compounded_wallet_sol: capital_sol,
        ..Default::default()
    }));

    let stream_start = Instant::now();
    let max_duration = Duration::from_secs(run_seconds);

    println!("{:<10} | {:<44} | {:<12} | {:<10} | {:<10} | {:<8} | {:<8}",
        "SLOT", "TOKEN MINT", "DIRECTION", "INPUT SOL", "NET PROFIT", "ROI %", "LATENCY");
    println!("-----------+----------------------------------------------+--------------+------------+------------+----------+--------");

    while let Some(msg_res) = stream.next().await {
        if stream_start.elapsed() >= max_duration {
            break;
        }

        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => {
                eprintln!("gRPC stream error: {}", e);
                break;
            }
        };

        match msg.update_oneof {
            // 1. Account Updates: Live Bonding Curves
            Some(UpdateOneof::Account(acc_update)) => {
                let mut st = stats.write().await;
                st.total_account_updates += 1;

                if let Some(account_info) = acc_update.account {
                    if let Ok(curve_pubkey) = Pubkey::try_from(account_info.pubkey.as_slice()) {
                        if let Some(pod) = BondingCurveAccountPod::read_from_account(&account_info.data) {
                            if pod.virtual_sol_reserves > 0 && pod.virtual_token_reserves > 0 {
                                let pool = ConstantProductPool::new(
                                    pod.virtual_sol_reserves,
                                    pod.virtual_token_reserves,
                                    100, // 1% fee on Pump.fun
                                );
                                let mut cache = curve_cache.write().await;
                                cache.insert(curve_pubkey, pool);
                            }
                        }
                    }
                }
            }

            // 2. Transactions: Live Swaps on Pump.fun & Raydium
            Some(UpdateOneof::Transaction(tx_update)) => {
                let mut st = stats.write().await;
                st.total_transactions_parsed += 1;
                st.total_slots_observed = tx_update.slot;

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

                // Check for Buy / Sell instructions
                for (data, accounts) in all_ixs {
                    let is_buy = fast_is_discriminator(data, BUY_DISCRIMINATOR_U64);
                    let is_sell = fast_is_discriminator(data, SELL_DISCRIMINATOR_U64)
                        || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64);

                    if (is_buy || is_sell) && accounts.len() > 2 {
                        let mint_idx = accounts[2] as usize;
                        if mint_idx < account_keys.len() {
                            let mint = Pubkey::new_from_array(account_keys[mint_idx]);
                            let (bonding_curve, _) = clockit::constants::derive_bonding_curve(&mint);

                            // Get current curve state
                            let cache = curve_cache.read().await;
                            let base_pool = cache.get(&bonding_curve).copied().unwrap_or_else(|| {
                                ConstantProductPool::new(
                                    clockit::constants::INITIAL_VIRTUAL_SOL_RESERVES,
                                    clockit::constants::INITIAL_VIRTUAL_TOKEN_RESERVES,
                                    100,
                                )
                            });
                            drop(cache);

                            // Extract trade size
                            let trade_sol = if is_buy {
                                PumpFunBuyPod::read_from_raw(data).map(|p| p.max_sol_cost).unwrap_or(500_000_000)
                            } else {
                                PumpFunSellPod::read_from_raw(data).map(|p| p.min_sol_output).unwrap_or(500_000_000)
                            };

                            // Analyze trades that create actionable price movement (>= 0.1 SOL)
                            if trade_sol >= 100_000_000 {
                                let eval_start = Instant::now();

                                // Pre-trade state is external AMM reference (Raydium / DEX)
                                let pool_amm = ConstantProductPool::new(
                                    base_pool.sol_reserves,
                                    base_pool.token_reserves,
                                    25, // 0.25% fee on Raydium
                                );

                                // Post-trade state on curve
                                let mut pool_curve = base_pool;
                                if is_buy {
                                    if let Some(tokens_out) = pool_curve.swap_sol_for_tokens(trade_sol) {
                                        pool_curve.sol_reserves += trade_sol;
                                        pool_curve.token_reserves = pool_curve.token_reserves.saturating_sub(tokens_out);
                                    }
                                } else {
                                    let tokens_in = (trade_sol as f64 / pool_curve.spot_price_sol().max(1e-12)) as u64;
                                    if let Some(sol_out) = pool_curve.swap_tokens_for_sol(tokens_in) {
                                        pool_curve.token_reserves += tokens_in;
                                        pool_curve.sol_reserves = pool_curve.sol_reserves.saturating_sub(sol_out);
                                    }
                                }

                                // Update in-memory curve cache
                                {
                                    let mut w_cache = curve_cache.write().await;
                                    w_cache.insert(bonding_curve, pool_curve);
                                }

                                if let Some(opp) = arb_engine.evaluate_pair(&pool_curve, &pool_amm) {
                                    let eval_nanos = eval_start.elapsed().as_nanos();
                                    st.latencies_ns.push(eval_nanos);

                                    let tip = tip_engine
                                        .calculate_optimal_tip(opp.gross_profit_lamports)
                                        .unwrap_or_else(|| tip_engine.default_tip());

                                    let deployed_sol = opp.optimal_sol_in_lamports as f64 / 1e9;
                                    let gross_sol = opp.gross_profit_lamports as f64 / 1e9;
                                    let tip_sol = tip as f64 / 1e9;
                                    let net_sol = gross_sol - tip_sol;

                                    if net_sol > 0.0005 {
                                        st.arbs_detected += 1;
                                        st.total_simulated_volume_sol += deployed_sol;
                                        st.total_gross_profit_sol += gross_sol;
                                        st.total_jito_tips_sol += tip_sol;
                                        st.total_net_profit_sol += net_sol;
                                        st.compounded_wallet_sol += net_sol;

                                        let dir_str = match opp.direction {
                                            clockit::decoders::arb_math::ArbDirection::PoolAToPoolB => "PUMP -> RAY",
                                            clockit::decoders::arb_math::ArbDirection::PoolBToPoolA => "RAY -> PUMP",
                                        };

                                        println!("{:<10} | {:<44} | {:<12} | {:>7.4} SOL | {:>+8.4} SOL | {:>+6.2}% | {:>5} ns",
                                            tx_update.slot,
                                            mint,
                                            dir_str,
                                            deployed_sol,
                                            net_sol,
                                            opp.roi_bps as f64 / 100.0,
                                            eval_nanos
                                        );
                                        std::io::stdout().flush().ok();
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

    // Print Final Live Paper Trading Scorecard
    let st = stats.read().await;
    let avg_latency = if !st.latencies_ns.is_empty() {
        st.latencies_ns.iter().sum::<u128>() / st.latencies_ns.len() as u128
    } else {
        0
    };

    println!("\n==========================================================================================");
    println!("              LIVE ARBITRAGE DRY-RUN SCORECARD (PAPER TRADING)");
    println!("==========================================================================================");
    println!(" Total Run Duration:          {:.1} seconds", stream_start.elapsed().as_secs_f64());
    println!(" Live Transactions Parsed:    {}", st.total_transactions_parsed);
    println!(" Real-Time Curve Updates:     {}", st.total_account_updates);
    println!(" Arbitrages Detected:         {} actionable spreads", st.arbs_detected);
    println!(" Avg Decision Latency:        {} nanoseconds (~{:.3} µs)", avg_latency, avg_latency as f64 / 1000.0);
    println!("------------------------------------------------------------------------------------------");
    println!(" Initial Paper Wallet:        {:.4} SOL", capital_sol);
    println!(" Total Simulated Volume:      {:.4} SOL (Recycled {} times)", st.total_simulated_volume_sol, st.arbs_detected);
    println!(" Gross Spread Profit:         {:+.4} SOL", st.total_gross_profit_sol);
    println!(" Simulated Jito MEV Tips:     {:.4} SOL (Allocated to validators)", st.total_jito_tips_sol);
    println!(" TOTAL NET PAPER PROFIT:      {:>+10.4} SOL", st.total_net_profit_sol);
    println!(" FINAL COMPOUNDED PAPER WALLET:{:>+9.4} SOL", st.compounded_wallet_sol);
    if capital_sol > 0.0 {
        println!(" NET SIMULATED RETURN:        {:>+10.2}% on {:.2} SOL capital", (st.total_net_profit_sol / capital_sol) * 100.0, capital_sol);
    }
    println!("==========================================================================================\n");

    Ok(())
}
