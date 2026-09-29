use anyhow::{Context, Result};
use base64::Engine;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use clockit::{
    config::BotConfig,
    constants::derive_bonding_curve,
    decoders::BondingCurveAccountPod,
    engine::{
        blockhash_cache::BlockhashCache,
        executor::{ExecutionEngine, TradeAction},
        jito::JitoClient,
        position_manager::PositionManager,
        tip_engine::TipEngine,
    },
};

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    println!("============================================================");
    println!("     CLOCKIT EMERGENCY / MANUAL POSITION EXIT TOOL");
    println!("============================================================");

    let config = Arc::new(BotConfig::load_from_env().context("Failed to load .env")?);
    let keypair = config.load_keypair().context("Failed to load keypair")?;
    let wallet_pubkey = keypair.pubkey();

    println!("🔑 Trader Wallet: {}", wallet_pubkey);

    let blockhash_cache = Arc::new(BlockhashCache::new());
    blockhash_cache.spawn_updater(config.solana_rpc_url.clone(), Duration::from_millis(1500));
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let tip_engine = Arc::new(TipEngine::new(
        config.min_tip_lamports,
        config.max_tip_lamports,
        config.tip_aggression_factor,
    ));

    let jito_client = Arc::new(JitoClient::new(config.jito_block_engine_url.clone()));
    jito_client.spawn_prewarmer();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let position_manager = Arc::new(PositionManager::new(
        config.trailing_stop_pct,
        config.hard_stop_loss_pct,
        config.take_profit_pct,
        config.dev_dump_threshold_pct,
    ));

    let executor = Arc::new(ExecutionEngine::new(
        keypair,
        jito_client,
        Arc::clone(&blockhash_cache),
        tip_engine,
        Arc::clone(&position_manager),
        config.solana_rpc_url.clone(),
    ));

    // Query token accounts
    let http = reqwest::Client::new();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountsByOwner",
        "params": [
            wallet_pubkey.to_string(),
            {"programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"},
            {"encoding": "jsonParsed"}
        ]
    });

    let resp: serde_json::Value = http
        .post(&config.solana_rpc_url)
        .json(&payload)
        .send()
        .await?
        .json()
        .await?;

    let accounts = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
        .context("Failed to get token accounts")?;

    let mut open_tokens = Vec::new();

    for acc in accounts {
        if let Some(info) = acc.get("account").and_then(|a| a.get("data")).and_then(|d| d.get("parsed")).and_then(|p| p.get("info")) {
            let mint_str = info.get("mint").and_then(|m| m.as_str()).unwrap_or_default();
            let amount_str = info.get("tokenAmount").and_then(|t| t.get("amount")).and_then(|a| a.as_str()).unwrap_or("0");
            let token_amount: u64 = amount_str.parse().unwrap_or(0);

            if token_amount > 0 {
                let mint = Pubkey::from_str(mint_str)?;
                open_tokens.push((mint, token_amount));
            }
        }
    }

    if open_tokens.is_empty() {
        println!("✅ No open token positions found in wallet!");
        return Ok(());
    }

    println!("Found {} open position(s) to sell:", open_tokens.len());

    for (mint, amount) in open_tokens {
        println!("\n------------------------------------------------------------");
        println!("🚀 Exiting position: Mint {}", mint);
        println!("   Token Balance: {} (raw: {})", amount as f64 / 1e6, amount);

        let (bonding_curve, _) = derive_bonding_curve(&mint);
        let curve_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                bonding_curve.to_string(),
                {"encoding": "base64"}
            ]
        });

        let c_resp: serde_json::Value = http
            .post(&config.solana_rpc_url)
            .json(&curve_payload)
            .send()
            .await?
            .json()
            .await?;

        let data_arr = c_resp
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.get("data"))
            .and_then(|d| d.as_array())
            .context("Failed to fetch curve account data")?;

        let b64 = data_arr.first().and_then(|d| d.as_str()).context("Invalid b64")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
        let curve_pod = BondingCurveAccountPod::read_from_account(&bytes).context("Failed to parse curve POD")?;

        println!("   Current Curve Spot Price: {:.10} SOL", curve_pod.current_spot_price_sol());
        println!("   Real SOL in Curve: {:.4} SOL", curve_pod.real_sol_reserves as f64 / 1e9);

        let sell_action = TradeAction::Sell {
            mint,
            token_amount: amount,
            slippage_bps: 1000, // 10% slippage floor
            curve_state: curve_pod,
            is_panic: false,
        };

        println!("   Broadcasting Jito + RPC Dual-Routed Sell Transaction...");
        executor.execute_action(sell_action).await?;
        println!("   ✅ Exit action completed for mint {}", mint);
    }

    // Check final wallet SOL balance
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let bal_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [wallet_pubkey.to_string()]
    });
    let bal_resp: serde_json::Value = http
        .post(&config.solana_rpc_url)
        .json(&bal_payload)
        .send()
        .await?
        .json()
        .await?;

    let lamports = bal_resp.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_u64()).unwrap_or(0);
    println!("\n============================================================");
    println!("💰 FINAL TRADER WALLET BALANCE: {:.4} SOL ({} lamports)", lamports as f64 / 1e9, lamports);
    println!("============================================================");

    Ok(())
}
