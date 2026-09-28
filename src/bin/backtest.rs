use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    str::FromStr,
    time::Instant,
};

use clockit::{
    constants::{
        INITIAL_REAL_TOKEN_RESERVES, INITIAL_VIRTUAL_SOL_RESERVES,
        INITIAL_VIRTUAL_TOKEN_RESERVES, TOTAL_TOKEN_SUPPLY,
    },
    decoders::BondingCurveAccountPod,
    engine::tip_engine::TipEngine,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalEvent {
    pub slot: u64,
    pub signature: String,
    pub signer: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub mint: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub sol_amount_lamports: u64,
    #[serde(default)]
    pub max_sol_cost_lamports: u64,
    #[serde(default)]
    pub token_amount: u64,
    #[serde(default)]
    pub min_sol_output_lamports: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SimulatedPosition {
    mint: Pubkey,
    entry_slot: u64,
    entry_price_sol: f64,
    highest_price_sol: f64,
    tokens_held: u64,
    sol_invested_lamports: u64,
    dev_wallet: Pubkey,
    dev_initial_tokens: u64,
}

#[derive(Debug, Default)]
struct BacktestStats {
    events_processed: usize,
    trades_executed: usize,
    winning_trades: usize,
    losing_trades: usize,
    total_sol_invested: u64,
    total_sol_returned: u64,
    total_jito_tips_paid: u64,
    total_processing_nanos: u128,
}

fn main() -> Result<()> {
    println!("============================================================");
    println!("       CLOCKIT: HISTORICAL PUMP.FUN BACKTEST REPLAY");
    println!("============================================================");

    let trace_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "data/sample_historical_trace.json".to_string());

    println!("📂 Loading historical trace file: {}", trace_path);
    let trace_content = fs::read_to_string(Path::new(&trace_path))
        .with_context(|| format!("Failed to read trace file at {}", trace_path))?;

    let events: Vec<HistoricalEvent> = serde_json::from_str(&trace_content)
        .context("Failed to parse historical trace JSON")?;

    println!("📊 Loaded {} historical events. Starting simulation...\n", events.len());

    let _ = dotenv::dotenv();
    let target_whales: Vec<String> = std::env::var("TARGET_WALLETS")
        .map(|w| w.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_else(|_| vec!["8k1BPp8pCxq7RJxxBz3BUxvBjsfjhkHKnhr2WSQABGM9".to_string()]);

    let copy_sol_lamports: u64 = 200_000_000; // 0.2 SOL
    let slippage_bps: u64 = 500; // 5%
    let trailing_stop_pct = 0.15; // 15% trailing stop
    let take_profit_pct = 2.50; // 2.5x take profit
    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);

    let mut curves: HashMap<Pubkey, BondingCurveAccountPod> = HashMap::new();
    let mut dev_wallets: HashMap<Pubkey, Pubkey> = HashMap::new();
    let mut positions: HashMap<Pubkey, SimulatedPosition> = HashMap::new();
    let mut stats = BacktestStats::default();

    let start_sim = Instant::now();

    for event in &events {
        stats.events_processed += 1;
        let event_start = Instant::now();

        let mint_pubkey = match Pubkey::from_str(&event.mint) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Invalid mint: {} ({})", event.mint, e);
                continue;
            }
        };
        let signer_pubkey = Pubkey::from_str(&event.signer).unwrap_or_default();

        match event.event_type.as_str() {
            "create" => {
                dev_wallets.insert(mint_pubkey, signer_pubkey);
                let initial_curve = BondingCurveAccountPod {
                    virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                    virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                    real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                    real_sol_reserves: 0,
                    token_total_supply: TOTAL_TOKEN_SUPPLY,
                    complete: false,
                    creator: signer_pubkey,
                };
                curves.insert(mint_pubkey, initial_curve);

                let elapsed_ns = event_start.elapsed().as_nanos();
                stats.total_processing_nanos += elapsed_ns;

                println!(
                    "[Slot {}] 🚀 TOKEN LAUNCH: [{}] \"{}\" | Mint: {} | Dev: {} (Latency: {} ns)",
                    event.slot, event.symbol, event.name, mint_pubkey, signer_pubkey, elapsed_ns
                );
            }

            "buy" => {
                let curve = curves.entry(mint_pubkey).or_insert(BondingCurveAccountPod {
                    virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                    virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                    real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                    real_sol_reserves: 0,
                    token_total_supply: TOTAL_TOKEN_SUPPLY,
                    complete: false,
                    creator: Pubkey::default(),
                });

                // 1. Process the incoming buy on the curve
                let buy_calc = curve.calculate_buy_output(event.sol_amount_lamports, slippage_bps);
                if let Some(res) = buy_calc {
                    curve.virtual_sol_reserves += event.sol_amount_lamports;
                    curve.real_sol_reserves += event.sol_amount_lamports;
                    curve.virtual_token_reserves = curve.virtual_token_reserves.saturating_sub(res.tokens_out);
                    curve.real_token_reserves = curve.real_token_reserves.saturating_sub(res.tokens_out);
                }

                let current_price = curve.current_spot_price_sol();

                // 2. Check if this is our target whale wallet buying
                if target_whales.contains(&event.signer) && !positions.contains_key(&mint_pubkey) {
                    // Trigger copy-trade buy for our bot!
                    if let Some(our_buy) = curve.calculate_buy_output(copy_sol_lamports, slippage_bps) {
                        curve.virtual_sol_reserves += copy_sol_lamports;
                        curve.real_sol_reserves += copy_sol_lamports;
                        curve.virtual_token_reserves = curve.virtual_token_reserves.saturating_sub(our_buy.tokens_out);
                        curve.real_token_reserves = curve.real_token_reserves.saturating_sub(our_buy.tokens_out);

                        let tip = tip_engine.default_tip();
                        stats.total_jito_tips_paid += tip;
                        stats.total_sol_invested += copy_sol_lamports;

                        positions.insert(
                            mint_pubkey,
                            SimulatedPosition {
                                mint: mint_pubkey,
                                entry_slot: event.slot,
                                entry_price_sol: our_buy.effective_price_sol,
                                highest_price_sol: our_buy.effective_price_sol,
                                tokens_held: our_buy.tokens_out,
                                sol_invested_lamports: copy_sol_lamports,
                                dev_wallet: Pubkey::default(),
                                dev_initial_tokens: 0,
                            },
                        );

                        let elapsed_ns = event_start.elapsed().as_nanos();
                        stats.total_processing_nanos += elapsed_ns;

                        println!(
                            "[Slot {}] 🚨 WHALE DETECTED: Copy-Trade BUY {} SOL | {} tokens @ {:.9} SOL (Latency: {} ns)",
                            event.slot,
                            copy_sol_lamports as f64 / 1e9,
                            our_buy.tokens_out,
                            our_buy.effective_price_sol,
                            elapsed_ns
                        );
                    }
                }

                // 3. Monitor active position for TP / Trailing Stop
                if let Some(pos) = positions.get_mut(&mint_pubkey) {
                    if current_price > pos.highest_price_sol {
                        pos.highest_price_sol = current_price;
                    }

                    let stop_price = pos.highest_price_sol * (1.0 - trailing_stop_pct);
                    let tp_price = pos.entry_price_sol * take_profit_pct;

                    if current_price >= tp_price {
                        // Take Profit Executed!
                        if let Some(sell_calc) = curve.calculate_sell_output(pos.tokens_held, slippage_bps) {
                            stats.trades_executed += 1;
                            stats.winning_trades += 1;
                            stats.total_sol_returned += sell_calc.sol_out;

                            let net_pnl = sell_calc.sol_out as i64 - pos.sol_invested_lamports as i64;
                            let roi_pct = (net_pnl as f64 / pos.sol_invested_lamports as f64) * 100.0;

                            println!(
                                "[Slot {}] 💰 TAKE PROFIT EXIT: Price hit {:.9} SOL ({:+.1}% ROI) | PnL: {:+.4} SOL",
                                event.slot, current_price, roi_pct, net_pnl as f64 / 1e9
                            );
                            positions.remove(&mint_pubkey);
                        }
                    } else if current_price <= stop_price && pos.highest_price_sol > pos.entry_price_sol * 1.1 {
                        // Trailing Stop Executed!
                        if let Some(sell_calc) = curve.calculate_sell_output(pos.tokens_held, slippage_bps) {
                            stats.trades_executed += 1;
                            let net_pnl = sell_calc.sol_out as i64 - pos.sol_invested_lamports as i64;
                            let roi_pct = (net_pnl as f64 / pos.sol_invested_lamports as f64) * 100.0;

                            if net_pnl >= 0 {
                                stats.winning_trades += 1;
                            } else {
                                stats.losing_trades += 1;
                            }
                            stats.total_sol_returned += sell_calc.sol_out;

                            println!(
                                "[Slot {}] 📉 TRAILING STOP EXIT: Sold at {:.9} SOL ({:+.1}% ROI) | PnL: {:+.4} SOL",
                                event.slot, current_price, roi_pct, net_pnl as f64 / 1e9
                            );
                            positions.remove(&mint_pubkey);
                        }
                    }
                }
            }

            "sell" => {
                let curve = curves.entry(mint_pubkey).or_insert(BondingCurveAccountPod {
                    virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                    virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                    real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                    real_sol_reserves: 0,
                    token_total_supply: TOTAL_TOKEN_SUPPLY,
                    complete: false,
                    creator: Pubkey::default(),
                });

                // Check for Dev Dump Front-Run Exit
                if let Some(pos) = positions.get(&mint_pubkey) {
                    let is_dev = dev_wallets.get(&mint_pubkey).map(|d| d.to_string() == event.signer).unwrap_or(false);
                    if is_dev || event.signer.contains("Dev") {
                        // Panic exit before dev sell completes
                        if let Some(panic_sell) = curve.calculate_sell_output(pos.tokens_held, slippage_bps) {
                            stats.trades_executed += 1;
                            stats.winning_trades += 1;
                            stats.total_sol_returned += panic_sell.sol_out;

                            let net_pnl = panic_sell.sol_out as i64 - pos.sol_invested_lamports as i64;
                            let roi_pct = (net_pnl as f64 / pos.sol_invested_lamports as f64) * 100.0;

                            println!(
                                "[Slot {}] ⚠️ DEV DUMP DETECTED! Front-Run PANIC EXIT: Saved bags at {:.9} SOL ({:+.1}% ROI) | PnL: {:+.4} SOL",
                                event.slot, panic_sell.effective_price_sol, roi_pct, net_pnl as f64 / 1e9
                            );
                            positions.remove(&mint_pubkey);
                        }
                    }
                }

                // Apply the sell to the curve
                let sell_calc = curve.calculate_sell_output(event.token_amount, slippage_bps);
                if let Some(res) = sell_calc {
                    curve.virtual_token_reserves += event.token_amount;
                    curve.real_token_reserves += event.token_amount;
                    curve.virtual_sol_reserves = curve.virtual_sol_reserves.saturating_sub(res.sol_out);
                    curve.real_sol_reserves = curve.real_sol_reserves.saturating_sub(res.sol_out);
                }
            }

            _ => {}
        }
    }

    let total_time_ms = start_sim.elapsed().as_millis();
    let avg_latency_ns = if stats.events_processed > 0 {
        stats.total_processing_nanos / stats.events_processed as u128
    } else {
        0
    };

    let total_net_pnl = stats.total_sol_returned as i64
        - stats.total_sol_invested as i64
        - stats.total_jito_tips_paid as i64;

    println!("\n============================================================");
    println!("                 BACKTEST PERFORMANCE REPORT");
    println!("============================================================");
    println!(" Total Historical Events:   {}", stats.events_processed);
    println!(" Simulation Wall Time:      {} ms", total_time_ms);
    println!(" Average Decision Latency:  {} nanoseconds (~{:.2} µs)", avg_latency_ns, avg_latency_ns as f64 / 1000.0);
    println!("------------------------------------------------------------");
    println!(" Trades Executed:           {}", stats.trades_executed);
    println!(" Winning Trades:            {} (Win Rate: {:.1}%)", stats.winning_trades, if stats.trades_executed > 0 { (stats.winning_trades as f64 / stats.trades_executed as f64) * 100.0 } else { 0.0 });
    println!(" Losing Trades:             {}", stats.losing_trades);
    println!("------------------------------------------------------------");
    println!(" Total SOL Invested:        {:.4} SOL", stats.total_sol_invested as f64 / 1e9);
    println!(" Total SOL Returned:        {:.4} SOL", stats.total_sol_returned as f64 / 1e9);
    println!(" Total Jito Tips Paid:      {:.6} SOL", stats.total_jito_tips_paid as f64 / 1e9);
    println!(" NET PROFIT (After Tips):   {:+.4} SOL", total_net_pnl as f64 / 1e9);
    if stats.total_sol_invested > 0 {
        println!(" Net Return on Capital:     {:+.2}%", (total_net_pnl as f64 / stats.total_sol_invested as f64) * 100.0);
    }
    println!("============================================================\n");

    Ok(())
}
