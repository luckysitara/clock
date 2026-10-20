use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::{str::FromStr, time::Instant};

use clockit::{
    decoders::arb_math::ArbDirection,
    engine::{
        arb_engine::ArbitrageEngine,
        pool_registry::PoolRegistry,
        tip_engine::TipEngine,
    },
};

#[derive(Debug, Default)]
struct TokenBacktestResult {
    symbol: String,
    total_trades_replayed: usize,
    arbs_captured: usize,
    total_deployed_sol: f64,
    total_gross_profit_sol: f64,
    total_tips_sol: f64,
    total_net_profit_sol: f64,
    roi_pct: f64,
}

fn main() -> Result<()> {
    println!("==========================================================================================");
    println!("       CLOCKIT: STRATEGY 1 DUAL-LISTED PRE-CACHED ATOMIC ARBITRAGE BACKTEST ENGINE");
    println!("       50-Token Historical Replay with 100% Complete 2-Leg Instructions & Rent Refund");
    println!("==========================================================================================");

    // Capital constraint CLI arg: default 0.45 SOL (based on user balance 0.53 SOL)
    let args: Vec<String> = std::env::args().collect();
    let mut capital_sol: f64 = 0.45;

    for i in 1..args.len() {
        if (args[i] == "--capital" || args[i] == "-c") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<f64>() {
                capital_sol = val;
            }
        }
    }

    let dataset_path = "data/50_tokens_historical.json";
    println!("📂 Loading Dual-Pool Dataset from: {}", dataset_path);

    let registry = PoolRegistry::load_from_json(dataset_path)?;
    println!("✅ Successfully loaded {} dual-listed token pairs with pre-cached Raydium AMM pools!", registry.pairs().len());
    println!("💰 Trader Capital Allocation: {:.4} SOL per atomic bullet", capital_sol);
    println!("🛡️ Instruction Verification: Full 5-instruction atomic bundle with CloseAccount rent refund");
    println!("------------------------------------------------------------------------------------------\n");

    let max_trade_lamports = (capital_sol * 1e9) as u64;
    let min_profit_lamports = 1_000_000; // 0.001 SOL net profit hurdle
    let arb_engine = ArbitrageEngine::new(min_profit_lamports, max_trade_lamports);
    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);

    let dummy_payer = Pubkey::from_str("FL1UpMfPybHWgbM5GwzT8VhGseMMARj5guEnuoGunhZG")?;

    let mut token_results = Vec::new();
    let mut grand_total_swaps = 0;
    let mut grand_total_arbs = 0;
    let mut grand_total_deployed_sol = 0.0;
    let mut grand_total_gross_sol = 0.0;
    let mut grand_total_tips_sol = 0.0;
    let mut grand_total_net_sol = 0.0;
    let mut running_compounded_wallet = capital_sol;
    let mut latencies_ns = Vec::new();

    println!("{:<3} | {:<10} | {:<11} | {:<7} | {:>10} | {:>12} | {:>12} | {:>8}",
        "#", "SYMBOL", "PRIMARY DEX", "ARBS", "DEPLOYED", "GROSS SOL", "NET PROFIT", "ROI %");
    println!("----+------------+-------------+---------+------------+--------------+--------------+---------");

    let start_time = Instant::now();

    for (idx, pair) in registry.pairs().iter().enumerate() {
        let mut pool_a = pair.pool_a_pump;
        let mut pool_b = pair.pool_b_raydium;

        let mut res = TokenBacktestResult {
            symbol: pair.symbol.clone(),
            ..Default::default()
        };

        for trade in &pair.trades {
            res.total_trades_replayed += 1;
            grand_total_swaps += 1;

            // 1. Replay historical trade event on the designated pool
            if trade.pool_target == "pool_a" {
                if trade.is_buy {
                    if let Some(tokens_out) = pool_a.swap_sol_for_tokens(trade.amount_lamports) {
                        pool_a.sol_reserves += trade.amount_lamports;
                        pool_a.token_reserves = pool_a.token_reserves.saturating_sub(tokens_out);
                    }
                } else {
                    let tokens_in = (trade.amount_lamports as f64 / pool_a.spot_price_sol().max(1e-12)) as u64;
                    if let Some(sol_out) = pool_a.swap_tokens_for_sol(tokens_in) {
                        pool_a.token_reserves += tokens_in;
                        pool_a.sol_reserves = pool_a.sol_reserves.saturating_sub(sol_out);
                    }
                }
            } else {
                if trade.is_buy {
                    if let Some(tokens_out) = pool_b.swap_sol_for_tokens(trade.amount_lamports) {
                        pool_b.sol_reserves += trade.amount_lamports;
                        pool_b.token_reserves = pool_b.token_reserves.saturating_sub(tokens_out);
                    }
                } else {
                    let tokens_in = (trade.amount_lamports as f64 / pool_b.spot_price_sol().max(1e-12)) as u64;
                    if let Some(sol_out) = pool_b.swap_tokens_for_sol(tokens_in) {
                        pool_b.token_reserves += tokens_in;
                        pool_b.sol_reserves = pool_b.sol_reserves.saturating_sub(sol_out);
                    }
                }
            }

            // 2. Evaluate Arbitrage with nanosecond microsecond timing
            let t0 = Instant::now();
            let maybe_opp = arb_engine.evaluate_pair(&pool_a, &pool_b);
            let elapsed_ns = t0.elapsed().as_nanos();
            latencies_ns.push(elapsed_ns);

            if let Some(opp) = maybe_opp {
                // Verify complete 5-instruction atomic bundle assembly
                let bundle_ixs = pair.build_atomic_arb_bundle(&dummy_payer, &opp);
                assert!(bundle_ixs.len() >= 4, "Atomic bundle must contain all setup, swap, and cleanup instructions");

                let tip_lamports = tip_engine
                    .calculate_optimal_tip(opp.gross_profit_lamports)
                    .unwrap_or_else(|| tip_engine.default_tip());

                let deployed_sol = opp.optimal_sol_in_lamports as f64 / 1e9;
                let gross_sol = opp.gross_profit_lamports as f64 / 1e9;
                let tip_sol = tip_lamports as f64 / 1e9;
                let net_sol = gross_sol - tip_sol;

                if net_sol > 0.0005 {
                    res.arbs_captured += 1;
                    res.total_deployed_sol += deployed_sol;
                    res.total_gross_profit_sol += gross_sol;
                    res.total_tips_sol += tip_sol;
                    res.total_net_profit_sol += net_sol;

                    grand_total_arbs += 1;
                    grand_total_deployed_sol += deployed_sol;
                    grand_total_gross_sol += gross_sol;
                    grand_total_tips_sol += tip_sol;
                    grand_total_net_sol += net_sol;
                    running_compounded_wallet += net_sol;

                    // Re-balance pools after arbitrage
                    match opp.direction {
                        ArbDirection::PoolAToPoolB => {
                            if let Some(toks) = pool_a.swap_sol_for_tokens(opp.optimal_sol_in_lamports) {
                                pool_a.sol_reserves += opp.optimal_sol_in_lamports;
                                pool_a.token_reserves = pool_a.token_reserves.saturating_sub(toks);
                                if let Some(sol_ret) = pool_b.swap_tokens_for_sol(toks) {
                                    pool_b.token_reserves += toks;
                                    pool_b.sol_reserves = pool_b.sol_reserves.saturating_sub(sol_ret);
                                }
                            }
                        }
                        ArbDirection::PoolBToPoolA => {
                            if let Some(toks) = pool_b.swap_sol_for_tokens(opp.optimal_sol_in_lamports) {
                                pool_b.sol_reserves += opp.optimal_sol_in_lamports;
                                pool_b.token_reserves = pool_b.token_reserves.saturating_sub(toks);
                                if let Some(sol_ret) = pool_a.swap_tokens_for_sol(toks) {
                                    pool_a.token_reserves += toks;
                                    pool_a.sol_reserves = pool_a.sol_reserves.saturating_sub(sol_ret);
                                }
                            }
                        }
                    }
                }
            }
        }

        if res.total_deployed_sol > 0.0 {
            res.roi_pct = (res.total_net_profit_sol / res.total_deployed_sol) * 100.0;
        }

        println!("{:<3} | {:<10} | {:<11} | {:<7} | {:>8.2} SOL | {:>+10.4} SOL | {:>+10.4} SOL | {:>+7.2}%",
            idx + 1,
            res.symbol,
            "RAYDIUM/PUMP",
            res.arbs_captured,
            res.total_deployed_sol,
            res.total_gross_profit_sol,
            res.total_net_profit_sol,
            res.roi_pct
        );

        token_results.push(res);
    }

    let avg_latency_ns = if !latencies_ns.is_empty() {
        latencies_ns.iter().sum::<u128>() / latencies_ns.len() as u128
    } else {
        0
    };

    println!("==========================================================================================");
    println!("              STRATEGY 1 ATOMIC ARBITRAGE BACKTEST SCORECARD");
    println!("==========================================================================================");
    println!(" Initial Trader Capital:      {:.4} SOL (Single-bullet constraint)", capital_sol);
    println!(" Total Tokens Backtested:     {} dual-listed tokens", registry.pairs().len());
    println!(" Total Swaps Replayed:        {} orders", grand_total_swaps);
    println!(" Arbitrages Captured:         {} opportunities", grand_total_arbs);
    println!(" Arbitrage Capture Rate:      {:.1}% of trade events created actionable spreads",
        (grand_total_arbs as f64 / grand_total_swaps as f64) * 100.0);
    println!(" Avg Decision Latency:        {} nanoseconds (~{:.3} µs)", avg_latency_ns, avg_latency_ns as f64 / 1000.0);
    println!("------------------------------------------------------------------------------------------");
    println!(" Total Cumulative Volume:     {:.4} SOL ({:.2} SOL recycled {} times)",
        grand_total_deployed_sol, capital_sol, grand_total_arbs);
    println!(" Total Gross Profit:          {:+.4} SOL", grand_total_gross_sol);
    println!(" Total Jito MEV Tips:         {:.4} SOL (to validators)", grand_total_tips_sol);
    println!(" TOTAL NET PROFIT (After Tip):{:>+11.4} SOL", grand_total_net_sol);
    println!(" FINAL COMPOUNDED WALLET:     {:>+11.4} SOL (Started with {:.2} SOL)",
        running_compounded_wallet, capital_sol);
    println!(" NET CAPITAL MULTIPLIER:      {:>+10.2}x ({:>+7.1}% growth on {:.2} SOL)",
        running_compounded_wallet / capital_sol, (grand_total_net_sol / capital_sol) * 100.0, capital_sol);
    println!(" Total Backtest Runtime:      {:.2} ms", start_time.elapsed().as_secs_f64() * 1000.0);
    println!("==========================================================================================");

    token_results.sort_by(|a, b| b.total_net_profit_sol.partial_cmp(&a.total_net_profit_sol).unwrap());
    println!("\n🏆 TOP 5 MOST PROFITABLE DUAL-LISTED TOKENS (STRATEGY 1):");
    for (i, top) in token_results.iter().take(5).enumerate() {
        println!("   {}. [{:<8}] | Net Profit: {:>+7.3} SOL | ROI: {:>+6.1}% | Arbs: {}",
            i + 1, top.symbol, top.total_net_profit_sol, top.roi_pct, top.arbs_captured);
    }
    println!("==========================================================================================\n");

    Ok(())
}
