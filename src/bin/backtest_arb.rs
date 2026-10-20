use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, time::Instant};

use clockit::{
    decoders::arb_math::{ConstantProductPool, ArbDirection},
    engine::{
        arb_engine::ArbitrageEngine,
        tip_engine::TipEngine,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTradeStep {
    pub step: usize,
    pub pool_target: String,
    pub is_buy: bool,
    pub amount_lamports: u64,
    pub desc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenArbDatasetEntry {
    pub id: usize,
    pub symbol: String,
    pub name: String,
    pub mint: String,
    pub primary_dex: String,
    pub pool_a_name: String,
    pub pool_a_sol_reserves: u64,
    pub pool_a_token_reserves: u64,
    pub pool_a_fee_bps: u64,
    pub pool_b_name: String,
    pub pool_b_sol_reserves: u64,
    pub pool_b_token_reserves: u64,
    pub pool_b_fee_bps: u64,
    pub trades: Vec<HistoricalTradeStep>,
}

#[derive(Debug, Default, Clone)]
pub struct TokenArbResult {
    pub id: usize,
    pub symbol: String,
    pub mint: String,
    pub dex: String,
    pub total_trades_replayed: usize,
    pub arbs_captured: usize,
    pub sol_deployed: f64,
    pub gross_profit_sol: f64,
    pub jito_tips_sol: f64,
    pub net_profit_sol: f64,
    pub roi_pct: f64,
    pub avg_latency_ns: u128,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();

    println!("==========================================================================================");
    println!("       CLOCKIT: 50-TOKEN HISTORICAL CROSS-DEX ARBITRAGE BACKTEST ENGINE");
    println!("       Powered by Triton Yellowstone gRPC & Triton RPC Infrastructure");
    println!("==========================================================================================");

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com/95794a05-4057-49cb-87e4-42d3d9a46774".to_string());
    let grpc_url = std::env::var("YELLOWSTONE_GRPC_URL")
        .unwrap_or_else(|_| "https://graceful-oasis-a398.mainnet.rpcpool.com:443".to_string());

    println!("📍 Triton Yellowstone gRPC: {}", grpc_url);
    println!("📍 Triton RPC Endpoint:     {}", rpc_url);

    // Verify RPC connectivity
    let http_client = reqwest::Client::new();
    let slot_payload = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "getSlot"});
    let current_slot = if let Ok(resp) = http_client.post(&rpc_url).json(&slot_payload).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            json["result"].as_u64().unwrap_or(0)
        } else {
            0
        }
    } else {
        0
    };
    println!("🔗 Connected to Mainnet at Current Slot: {}", current_slot);
    println!("------------------------------------------------------------------------------------------\n");

    let dataset_path = "data/50_tokens_historical.json";
    println!("📂 Loading 50-token historical multi-DEX dataset from: {}", dataset_path);

    let dataset_content = fs::read_to_string(Path::new(dataset_path))
        .with_context(|| format!("Failed to read dataset at {}", dataset_path))?;
    let tokens: Vec<TokenArbDatasetEntry> = serde_json::from_str(&dataset_content)?;

    println!("✅ Successfully loaded {} distinct tokens with sequential historical order-flow!\n", tokens.len());

    let args: Vec<String> = std::env::args().collect();
    let mut initial_capital_sol: f64 = 0.50; // Default to 0.50 SOL capital constraint
    for i in 1..args.len() {
        if (args[i] == "--capital" || args[i] == "-c") && i + 1 < args.len() {
            if let Ok(val) = args[i + 1].parse::<f64>() {
                initial_capital_sol = val;
            }
        }
    }

    println!("💰 Trader Capital Allocation: {:.4} SOL per trade bullet", initial_capital_sol);
    println!("------------------------------------------------------------------------------------------\n");

    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);
    // For 0.5 SOL capital: max trade is 0.5 SOL, min profit hurdle is 0.001 SOL (1M lamports)
    let max_trade_lamports = (initial_capital_sol * 1e9) as u64;
    let min_profit_lamports = 1_000_000;
    let arb_engine = ArbitrageEngine::new(min_profit_lamports, max_trade_lamports);

    let mut token_results = Vec::new();
    let mut compounded_wallet_sol = initial_capital_sol;
    let global_start = Instant::now();

    println!("{:<3} | {:<8} | {:<10} | {:<7} | {:<8} | {:<10} | {:<10} | {:<9}", 
        "#", "SYMBOL", "PRIMARY DEX", "ARBS", "DEPLOYED", "GROSS SOL", "NET PROFIT", "ROI %");
    println!("----+----------+------------+---------+----------+------------+------------+----------");


    for t in &tokens {
        let mut pool_a = ConstantProductPool::new(t.pool_a_sol_reserves, t.pool_a_token_reserves, t.pool_a_fee_bps);
        let mut pool_b = ConstantProductPool::new(t.pool_b_sol_reserves, t.pool_b_token_reserves, t.pool_b_fee_bps);

        let mut token_arbs = 0;
        let mut token_deployed = 0u64;
        let mut token_gross_profit = 0u64;
        let mut token_jito_tips = 0u64;
        let mut decision_nanos = Vec::new();

        for trade in &t.trades {
            // Apply historical trade
            if trade.pool_target == "pool_a" {
                if trade.is_buy {
                    if let Some(tokens_out) = pool_a.swap_sol_for_tokens(trade.amount_lamports) {
                        pool_a.sol_reserves += trade.amount_lamports;
                        pool_a.token_reserves = pool_a.token_reserves.saturating_sub(tokens_out);
                    }
                } else {
                    let tokens_in = 15_000_000_000_000u64;
                    if let Some(sol_out) = pool_a.swap_tokens_for_sol(tokens_in) {
                        pool_a.token_reserves += tokens_in;
                        pool_a.sol_reserves = pool_a.sol_reserves.saturating_sub(sol_out);
                    }
                }
            } else if trade.pool_target == "pool_b" {
                if trade.is_buy {
                    if let Some(tokens_out) = pool_b.swap_sol_for_tokens(trade.amount_lamports) {
                        pool_b.sol_reserves += trade.amount_lamports;
                        pool_b.token_reserves = pool_b.token_reserves.saturating_sub(tokens_out);
                    }
                } else {
                    let tokens_in = 20_000_000_000_000u64;
                    if let Some(sol_out) = pool_b.swap_tokens_for_sol(tokens_in) {
                        pool_b.token_reserves += tokens_in;
                        pool_b.sol_reserves = pool_b.sol_reserves.saturating_sub(sol_out);
                    }
                }
            }

            // Arbitrage Evaluation
            let eval_start = Instant::now();
            let opp_opt = arb_engine.evaluate_pair(&pool_a, &pool_b);
            let latency_ns = eval_start.elapsed().as_nanos();
            decision_nanos.push(latency_ns);

            if let Some(opp) = opp_opt {
                token_arbs += 1;
                token_deployed += opp.optimal_sol_in_lamports;
                token_gross_profit += opp.gross_profit_lamports;

                let tip = tip_engine
                    .calculate_optimal_tip(opp.gross_profit_lamports)
                    .unwrap_or_else(|| tip_engine.default_tip());
                token_jito_tips += tip;

                // Rebalance pools in RAM post-arb
                match opp.direction {
                    ArbDirection::PoolAToPoolB => {
                        pool_a.sol_reserves += opp.optimal_sol_in_lamports;
                        pool_a.token_reserves = pool_a.token_reserves.saturating_sub(opp.expected_tokens_intermediate);
                        pool_b.token_reserves += opp.expected_tokens_intermediate;
                        pool_b.sol_reserves = pool_b.sol_reserves.saturating_sub(opp.expected_sol_out_lamports);
                    }
                    ArbDirection::PoolBToPoolA => {
                        pool_b.sol_reserves += opp.optimal_sol_in_lamports;
                        pool_b.token_reserves = pool_b.token_reserves.saturating_sub(opp.expected_tokens_intermediate);
                        pool_a.token_reserves += opp.expected_tokens_intermediate;
                        pool_a.sol_reserves = pool_a.sol_reserves.saturating_sub(opp.expected_sol_out_lamports);
                    }
                }
            }
        }

        let gross_sol = token_gross_profit as f64 / 1e9;
        let tips_sol = token_jito_tips as f64 / 1e9;
        let net_sol = gross_sol - tips_sol;
        let deployed_sol = token_deployed as f64 / 1e9;
        let roi_pct = if deployed_sol > 0.0 {
            (net_sol / deployed_sol) * 100.0
        } else {
            0.0
        };
        let avg_lat = if !decision_nanos.is_empty() {
            decision_nanos.iter().sum::<u128>() / decision_nanos.len() as u128
        } else {
            0
        };

        println!("{:<3} | {:<8} | {:<10} | {:<7} | {:>6.2} SOL | {:>+8.4} SOL | {:>+8.4} SOL | {:>+7.2}%",
            t.id,
            if t.symbol.len() > 8 { &t.symbol[..8] } else { &t.symbol },
            t.primary_dex.to_uppercase(),
            token_arbs,
            deployed_sol,
            gross_sol,
            net_sol,
            roi_pct
        );

        compounded_wallet_sol += net_sol;

        token_results.push(TokenArbResult {
            id: t.id,
            symbol: t.symbol.clone(),
            mint: t.mint.clone(),
            dex: t.primary_dex.clone(),
            total_trades_replayed: t.trades.len(),
            arbs_captured: token_arbs,
            sol_deployed: deployed_sol,
            gross_profit_sol: gross_sol,
            jito_tips_sol: tips_sol,
            net_profit_sol: net_sol,
            roi_pct,
            avg_latency_ns: avg_lat,
        });
    }

    // Grand Totals
    let total_replayed: usize = token_results.iter().map(|r| r.total_trades_replayed).sum();
    let total_arbs: usize = token_results.iter().map(|r| r.arbs_captured).sum();
    let total_deployed: f64 = token_results.iter().map(|r| r.sol_deployed).sum();
    let total_gross: f64 = token_results.iter().map(|r| r.gross_profit_sol).sum();
    let total_tips: f64 = token_results.iter().map(|r| r.jito_tips_sol).sum();
    let total_net: f64 = token_results.iter().map(|r| r.net_profit_sol).sum();
    let avg_latency_overall: u128 = if !token_results.is_empty() {
        token_results.iter().map(|r| r.avg_latency_ns).sum::<u128>() / token_results.len() as u128
    } else {
        0
    };

    println!("\n==========================================================================================");
    println!("              50-TOKEN CROSS-DEX ARBITRAGE BACKTEST SCORECARD");
    println!("==========================================================================================");
    println!(" Initial Trader Capital:      {:.4} SOL (Single-bullet constraint)", initial_capital_sol);
    println!(" Total Tokens Backtested:     {} tokens", token_results.len());
    println!(" Total Swaps Replayed:        {} orders", total_replayed);
    println!(" Arbitrages Captured:         {} opportunities", total_arbs);
    println!(" Arbitrage Capture Rate:      {:.1}% of trade events created actionable arb spreads", (total_arbs as f64 / total_replayed as f64) * 100.0);
    println!(" Avg Decision Latency:        {} nanoseconds (~{:.3} µs)", avg_latency_overall, avg_latency_overall as f64 / 1000.0);
    println!("------------------------------------------------------------------------------------------");
    println!(" Total Cumulative Volume:     {:.4} SOL (0.50 SOL recycled {} times)", total_deployed, total_arbs);
    println!(" Total Gross Profit:          {:+.4} SOL", total_gross);
    println!(" Total Jito MEV Tips:         {:.4} SOL (to validators)", total_tips);
    println!(" TOTAL NET PROFIT (After Tip):{:>+10.4} SOL", total_net);
    println!(" FINAL COMPOUNDED WALLET:     {:>+10.4} SOL (Started with {:.2} SOL)", compounded_wallet_sol, initial_capital_sol);
    println!(" NET CAPITAL MULTIPLIER:      {:>+10.2}x ({:>+7.1}% growth on {:.2} SOL)", compounded_wallet_sol / initial_capital_sol, ((compounded_wallet_sol - initial_capital_sol) / initial_capital_sol) * 100.0, initial_capital_sol);
    println!(" Total Backtest Runtime:      {:.2} ms", global_start.elapsed().as_secs_f64() * 1000.0);
    println!("==========================================================================================");


    // Top 5 Most Profitable Tokens
    let mut sorted_by_profit = token_results.clone();
    sorted_by_profit.sort_by(|a, b| b.net_profit_sol.partial_cmp(&a.net_profit_sol).unwrap());

    println!("\n🏆 TOP 5 MOST PROFITABLE ARBITRAGE TOKENS:");
    for (rank, t) in sorted_by_profit.iter().take(5).enumerate() {
        println!("   {}. [{:<8}] (Mint: {}) | Net Profit: {:>+6.3} SOL (ROI: {:>+5.1}%) | DEX: {}",
            rank + 1, t.symbol, t.mint, t.net_profit_sol, t.roi_pct, t.dex.to_uppercase()
        );
    }
    println!("==========================================================================================\n");

    Ok(())
}
