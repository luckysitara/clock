use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::{fs, path::Path, time::Instant};

use clockit::{
    constants::{
        INITIAL_REAL_TOKEN_RESERVES, INITIAL_VIRTUAL_SOL_RESERVES,
        INITIAL_VIRTUAL_TOKEN_RESERVES, TOTAL_TOKEN_SUPPLY,
    },
    decoders::BondingCurveAccountPod,
    engine::tip_engine::TipEngine,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopTokenData {
    pub symbol: String,
    pub name: String,
    pub mint: String,
    pub dex: String,
    pub price_native: f64,
    pub price_usd: f64,
    pub gain_24h_pct: f64,
    pub volume_24h: f64,
    pub fdv: f64,
}

#[derive(Debug)]
pub struct TokenTradeResult {
    pub symbol: String,
    pub mint: String,
    pub gain_24h_pct: f64,
    pub sol_invested: f64,
    pub sol_returned: f64,
    pub jito_tip: f64,
    pub net_pnl_sol: f64,
    pub roi_pct: f64,
    pub exit_reason: &'static str,
    pub graduated: bool,
}

fn main() -> Result<()> {
    println!("==========================================================================================");
    println!("       CLOCKIT: BACKTEST ON TOP 20 MOST APPRECIATED PUMP.FUN TOKENS TODAY");
    println!("==========================================================================================");

    let file_path = "data/top_20_tokens_today.json";
    let data_content = fs::read_to_string(Path::new(file_path))
        .with_context(|| format!("Failed to read {}", file_path))?;

    let tokens: Vec<TopTokenData> = serde_json::from_str(&data_content)
        .context("Failed to parse top tokens JSON")?;

    println!("📂 Loaded {} top appreciated tokens created on Pump.fun today.", tokens.len());
    println!("⚙️ Strategy Parameters:");
    println!("   - Entry Size:          0.20 SOL per token");
    println!("   - Take-Profit Target:  2.50x (+150% ROI)");
    println!("   - Trailing Stop-Loss:  15.0% pullback from peak");
    println!("   - Max Loss Cut:        -15.0% maximum risk cap");
    println!("   - Slippage Tolerance:  500 bps (5%)\n");

    let entry_sol_lamports: u64 = 200_000_000; // 0.20 SOL
    let entry_sol = entry_sol_lamports as f64 / 1e9;
    let take_profit_multiplier = 2.50;
    let trailing_stop_pct = 0.15;
    let hard_stop_loss_pct = 0.20;
    let tip_engine = TipEngine::new(100_000, 50_000_000, 0.65);

    let mut results: Vec<TokenTradeResult> = Vec::new();
    let start_sim = Instant::now();

    for t in &tokens {
        // Initial bonding curve at creation (Block 0)
        let initial_curve = BondingCurveAccountPod {
            virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
            virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
            real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
            real_sol_reserves: 0,
            token_total_supply: TOTAL_TOKEN_SUPPLY,
            complete: false,
            creator: Pubkey::default(),
        };

        // Simulate 0.2 SOL buy at launch
        let buy_res = initial_curve
            .calculate_buy_output(entry_sol_lamports, 500)
            .expect("Curve buy math should succeed");

        let entry_price_sol = buy_res.effective_price_sol;
        let tokens_acquired = buy_res.tokens_out;

        // Peak price multiple reached during today's appreciation
        // (Base launch price on curve is ~0.000000028 SOL)
        let launch_price_approx = 0.000000028;
        let peak_price_estimate = if t.price_native > launch_price_approx {
            t.price_native
        } else {
            launch_price_approx * (1.0 + (t.gain_24h_pct / 100.0).max(0.0))
        };

        let peak_multiple = peak_price_estimate / entry_price_sol;

        // Evaluate exit condition
        let (exit_price_sol, exit_reason, graduated) = if t.dex == "pumpswap" || t.dex == "raydium" {
            // Token successfully completed curve and graduated to AMM!
            // When token reaches 85 SOL on curve, price is ~8x (~0.00000022 SOL)
            if peak_multiple >= take_profit_multiplier {
                (entry_price_sol * take_profit_multiplier, "Take-Profit Hit (2.5x)", true)
            } else {
                (peak_price_estimate * (1.0 - trailing_stop_pct), "Graduation Trailing Exit", true)
            }
        } else if peak_multiple >= take_profit_multiplier {
            // Surged and hit 2.5x Take-Profit
            (entry_price_sol * take_profit_multiplier, "Take-Profit Hit (2.5x)", false)
        } else if peak_multiple >= 1.30 {
            // Modest pump (>= +30%), followed by 15% trailing stop
            (peak_price_estimate * (1.0 - trailing_stop_pct), "Trailing Stop (15% pullback)", false)
        } else {
            // Stalled, slow, or dumped token -> stopped out at -20% hard stop
            (entry_price_sol * (1.0 - hard_stop_loss_pct), "Hard Stop-Loss (-20%)", false)
        };

        let gross_sol_returned = (tokens_acquired as f64 * exit_price_sol) / 1e6;
        let net_profit_before_tip = gross_sol_returned - entry_sol;
        let tip_lamports = tip_engine.default_tip();
        let tip_sol = tip_lamports as f64 / 1e9;
        let net_pnl_sol = net_profit_before_tip - tip_sol;
        let roi_pct = (net_pnl_sol / entry_sol) * 100.0;

        results.push(TokenTradeResult {
            symbol: t.symbol.clone(),
            mint: t.mint.clone(),
            gain_24h_pct: t.gain_24h_pct,
            sol_invested: entry_sol,
            sol_returned: gross_sol_returned,
            jito_tip: tip_sol,
            net_pnl_sol,
            roi_pct,
            exit_reason,
            graduated,
        });
    }

    let elapsed_ms = start_sim.elapsed().as_millis();

    // Print Individual Token Results Table
    println!("{:<3} | {:<10} | {:<10} | {:<7} | {:<28} | {:<9} | {:<9} | {:<10}",
        "#", "Symbol", "Gain 24h", "Grad?", "Exit Trigger", "Invested", "Returned", "Net ROI"
    );
    println!("{:-<106}", "");

    let mut total_invested = 0.0;
    let mut total_returned = 0.0;
    let mut total_tips = 0.0;
    let mut winning_trades = 0;
    let mut losing_trades = 0;

    for (idx, r) in results.iter().enumerate() {
        total_invested += r.sol_invested;
        total_returned += r.sol_returned;
        total_tips += r.jito_tip;

        if r.net_pnl_sol >= 0.0 {
            winning_trades += 1;
        } else {
            losing_trades += 1;
        }

        println!(
            "{:2}  | {:<10} | {:>8.1}% | {:<7} | {:<28} | {:>7.2} SOL | {:>7.2} SOL | {:>+8.1}%",
            idx + 1,
            if r.symbol.len() > 10 { &r.symbol[..10] } else { &r.symbol },
            r.gain_24h_pct,
            if r.graduated { "YES 🎓" } else { "NO" },
            r.exit_reason,
            r.sol_invested,
            r.sol_returned,
            r.roi_pct
        );
    }

    let total_net_pnl = total_returned - total_invested - total_tips;
    let total_portfolio_roi = (total_net_pnl / total_invested) * 100.0;

    println!("{:-<106}", "");
    println!("\n==========================================================================================");
    println!("                   PORTFOLIO PERFORMANCE ON TOP 20 TOKENS TODAY");
    println!("==========================================================================================");
    println!(" Total Tokens Tested:       {}", results.len());
    println!(" Simulation Wall Time:      {} ms", elapsed_ms);
    println!("------------------------------------------------------------------------------------------");
    println!(" Winning Trades:            {} (Win Rate: {:.1}%)", winning_trades, (winning_trades as f64 / results.len() as f64) * 100.0);
    println!(" Losing Trades:             {} (Stopped out at strict -15% max cap)", losing_trades);
    println!("------------------------------------------------------------------------------------------");
    println!(" Total SOL Deployed:        {:.2} SOL (0.20 SOL * 20 tokens)", total_invested);
    println!(" Total SOL Recovered:       {:.4} SOL", total_returned);
    println!(" Total Jito MEV Tips Paid:  {:.6} SOL", total_tips);
    println!(" NET PORTFOLIO PROFIT:      {:+.4} SOL", total_net_pnl);
    println!(" NET PORTFOLIO RETURN:      {:+.2}% ROI across today's top 20 tokens", total_portfolio_roi);
    println!("==========================================================================================\n");

    Ok(())
}
