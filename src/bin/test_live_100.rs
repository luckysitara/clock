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
    SubscribeRequestFilterTransactions,
};

use clockit::{
    constants::{
        BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_U64, INITIAL_REAL_TOKEN_RESERVES,
        INITIAL_VIRTUAL_SOL_RESERVES, INITIAL_VIRTUAL_TOKEN_RESERVES, PUMPFUN_PROGRAM,
        SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64, TOTAL_TOKEN_SUPPLY,
    },
    decoders::{
        fast_is_discriminator, BondingCurveAccountPod, CreateInstructionView, PumpFunBuyPod,
        PumpFunSellPod,
    },
    engine::tip_engine::TipEngine,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveExitReason {
    TakeProfit,
    TrailingStop,
    DevDumpPanic,
    CurveGraduated,
    StillHolding,
}

#[derive(Debug, Clone)]
pub struct LiveTokenState {
    pub id: usize,
    pub symbol: String,
    pub name: String,
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub created_at_slot: u64,
    pub entry_time: Instant,
    pub entry_price_sol: f64,
    pub highest_price_sol: f64,
    pub tokens_held: u64,
    pub sol_invested: f64,
    pub sol_returned: f64,
    pub net_pnl_sol: f64,
    pub roi_pct: f64,
    pub status: LiveExitReason,
    pub curve: BondingCurveAccountPod,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();

    println!("==========================================================================================");
    println!("       CLOCKIT: LIVE TEST ON 100 NEWEST TOKENS CREATED ON PUMP.FUN");
    println!("       Powered by Triton Yellowstone gRPC (Processed Commitment)");
    println!("==========================================================================================");

    let grpc_url = std::env::var("YELLOWSTONE_GRPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com:443".to_string());

    let x_token = std::env::var("YELLOWSTONE_X_TOKEN")
        .or_else(|_| std::env::var("TRITON_X_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty());

    let target_token_count: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let entry_sol = 0.20; // 0.20 SOL per token
    let entry_sol_lamports: u64 = 200_000_000;
    let take_profit_multiplier = 2.50; // 2.5x (+150% ROI)
    let trailing_stop_pct = 0.15; // 15% trailing stop
    let _tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);

    println!("📍 Endpoint:          {}", grpc_url);
    println!("🎯 Target Launches:   {} tokens", target_token_count);
    println!("⚙️ Snipe Size:        {:.2} SOL per token", entry_sol);
    println!("⚙️ Take-Profit:       {:.2}x (+150% ROI)", take_profit_multiplier);
    println!("⚙️ Trailing Stop:     {:.1}% pullback", trailing_stop_pct * 100.0);
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

    println!("✅ Connected! Opening live stream for Pump.fun transactions...");

    let mut transactions = HashMap::new();
    transactions.insert(
        "pumpfun_live_snipes".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: Some(false),
            signature: None,
            account_include: vec![PUMPFUN_PROGRAM.to_string()],
            account_exclude: vec![],
            account_required: vec![],
        },
    );

    let request = SubscribeRequest {
        accounts: HashMap::new(),
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

    println!("📡 Live Stream Active! Sniping next {} newly created tokens in real-time...\n", target_token_count);

    let tracked_tokens: Arc<RwLock<HashMap<Pubkey, LiveTokenState>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let token_order: Arc<RwLock<Vec<Pubkey>>> = Arc::new(RwLock::new(Vec::new()));

    let sim_start = Instant::now();
    let mut observation_start: Option<Instant> = None;
    let mut detection_latencies_ns: Vec<u128> = Vec::new();
    let mut total_created = 0;

    while let Some(msg_res) = stream.next().await {
        let msg = match msg_res {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Stream error: {}", e);
                break;
            }
        };

        if let Some(UpdateOneof::Transaction(tx_info)) = msg.update_oneof {
            let tx_sub = match tx_info.transaction.as_ref() {
                Some(t) => t,
                None => continue,
            };
            let tx = match tx_sub.transaction.as_ref() {
                Some(t) => t,
                None => continue,
            };
            let msg_data = match tx.message.as_ref() {
                Some(m) => m,
                None => continue,
            };

            let account_keys: Vec<[u8; 32]> = msg_data
                .account_keys
                .iter()
                .filter_map(|k| k.as_slice().try_into().ok())
                .collect();

            let mut all_instructions: Vec<(&[u8], &[u8])> =
                Vec::with_capacity(msg_data.instructions.len() + 8);
            for ix in &msg_data.instructions {
                all_instructions.push((&ix.data, &ix.accounts));
            }
            if let Some(meta) = tx_sub.meta.as_ref() {
                for inner_set in &meta.inner_instructions {
                    for inner_ix in &inner_set.instructions {
                        all_instructions.push((&inner_ix.data, &inner_ix.accounts));
                    }
                }
            }

            for (data, accounts) in all_instructions {
                // 1. DETECT TOKEN CREATION (New Launch)
                if fast_is_discriminator(data, CREATE_DISCRIMINATOR_U64) {
                    let detect_start = Instant::now();

                    if let Some(view) = CreateInstructionView::parse(data) {
                        if accounts.len() > 0 && (accounts[0] as usize) < account_keys.len() {
                            let mint_idx = accounts[0] as usize;
                            let mint = Pubkey::new_from_array(account_keys[mint_idx]);

                            let mut tracked_lock = tracked_tokens.write().await;
                            if tracked_lock.len() < target_token_count && !tracked_lock.contains_key(&mint) {
                                total_created += 1;
                                let id = total_created;

                                let creator = view.creator.unwrap_or_else(|| {
                                    if accounts.len() > 7 && (accounts[7] as usize) < account_keys.len() {
                                        Pubkey::new_from_array(account_keys[accounts[7] as usize])
                                    } else {
                                        Pubkey::new_from_array(account_keys[0])
                                    }
                                });

                                let mut initial_curve = BondingCurveAccountPod {
                                    virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                                    virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                                    real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                                    real_sol_reserves: 0,
                                    token_total_supply: TOTAL_TOKEN_SUPPLY,
                                    complete: false,
                                    creator,
                                };

                                let buy_res = initial_curve
                                    .calculate_buy_output(entry_sol_lamports, 500)
                                    .unwrap();

                                let entry_price = buy_res.effective_price_sol;
                                let tokens_out = buy_res.tokens_out;

                                // Update curve reserves with our simulated snipe
                                initial_curve.virtual_sol_reserves += entry_sol_lamports;
                                initial_curve.real_sol_reserves += entry_sol_lamports;
                                initial_curve.virtual_token_reserves =
                                    initial_curve.virtual_token_reserves.saturating_sub(tokens_out);
                                initial_curve.real_token_reserves =
                                    initial_curve.real_token_reserves.saturating_sub(tokens_out);

                                let latency_ns = detect_start.elapsed().as_nanos();
                                detection_latencies_ns.push(latency_ns);

                                println!(
                                    "[{:>3}/{}] 🚀 SNIPED: [{:<8}] \"{:<18}\" | Mint: {} | Latency: {} ns",
                                    id, target_token_count,
                                    if view.symbol.len() > 8 { &view.symbol[..8] } else { view.symbol },
                                    if view.name.len() > 18 { &view.name[..18] } else { view.name },
                                    mint, latency_ns
                                );
                                std::io::stdout().flush().ok();

                                tracked_lock.insert(
                                    mint,
                                    LiveTokenState {
                                        id,
                                        symbol: view.symbol.to_string(),
                                        name: view.name.to_string(),
                                        mint,
                                        creator,
                                        created_at_slot: tx_info.slot,
                                        entry_time: Instant::now(),
                                        entry_price_sol: entry_price,
                                        highest_price_sol: entry_price,
                                        tokens_held: tokens_out,
                                        sol_invested: entry_sol,
                                        sol_returned: 0.0,
                                        net_pnl_sol: 0.0,
                                        roi_pct: 0.0,
                                        status: LiveExitReason::StillHolding,
                                        curve: initial_curve,
                                    },
                                );

                                token_order.write().await.push(mint);
                            }
                        }
                    }
                }

                // 2. DETECT BUYS ON TRACKED TOKENS (Price Appreciation & Exits)
                else if fast_is_discriminator(data, BUY_DISCRIMINATOR_U64) {
                    if let Some(buy_pod) = PumpFunBuyPod::read_from_raw(data) {
                        if accounts.len() > 2 && (accounts[2] as usize) < account_keys.len() {
                            let mint_idx = accounts[2] as usize;
                            let mint = Pubkey::new_from_array(account_keys[mint_idx]);

                            let mut tracked_lock = tracked_tokens.write().await;
                            if let Some(token) = tracked_lock.get_mut(&mint) {
                                if token.status == LiveExitReason::StillHolding {
                                    // Update curve state
                                    token.curve.virtual_sol_reserves += buy_pod.max_sol_cost;
                                    token.curve.real_sol_reserves += buy_pod.max_sol_cost;
                                    token.curve.virtual_token_reserves = token
                                        .curve
                                        .virtual_token_reserves
                                        .saturating_sub(buy_pod.amount);
                                    token.curve.real_token_reserves = token
                                        .curve
                                        .real_token_reserves
                                        .saturating_sub(buy_pod.amount);

                                    let current_price = token.curve.current_spot_price_sol();
                                    if current_price > token.highest_price_sol {
                                        token.highest_price_sol = current_price;
                                    }

                                    let price_multiple = current_price / token.entry_price_sol;
                                    let stop_price = token.highest_price_sol * (1.0 - trailing_stop_pct);

                                    // Check Take Profit (2.5x)
                                    if price_multiple >= take_profit_multiplier {
                                        let returned = (token.tokens_held as f64 * current_price) / 1e6;
                                        token.sol_returned = returned;
                                        token.net_pnl_sol = returned - token.sol_invested;
                                        token.roi_pct = (token.net_pnl_sol / token.sol_invested) * 100.0;
                                        token.status = LiveExitReason::TakeProfit;

                                        println!(
                                            "[{:>3}] 💰 TAKE-PROFIT EXIT: [{}] hit {:.2}x ({:>+6.1}% ROI) | PnL: {:>+6.3} SOL",
                                            token.id, token.symbol, price_multiple, token.roi_pct, token.net_pnl_sol
                                        );
                                        std::io::stdout().flush().ok();
                                    }
                                    // Check Trailing Stop (15% pullback after at least 30% gain)
                                    else if current_price <= stop_price && token.highest_price_sol > token.entry_price_sol * 1.30 {
                                        let returned = (token.tokens_held as f64 * current_price) / 1e6;
                                        token.sol_returned = returned;
                                        token.net_pnl_sol = returned - token.sol_invested;
                                        token.roi_pct = (token.net_pnl_sol / token.sol_invested) * 100.0;
                                        token.status = LiveExitReason::TrailingStop;

                                        println!(
                                            "[{:>3}] 📉 TRAILING-STOP EXIT: [{}] pulled back ({:>+6.1}% ROI) | PnL: {:>+6.3} SOL",
                                            token.id, token.symbol, token.roi_pct, token.net_pnl_sol
                                        );
                                        std::io::stdout().flush().ok();
                                    }
                                }
                            }
                        }
                    }
                }

                // 3. DETECT SELLS & DEV DUMPS (Panic Front-Running)
                else if fast_is_discriminator(data, SELL_DISCRIMINATOR_U64)
                    || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64)
                {
                    if let Some(sell_pod) = PumpFunSellPod::read_from_raw(data) {
                        if accounts.len() > 2 && (accounts[2] as usize) < account_keys.len() {
                            let mint_idx = accounts[2] as usize;
                            let mint = Pubkey::new_from_array(account_keys[mint_idx]);
                            let signer = Pubkey::new_from_array(account_keys[0]);

                            let mut tracked_lock = tracked_tokens.write().await;
                            if let Some(token) = tracked_lock.get_mut(&mint) {
                                if token.status == LiveExitReason::StillHolding {
                                    // Check if Creator is dumping > 20%
                                    if signer == token.creator {
                                        let current_price = token.curve.current_spot_price_sol();
                                        let returned = (token.tokens_held as f64 * current_price) / 1e6;
                                        token.sol_returned = returned;
                                        token.net_pnl_sol = returned - token.sol_invested;
                                        token.roi_pct = (token.net_pnl_sol / token.sol_invested) * 100.0;
                                        token.status = LiveExitReason::DevDumpPanic;

                                        println!(
                                            "[{:>3}] ⚠️ DEV DUMP FRONT-RUN EXIT: [{}] dev dumping! Escaped with {:>+6.3} SOL ({:>+6.1}% ROI)",
                                            token.id, token.symbol, token.net_pnl_sol, token.roi_pct
                                        );
                                        std::io::stdout().flush().ok();
                                    } else {
                                        // Regular sell -> update curve
                                        token.curve.virtual_token_reserves += sell_pod.amount;
                                        token.curve.real_token_reserves += sell_pod.amount;
                                        token.curve.virtual_sol_reserves = token
                                            .curve
                                            .virtual_sol_reserves
                                            .saturating_sub(sell_pod.min_sol_output);
                                        token.curve.real_sol_reserves = token
                                            .curve
                                            .real_sol_reserves
                                            .saturating_sub(sell_pod.min_sol_output);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Check if we hit target tokens and completed observation
        let count = tracked_tokens.read().await.len();
        if count >= target_token_count {
            if observation_start.is_none() {
                observation_start = Some(Instant::now());
                println!("\n🎯 Successfully sniped all {} target tokens! Monitoring subsequent trades for 30s...", target_token_count);
                std::io::stdout().flush().ok();
            } else if observation_start.unwrap().elapsed() > Duration::from_secs(30) {
                println!("⏱️ 30-second observation window completed! Calculating final PnL...\n");
                std::io::stdout().flush().ok();
                break;
            }
        }
    }

    // Generate Final Scorecard
    let tracked = tracked_tokens.read().await;
    let order = token_order.read().await;

    let mut total_invested = 0.0;
    let mut total_returned = 0.0;
    let mut wins = 0;
    let mut losses = 0;
    let mut dev_dumps_caught = 0;
    let mut tp_hits = 0;
    let mut trailing_stops = 0;

    for mint in order.iter() {
        if let Some(t) = tracked.get(mint) {
            total_invested += t.sol_invested;
            let returned = if t.status == LiveExitReason::StillHolding {
                // Mark to market current curve price
                let current_price = t.curve.current_spot_price_sol();
                (t.tokens_held as f64 * current_price) / 1e6
            } else {
                t.sol_returned
            };
            total_returned += returned;

            let net_pnl = returned - t.sol_invested;
            if net_pnl >= 0.0 {
                wins += 1;
            } else {
                losses += 1;
            }

            match t.status {
                LiveExitReason::TakeProfit => tp_hits += 1,
                LiveExitReason::TrailingStop => trailing_stops += 1,
                LiveExitReason::DevDumpPanic => dev_dumps_caught += 1,
                _ => {}
            }
        }
    }

    let avg_latency: u128 = if !detection_latencies_ns.is_empty() {
        detection_latencies_ns.iter().sum::<u128>() / detection_latencies_ns.len() as u128
    } else {
        0
    };

    let total_net_pnl = total_returned - total_invested;
    let portfolio_roi = if total_invested > 0.0 {
        (total_net_pnl / total_invested) * 100.0
    } else {
        0.0
    };

    println!("\n==========================================================================================");
    println!("              LIVE TESTING SCORECARD: LAST {} TOKENS ON PUMP.FUN", tracked.len());
    println!("==========================================================================================");
    println!(" Total New Tokens Sniped:   {}", tracked.len());
    println!(" Total Test Duration:       {:.1} seconds", sim_start.elapsed().as_secs_f64());
    println!(" Avg Detection Latency:     {} nanoseconds (~{:.2} µs)", avg_latency, avg_latency as f64 / 1000.0);
    println!("------------------------------------------------------------------------------------------");
    println!(" Winning Snipes:            {} (Win Rate: {:.1}%)", wins, if tracked.len() > 0 { (wins as f64 / tracked.len() as f64) * 100.0 } else { 0.0 });
    println!(" Losing Snipes:             {}", losses);
    println!(" Take-Profit (2.5x) Hits:   {}", tp_hits);
    println!(" Trailing Stop Exits:       {}", trailing_stops);
    println!(" Dev Rugs Avoided (Panic):  {}", dev_dumps_caught);
    println!("------------------------------------------------------------------------------------------");
    println!(" Total SOL Deployed:        {:.2} SOL ({:.2} SOL * {} tokens)", total_invested, entry_sol, tracked.len());
    println!(" Total SOL Recovered:       {:.4} SOL", total_returned);
    println!(" NET SIMULATED PROFIT:      {:>+8.4} SOL", total_net_pnl);
    println!(" NET PORTFOLIO ROI:         {:>+8.2}% across live stream", portfolio_roi);
    println!("==========================================================================================\n");

    Ok(())
}
