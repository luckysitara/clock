use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::signature::Keypair;
use std::{env, fs, path::Path, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub solana_rpc_url: String,
    pub yellowstone_grpc_url: String,
    pub yellowstone_x_token: Option<String>,
    pub keypair_path: String,
    pub jito_block_engine_url: String,
    pub min_tip_lamports: u64,
    pub max_tip_lamports: u64,
    pub tip_aggression_factor: f64,
    pub tip_max_pct_of_trade: f64,
    pub tip_max_panic_pct_of_trade: f64,
    pub dry_run: bool,
    pub paper_trades_file: String,
    pub copy_trade_amount_sol: f64,
    pub max_position_sol: f64,
    pub slippage_bps: u64,
    pub trailing_stop_pct: f64,
    pub hard_stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub dev_dump_threshold_pct: f64,
    pub min_creator_buy_sol: f64,
    pub min_whale_buy_sol: f64,
    pub target_wallets: Vec<String>,
    /// Fast wallets used as a group-buy signal (not copied one by one)
    pub signal_wallets: Vec<String>,
    pub signal_min_wallets: usize,
    pub signal_window_secs: u64,
    pub dual_route_rpc: bool,
    pub positions_file: String,
    pub max_curve_cache_entries: usize,
}

/// DUAL_ROUTE_RPC: also send transactions via public RPC (default true; "0/false/no/off" disables)
pub fn dual_route_rpc_from_env() -> bool {
    env::var("DUAL_ROUTE_RPC")
        .ok()
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true)
}

impl BotConfig {
    pub fn load_from_env() -> Result<Self> {
        let _ = dotenv::dotenv();

        let solana_rpc_url = env::var("SOLANA_RPC_URL")
            .or_else(|_| env::var("HELIUS_RPC_URL"))
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

        let yellowstone_grpc_url = env::var("YELLOWSTONE_GRPC_URL")
            .unwrap_or_else(|_| "https://mainnet.helius-rpc.com:443".to_string());

        let yellowstone_x_token = env::var("YELLOWSTONE_X_TOKEN")
            .or_else(|_| env::var("TRITON_X_TOKEN"))
            .ok()
            .filter(|s| !s.is_empty());

        let keypair_path = env::var("TRADER_PRIVATE_KEY")
            .or_else(|_| env::var("KEYPAIR_PATH"))
            .unwrap_or_else(|_| {
                let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                format!("{}/.config/solana/id.json", home)
            });

        let jito_block_engine_url = env::var("JITO_BLOCK_ENGINE_URL")
            .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf/api/v1/bundles".to_string());

        let min_tip_lamports = env::var("MIN_TIP_LAMPORTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100_000); // 0.0001 SOL

        let max_tip_lamports = env::var("MAX_TIP_LAMPORTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50_000_000); // 0.05 SOL

        let tip_aggression_factor = env::var("TIP_AGGRESSION_FACTOR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.65); // 65% of EV

        // Tips never exceed this share of the trade value (entries/normal exits, panic exits/retries)
        let tip_max_pct_of_trade = env::var("TIP_MAX_PCT_OF_TRADE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.01);
        let tip_max_panic_pct_of_trade = env::var("TIP_MAX_PANIC_PCT_OF_TRADE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.03);

        // Paper trading: simulate buys, track positions and exits, send nothing
        let dry_run = env::var("DRY_RUN")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let paper_trades_file = env::var("PAPER_TRADES_FILE")
            .unwrap_or_else(|_| "data/paper_trades.jsonl".to_string());

        let copy_trade_amount_sol = env::var("COPY_TRADE_AMOUNT_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.035); // 0.035 SOL default bullet

        let max_position_sol = env::var("MAX_POSITION_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.07); // 0.07 SOL max (2 bullets)

        let slippage_bps = env::var("SLIPPAGE_BPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(500); // 5% slippage

        let trailing_stop_pct = env::var("TRAILING_STOP_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.15); // 15% trailing stop

        let hard_stop_loss_pct = env::var("HARD_STOP_LOSS_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.20); // 20% hard stop loss

        let take_profit_pct = env::var("TAKE_PROFIT_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.5); // 2.5x take profit

        let dev_dump_threshold_pct = env::var("DEV_DUMP_THRESHOLD_PCT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.20); // 20% dump triggers panic exit

        let min_creator_buy_sol = env::var("MIN_CREATOR_BUY_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.20); // Minimum 0.20 SOL dev buy filter

        let min_whale_buy_sol = env::var("MIN_WHALE_BUY_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0); // Minimum 1.0 SOL whale buy required to trigger copy-trade

        // Target Leaderboard Wallets (comma-separated or loaded from TARGET_WALLETS / file)
        let target_wallets = env::var("TARGET_WALLETS")
            .map(|w| {
                w.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_else(|_| {
                if let Ok(content) = std::fs::read_to_string("data/smart_money_wallets.txt") {
                    content
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    vec![]
                }
            });

        // Also send transactions via public RPC sendTransaction (true) or Jito bundles only (false)
        let dual_route_rpc = dual_route_rpc_from_env();

        // Paper positions are kept apart from real ones
        let positions_file = env::var("POSITIONS_FILE").unwrap_or_else(|_| {
            if dry_run { "data/paper_positions.json" } else { "data/positions.json" }.to_string()
        });

        let max_curve_cache_entries = env::var("MAX_CURVE_CACHE_ENTRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50_000);

        // Group-buy signal: buy when this many signal wallets buy the same token within the window
        let signal_wallets: Vec<String> = env::var("SIGNAL_WALLETS")
            .map(|w| w.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let signal_min_wallets = env::var("SIGNAL_MIN_WALLETS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2usize)
            .max(2);
        let signal_window_secs = env::var("SIGNAL_WINDOW_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);

        Ok(Self {
            solana_rpc_url,
            yellowstone_grpc_url,
            yellowstone_x_token,
            keypair_path,
            jito_block_engine_url,
            min_tip_lamports,
            max_tip_lamports,
            tip_aggression_factor,
            tip_max_pct_of_trade,
            tip_max_panic_pct_of_trade,
            dry_run,
            paper_trades_file,
            copy_trade_amount_sol,
            max_position_sol,
            slippage_bps,
            trailing_stop_pct,
            hard_stop_loss_pct,
            take_profit_pct,
            dev_dump_threshold_pct,
            min_creator_buy_sol,
            min_whale_buy_sol,
            target_wallets,
            signal_wallets,
            signal_min_wallets,
            signal_window_secs,
            dual_route_rpc,
            positions_file,
            max_curve_cache_entries,
        })
    }

    pub fn load_keypair(&self) -> Result<Arc<Keypair>> {
        let path = Path::new(&self.keypair_path);
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read keypair file at {}", self.keypair_path))?;

        // Format is JSON array of bytes [1,2,3,...] or base58 string
        let keypair = if data.trim().starts_with('[') {
            let bytes: Vec<u8> = serde_json::from_str(&data)
                .context("Failed to parse keypair JSON array")?;
            Keypair::try_from(&bytes[..]).context("Invalid keypair bytes")?
        } else {
            let bytes = bs58::decode(data.trim())
                .into_vec()
                .context("Failed to decode base58 keypair string")?;
            Keypair::try_from(&bytes[..]).context("Invalid keypair bytes from base58")?
        };

        Ok(Arc::new(keypair))
    }
}
