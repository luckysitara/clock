use anyhow::Result;
use log::{error, info};
use solana_sdk::signer::Signer;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

use clockit::{
    config::BotConfig,
    engine::{
        blockhash_cache::BlockhashCache,
        executor::{ExecutionEngine, TradeAction},
        jito::JitoClient,
        position_manager::PositionManager,
        tip_engine::TipEngine,
    },
    stream::yellowstone::YellowstoneStreamer,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> Result<()> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    println!(
        r#"
  ██████╗██╗      ██████╗  ██████╗██╗  ██╗██╗████████╗
 ██╔════╝██║     ██╔═══██╗██╔════╝██║ ██╔╝██║╚══██╔══╝
 ██║     ██║     ██║   ██║██║     █████╔╝ ██║   ██║   
 ██║     ██║     ██║   ██║██║     ██╔═██╗ ██║   ██║   
 ╚██████╗███████╗╚██████╔╝╚██████╗██║  ██╗██║   ██║   
  ╚═════╝╚══════╝ ╚═════╝  ╚═════╝╚═╝  ╚═╝╚═╝   ╚═╝   
    Pump.fun Zero-Latency HFT Sniper & Copy-Trader v{}
    Powered by Triton Yellowstone gRPC & Jito Block Engine
"#,
        clockit::version()
    );

    // 1. Load Configuration
    let config = Arc::new(BotConfig::load_from_env().unwrap_or_else(|e| {
        error!("Failed to load configuration: {:#}. Using default parameters.", e);
        BotConfig {
            solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            yellowstone_grpc_url: "https://solana-grpc.triton.one:443".to_string(),
            yellowstone_x_token: None,
            keypair_path: "/home/rootkit/.config/solana/id.json".to_string(),
            jito_block_engine_url: "https://mainnet.block-engine.jito.wtf/api/v1/bundles".to_string(),
            min_tip_lamports: 100_000,
            max_tip_lamports: 50_000_000,
            tip_aggression_factor: 0.65,
            copy_trade_amount_sol: 0.2,
            max_position_sol: 2.0,
            slippage_bps: 500,
            trailing_stop_pct: 0.15,
            hard_stop_loss_pct: 0.20,
            take_profit_pct: 2.5,
            dev_dump_threshold_pct: 0.20,
            target_wallets: vec![],
        }
    }));

    // 2. Load Keypair
    let keypair = match config.load_keypair() {
        Ok(kp) => {
            info!("🔑 Trader Keypair loaded successfully: {}", kp.pubkey());
            kp
        }
        Err(e) => {
            error!(
                "⚠️ Could not load keypair at '{}': {}. Generating temporary keypair for dry-run.",
                config.keypair_path, e
            );
            Arc::new(solana_sdk::signature::Keypair::new())
        }
    };

    info!("⚙️ Network Configuration:");
    info!("   Solana RPC:          {}", config.solana_rpc_url);
    info!("   Triton gRPC:         {}", config.yellowstone_grpc_url);
    info!(
        "   Triton x-token:      {}",
        if config.yellowstone_x_token.is_some() {
            "Configured [PROTECTED]"
        } else {
            "NOT CONFIGURED (Check .env)"
        }
    );
    info!("   Jito Block Engine:   {}", config.jito_block_engine_url);
    info!("   Target Wallets:      {} addresses", config.target_wallets.len());
    info!("   Copy-Trade Amount:   {} SOL", config.copy_trade_amount_sol);
    info!("   Slippage Tolerance:  {} bps ({}%)", config.slippage_bps, config.slippage_bps as f64 / 100.0);
    info!("   Trailing Stop Loss:  {}%", config.trailing_stop_pct * 100.0);
    info!("   Hard Stop Loss:      -{}%", config.hard_stop_loss_pct * 100.0);
    info!("   Take Profit Target:  {}x", config.take_profit_pct);

    // 3. Initialize Blockhash Cache Background Task
    let blockhash_cache = Arc::new(BlockhashCache::new());
    blockhash_cache.spawn_updater(config.solana_rpc_url.clone(), Duration::from_millis(1500));

    // 4. Initialize Core Components
    let tip_engine = Arc::new(TipEngine::new(
        config.min_tip_lamports,
        config.max_tip_lamports,
        config.tip_aggression_factor,
    ));

    let jito_client = Arc::new(JitoClient::new(config.jito_block_engine_url.clone()));

    let position_manager = Arc::new(PositionManager::new(
        config.trailing_stop_pct,
        config.hard_stop_loss_pct,
        config.take_profit_pct,
        config.dev_dump_threshold_pct,
    ));

    // 5. Initialize In-Memory Bonding Curve Cache
    let curve_cache = Arc::new(RwLock::new(HashMap::new()));

    // 6. Setup MPSC Channel for Trade Signals & Spawn Execution Worker Pool
    let (tx_trades, rx_trades) = mpsc::channel::<TradeAction>(2000);

    let executor = Arc::new(ExecutionEngine::new(
        keypair,
        jito_client,
        Arc::clone(&blockhash_cache),
        tip_engine,
        Arc::clone(&position_manager),
        config.solana_rpc_url.clone(),
    ));

    executor.spawn_worker(rx_trades);

    // 7. Connect to Triton Yellowstone gRPC and Start Stream Loop
    let streamer = YellowstoneStreamer::new(
        Arc::clone(&config),
        curve_cache,
        position_manager,
        tx_trades,
    );

    streamer.run_loop().await?;

    Ok(())
}
