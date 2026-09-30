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

        let copy_trade_amount_sol = env::var("COPY_TRADE_AMOUNT_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.2); // 0.2 SOL default

        let max_position_sol = env::var("MAX_POSITION_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.0); // 2.0 SOL max

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
            .unwrap_or(0.50); // Minimum 0.50 SOL dev buy required to snipe unbacked launches

        let min_whale_buy_sol = env::var("MIN_WHALE_BUY_SOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0); // Minimum 1.0 SOL whale buy required to trigger copy-trade

        // Target Top 20 Leaderboard Wallets (comma-separated or loaded from TARGET_WALLETS)
        let target_wallets = env::var("TARGET_WALLETS")
            .map(|w| {
                w.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_else(|_| {
                // Sample Top Whale / Smart Money Leaderboard Wallets
                vec![
                    "4y2T1N99Z1fK492N28Psp52qf8Dk7uGg8sK5zUqbdvE1".to_string(),
                    "2k1c4GfX8n6D9sF6jZ8n5k4D8s7F6g5H4j3K2L1jaCf".to_string(),
                    "BvApZ7m9N8b7V6c5X4z3A2s1D4f5G6h7J8k9L0mSruZ".to_string(),
                ]
            });

        Ok(Self {
            solana_rpc_url,
            yellowstone_grpc_url,
            yellowstone_x_token,
            keypair_path,
            jito_block_engine_url,
            min_tip_lamports,
            max_tip_lamports,
            tip_aggression_factor,
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
