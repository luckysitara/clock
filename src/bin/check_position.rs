use anyhow::{Context, Result};
use base64::Engine;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use std::str::FromStr;
use std::sync::Arc;

use clockit::{
    config::BotConfig,
    constants::derive_bonding_curve,
    decoders::BondingCurveAccountPod,
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

    // Query token accounts
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountsByOwner",
        "params": [
            wallet_pubkey.to_string(),
            { "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" },
            { "encoding": "jsonParsed" }
        ]
    });

    let resp = http_client.post(&rpc_url).json(&payload).send().await?.json::<serde_json::Value>().await?;
    let accounts = resp["result"]["value"].as_array().cloned().unwrap_or_default();

    let mut open_positions = Vec::new();
    for acc in accounts {
        let info = &acc["account"]["data"]["parsed"]["info"];
        let mint_str = info["mint"].as_str().unwrap_or_default();
        let amount_str = info["tokenAmount"]["amount"].as_str().unwrap_or("0");
        let ui_amount = info["tokenAmount"]["uiAmount"].as_f64().unwrap_or(0.0);
        let amount: u64 = amount_str.parse().unwrap_or(0);

        if amount > 0 {
            let mint = Pubkey::from_str(mint_str)?;
            open_positions.push((mint, amount, ui_amount));
        }
    }

    if open_positions.is_empty() {
        println!("No open token positions found in wallet.");
        return Ok(());
    }

    println!("\nFound {} open position(s):", open_positions.len());
    for (mint, amount, ui_amount) in open_positions {
        let (curve_pda, _) = derive_bonding_curve(&mint);

        let pda_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                curve_pda.to_string(),
                { "encoding": "base64" }
            ]
        });

        let pda_resp = http_client.post(&rpc_url).json(&pda_payload).send().await?.json::<serde_json::Value>().await?;
        let data_b64 = pda_resp["result"]["value"]["data"][0].as_str().unwrap_or_default();
        let curve_data = base64::engine::general_purpose::STANDARD.decode(data_b64)?;

        if let Some(curve_pod) = BondingCurveAccountPod::read_from_account(&curve_data) {
            let spot_price = curve_pod.current_spot_price_sol();
            let real_sol = curve_pod.real_sol_reserves as f64 / 1e9;
            let current_value_sol = ui_amount * spot_price;
            let pnl_pct = ((current_value_sol - 0.20) / 0.20) * 100.0;

            println!("------------------------------------------------------------");
            println!("🚀 Position Mint: {}", mint);
            println!("   Tokens Held:   {:.2} (raw: {})", ui_amount, amount);
            println!("   Curve Spot:    {:.10} SOL", spot_price);
            println!("   Real SOL Res:  {:.4} SOL", real_sol);
            println!("   Estimated Val: {:.4} SOL (Entry: 0.2000 SOL | P&L: {:+.2}%)", current_value_sol, pnl_pct);
            println!("   Curve Prog:    {:.2}% complete", (real_sol / 85.0) * 100.0);
        } else {
            println!("⚠️ Failed to parse curve data for mint: {}", mint);
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
