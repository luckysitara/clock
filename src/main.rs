use anyhow::{Context, Result};
use log::{error, info, warn};
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
            tip_max_pct_of_trade: 0.01,
            tip_max_panic_pct_of_trade: 0.03,
            dry_run: false,
            paper_trades_file: "data/paper_trades.jsonl".to_string(),
            copy_trade_amount_sol: 0.2,
            max_position_sol: 2.0,
            slippage_bps: 500,
            trailing_stop_pct: 0.15,
            hard_stop_loss_pct: 0.20,
            take_profit_pct: 2.5,
            dev_dump_threshold_pct: 0.20,
            min_creator_buy_sol: 0.20,
            min_whale_buy_sol: 1.0,
            target_wallets: vec![],
            dual_route_rpc: true,
            positions_file: "data/positions.json".to_string(),
            max_curve_cache_entries: 50_000,
        }
    }));

    // 2. Load Keypair
    // No silent fallback: a throwaway keypair would still send real (failing) transactions
    let keypair = config
        .load_keypair()
        .with_context(|| format!("Could not load trader keypair at '{}'", config.keypair_path))?;
    info!("🔑 Trader Keypair loaded successfully: {}", keypair.pubkey());

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
    info!("   Min Dev Buy Filter:  {} SOL (Launch Snipe Concurrence)", config.min_creator_buy_sol);
    info!("   Copy-Trade Amount:   {} SOL", config.copy_trade_amount_sol);
    info!("   Slippage Tolerance:  {} bps ({}%)", config.slippage_bps, config.slippage_bps as f64 / 100.0);
    info!("   Trailing Stop Loss:  {}%", config.trailing_stop_pct * 100.0);
    info!("   Hard Stop Loss:      -{}%", config.hard_stop_loss_pct * 100.0);
    info!("   Take Profit Target:  {}x", config.take_profit_pct);
    info!("   Min Whale Buy:       {} SOL", config.min_whale_buy_sol);
    info!("   Tip caps:            {:.1}% of trade (panic {:.1}%)", config.tip_max_pct_of_trade * 100.0, config.tip_max_panic_pct_of_trade * 100.0);
    info!("   Mode:                {}", if config.dry_run { "DRY RUN (paper trading)" } else { "LIVE" });
    info!("   Routing:             {}", if config.dual_route_rpc { "Jito + public RPC" } else { "Jito bundles only" });

    // 3. Initialize Blockhash Cache Background Task
    let blockhash_cache = Arc::new(BlockhashCache::new());
    blockhash_cache.spawn_updater(config.solana_rpc_url.clone(), Duration::from_millis(1500));
    if !blockhash_cache.wait_until_ready(Duration::from_secs(15)).await {
        anyhow::bail!("No blockhash from {} after 15s; check SOLANA_RPC_URL", config.solana_rpc_url);
    }

    // 4. Initialize Core Components
    let tip_engine = Arc::new(TipEngine::new(
        config.min_tip_lamports,
        config.max_tip_lamports,
        config.tip_aggression_factor,
    ).with_trade_caps(config.tip_max_pct_of_trade, config.tip_max_panic_pct_of_trade));

    let jito_client = Arc::new(JitoClient::new(config.jito_block_engine_url.clone()));
    jito_client.spawn_prewarmer();

    let position_manager = Arc::new(PositionManager::new(
        config.trailing_stop_pct,
        config.hard_stop_loss_pct,
        config.take_profit_pct,
        config.dev_dump_threshold_pct,
    ).with_state_file(&config.positions_file));

    // 5. Initialize In-Memory Bonding Curve Cache
    let curve_cache = Arc::new(RwLock::new(HashMap::new()));

    // Recover open on-chain positions from trader wallet (paper mode starts flat instead)
    if config.dry_run {
        warn!("📝 DRY RUN: nothing will be sent on-chain. Buys are simulated, trades logged to {}", config.paper_trades_file);
    } else {
        position_manager
            .recover_open_positions(&config.solana_rpc_url, &keypair.pubkey(), &curve_cache)
            .await;
    }

    // 6. Setup MPSC Channel for Trade Signals & Spawn Execution Worker Pool
    let (tx_trades, rx_trades) = mpsc::channel::<TradeAction>(2000);

    let executor = ExecutionEngine::new(
        keypair,
        jito_client,
        Arc::clone(&blockhash_cache),
        tip_engine,
        Arc::clone(&position_manager),
        config.solana_rpc_url.clone(),
        config.dual_route_rpc,
    );
    let executor = Arc::new(if config.dry_run {
        executor.with_dry_run(&config.paper_trades_file)
    } else {
        executor
    });

    executor.spawn_balance_refresher();
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
