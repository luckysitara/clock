use anyhow::{Context, Result};
use solana_sdk::signer::Signer;
use std::path::Path;
use std::sync::Arc;

use clockit::{
    config::BotConfig,
    engine::{
        position_manager::load_saved_positions,
        wallet::{fetch_bonding_curve, fetch_token_holdings},
    },
};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();

    let config = Arc::new(BotConfig::load_from_env().context("Failed to load .env")?);
    let keypair = config.load_keypair().context("Failed to load keypair")?;
    let wallet_pubkey = keypair.pubkey();

    println!("🔑 Trader Wallet: {}", wallet_pubkey);

    let http_client = reqwest::Client::new();
    let rpc_url = config.solana_rpc_url.clone();
    let saved = load_saved_positions(Path::new(&config.positions_file));

    let holdings = fetch_token_holdings(&http_client, &rpc_url, &wallet_pubkey)
        .await
        .context("Failed to get token accounts")?;

    if holdings.is_empty() {
        println!("No open token positions found in wallet.");
    } else {
        println!("\nFound {} token balance(s):", holdings.len());
    }

    for holding in holdings {
        let mint = holding.mint;
        let ui_amount = holding.amount as f64 / 1e6;

        let curve_pod = match fetch_bonding_curve(&http_client, &rpc_url, &mint).await {
            Ok(Some(pod)) => pod,
            Ok(None) => {
                println!("------------------------------------------------------------");
                println!("ℹ️  {} ({:.2} tokens): not a Pump.fun bonding curve token", mint, ui_amount);
                continue;
            }
            Err(e) => {
                println!("⚠️ Failed to fetch curve data for mint {}: {:#}", mint, e);
                continue;
            }
        };

        let spot_price = curve_pod.current_spot_price_sol();
        let real_sol = curve_pod.real_sol_reserves as f64 / 1e9;
        // What selling the whole bag into the curve would return right now
        let current_value_sol = curve_pod
            .calculate_sell_output(holding.amount, 0)
            .map(|c| c.sol_out as f64 / 1e9)
            .unwrap_or(0.0);

        println!("------------------------------------------------------------");
        println!("🚀 Position Mint: {}", mint);
        println!("   Tokens Held:   {:.2} (raw: {}) | Token Prog: {}", ui_amount, holding.amount, holding.token_program);
        println!("   Curve Spot:    {:.10} SOL", spot_price);
        println!("   Real SOL Res:  {:.4} SOL", real_sol);
        match saved.get(&mint) {
            Some(rec) => {
                let invested = rec.sol_invested_lamports as f64 / 1e9;
                let pnl_pct = if invested > 0.0 { (current_value_sol - invested) / invested * 100.0 } else { 0.0 };
                println!(
                    "   Sell Value:    {:.4} SOL (Entry: {:.4} SOL @ {:.10} | P&L: {:+.2}%)",
                    current_value_sol, invested, rec.entry_price_sol, pnl_pct
                );
            }
            None => println!("   Sell Value:    {:.4} SOL (no entry record in {})", current_value_sol, config.positions_file),
        }
        if curve_pod.complete {
            println!("   Curve Prog:    🎓 graduated (sell on PumpSwap/Raydium)");
        } else {
            println!("   Curve Prog:    {:.2}% complete", curve_pod.graduation_progress_pct());
        }
    }

    // Check SOL balance
    let bal_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [wallet_pubkey.to_string()]
    });
    let bal_resp = http_client.post(&rpc_url).json(&bal_payload).send().await?.json::<serde_json::Value>().await?;
    let sol_lamports = bal_resp["result"]["value"].as_u64().unwrap_or(0);
    println!("------------------------------------------------------------");
    println!("💰 Liquid SOL Balance: {:.4} SOL", sol_lamports as f64 / 1e9);

    Ok(())
}
