use anyhow::{Context, Result};
use solana_sdk::signer::Signer;
use std::sync::Arc;
use std::time::Duration;

use clockit::{
    config::BotConfig,
    engine::{
        blockhash_cache::BlockhashCache,
        executor::{ExecutionEngine, TradeAction},
        jito::JitoClient,
        position_manager::PositionManager,
        tip_engine::TipEngine,
        wallet::{fetch_bonding_curve, fetch_token_holdings},
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
    if !blockhash_cache.wait_until_ready(Duration::from_secs(15)).await {
        anyhow::bail!("No blockhash from {} after 15s; check SOLANA_RPC_URL", config.solana_rpc_url);
    }

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
        config.dual_route_rpc,
    ));

    let http = reqwest::Client::new();
    let holdings = fetch_token_holdings(&http, &config.solana_rpc_url, &wallet_pubkey)
        .await
        .context("Failed to get token accounts")?;

    if holdings.is_empty() {
        println!("✅ No open token positions found in wallet!");
        return Ok(());
    }

    println!("Found {} token balance(s) to check:", holdings.len());

    for holding in holdings {
        let mint = holding.mint;
        println!("\n------------------------------------------------------------");
        println!("🚀 Exiting position: Mint {}", mint);
        println!("   Token Balance: {} (raw: {}) | Token Prog: {}", holding.amount as f64 / 1e6, holding.amount, holding.token_program);

        let curve_pod = match fetch_bonding_curve(&http, &config.solana_rpc_url, &mint).await {
            Ok(Some(pod)) => pod,
            Ok(None) => {
                println!("   ⏭️  Not a Pump.fun bonding curve token, skipping");
                continue;
            }
            Err(e) => {
                println!("   ❌ Failed to fetch curve: {:#}", e);
                continue;
            }
        };
        if curve_pod.complete {
            println!("   🎓 Curve graduated: sell this on PumpSwap/Raydium, skipping");
            continue;
        }

        println!("   Current Curve Spot Price: {:.10} SOL", curve_pod.current_spot_price_sol());
        println!("   Real SOL in Curve: {:.4} SOL", curve_pod.real_sol_reserves as f64 / 1e9);

        let sell_action = TradeAction::Sell {
            mint,
            token_amount: holding.amount,
            slippage_bps: 1000, // 10% slippage floor
            curve_state: curve_pod,
            is_panic: false,
            token_program: Some(holding.token_program),
        };

        println!("   Broadcasting sell transaction...");
        if let Err(e) = executor.execute_action(sell_action).await {
            println!("   ❌ Sell failed for {}: {:#}", mint, e);
            continue;
        }
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
