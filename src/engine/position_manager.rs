use base64::Engine;
use log::{info, warn};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct Position {
    pub mint: Pubkey,
    pub entry_price_sol: f64,
    pub highest_price_sol: f64,
    pub token_balance: u64,
    pub sol_invested_lamports: u64,
    pub dev_wallet: Option<Pubkey>,
    pub dev_initial_balance: u64,
    pub token_program: Pubkey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    TrailingStop,
    TakeProfit,
    HardStopLoss,
    DevDumpPanic,
    BondingCurveGraduated,
}

pub struct PositionManager {
    positions: Arc<RwLock<HashMap<Pubkey, Position>>>,
    trailing_stop_pct: f64,
    hard_stop_loss_pct: f64,
    take_profit_pct: f64,
    dev_dump_threshold_pct: f64,
}

impl PositionManager {
    pub fn new(
        trailing_stop_pct: f64,
        hard_stop_loss_pct: f64,
        take_profit_pct: f64,
        dev_dump_threshold_pct: f64,
    ) -> Self {
        Self {
            positions: Arc::new(RwLock::new(HashMap::new())),
            trailing_stop_pct,
            hard_stop_loss_pct,
            take_profit_pct,
            dev_dump_threshold_pct,
        }
    }

    /// Add new acquired position
    pub async fn add_position(&self, position: Position) {
        let mut lock = self.positions.write().await;
        info!(
            "🎯 Opened position for mint {}: entry price {} SOL, balance {} tokens",
            position.mint, position.entry_price_sol, position.token_balance
        );
        lock.insert(position.mint, position);
    }

    /// Check price update against Take Profit, Hard Stop Loss, and Trailing Stop
    pub async fn on_price_update(&self, curve_or_mint: &Pubkey, current_price_sol: f64) -> Option<(Position, ExitReason)> {
        let mut lock = self.positions.write().await;
        let matched_mint = lock.iter().find_map(|(mint, _)| {
            let (bonding_curve, _) = crate::constants::derive_bonding_curve(mint);
            if mint == curve_or_mint || &bonding_curve == curve_or_mint {
                Some(*mint)
            } else {
                None
            }
        });

        if let Some(mint) = matched_mint {
            if let Some(pos) = lock.get_mut(&mint) {
                if current_price_sol > pos.highest_price_sol {
                    pos.highest_price_sol = current_price_sol;
                }

                let hard_stop_price = pos.entry_price_sol * (1.0 - self.hard_stop_loss_pct);
                let trailing_stop_price = pos.highest_price_sol * (1.0 - self.trailing_stop_pct);
                let tp_price = pos.entry_price_sol * self.take_profit_pct;

            // 1. Take Profit
            if current_price_sol >= tp_price {
                info!(
                    "💰 Take profit hit for {}: price reached {} SOL (entry: {})",
                    mint, current_price_sol, pos.entry_price_sol
                );
                let removed = lock.remove(&mint).unwrap();
                return Some((removed, ExitReason::TakeProfit));
            }
            // 2. Hard Stop Loss (cut dead drops immediately)
            else if current_price_sol <= hard_stop_price {
                warn!(
                    "🛑 Hard stop loss hit for {}: price dropped to {} SOL <= floor {} SOL (-{:.1}%)",
                    mint, current_price_sol, hard_stop_price, self.hard_stop_loss_pct * 100.0
                );
                let removed = lock.remove(&mint).unwrap();
                return Some((removed, ExitReason::HardStopLoss));
            }
            // 3. Trailing Stop (lock in gains after initial +30% pump)
            else if current_price_sol <= trailing_stop_price && pos.highest_price_sol > pos.entry_price_sol * 1.30 {
                warn!(
                    "📉 Trailing stop hit for {}: price dropped from peak {} to {} SOL",
                    mint, pos.highest_price_sol, current_price_sol
                );
                let removed = lock.remove(&mint).unwrap();
                return Some((removed, ExitReason::TrailingStop));
            }
        }
    }
    None
}

    /// Check if transaction is a dev dump that requires immediate panic front-run
    pub async fn check_dev_dump(
        &self,
        mint: &Pubkey,
        signer: &Pubkey,
        sell_amount: u64,
    ) -> Option<Position> {
        let mut lock = self.positions.write().await;
        if let Some(pos) = lock.get(mint) {
            if let Some(dev) = pos.dev_wallet {
                if dev == *signer {
                    let threshold = (pos.dev_initial_balance as f64 * self.dev_dump_threshold_pct) as u64;
                    if sell_amount >= threshold && threshold > 0 {
                        warn!(
                            "🚨 DEV DUMP DETECTED for {}: dev sold {} tokens! Triggering PANIC SELL!",
                            mint, sell_amount
                        );
                        return lock.remove(mint);
                    }
                }
            }
        }
        None
    }

    /// Get position copy
    pub async fn get_position(&self, mint: &Pubkey) -> Option<Position> {
        let lock = self.positions.read().await;
        lock.get(mint).cloned()
    }

    /// Check total SOL currently deployed in open positions
    pub async fn total_invested_sol(&self) -> f64 {
        let lock = self.positions.read().await;
        lock.values().map(|p| p.sol_invested_lamports as f64 / 1e9).sum()
    }

    /// Check if we already have an active position for a mint
    pub async fn has_position(&self, mint: &Pubkey) -> bool {
        let lock = self.positions.read().await;
        lock.contains_key(mint)
    }

    /// Number of active open positions
    pub async fn position_count(&self) -> usize {
        let lock = self.positions.read().await;
        lock.len()
    }

    /// Recover open token positions from on-chain wallet token accounts on startup
    pub async fn recover_open_positions(
        &self,
        rpc_url: &str,
        wallet: &Pubkey,
        curve_cache: &Arc<RwLock<HashMap<Pubkey, crate::decoders::BondingCurveAccountPod>>>,
    ) {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                wallet.to_string(),
                {"programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"},
                {"encoding": "jsonParsed"}
            ]
        });

        if let Ok(resp) = client.post(rpc_url).json(&payload).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(accounts) = json.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_array()) {
                    for acc in accounts {
                        if let Some(info) = acc.get("account").and_then(|a| a.get("data")).and_then(|d| d.get("parsed")).and_then(|p| p.get("info")) {
                            let mint_str = info.get("mint").and_then(|m| m.as_str()).unwrap_or_default();
                            let amount_str = info.get("tokenAmount").and_then(|t| t.get("amount")).and_then(|a| a.as_str()).unwrap_or("0");
                            let token_amount: u64 = amount_str.parse().unwrap_or(0);

                            if token_amount > 0 {
                                if let Ok(mint) = Pubkey::from_str(mint_str) {
                                    let (bonding_curve, _) = crate::constants::derive_bonding_curve(&mint);
                                    let curve_payload = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": 1,
                                        "method": "getAccountInfo",
                                        "params": [
                                            bonding_curve.to_string(),
                                            {"encoding": "base64"}
                                        ]
                                    });
                                    if let Ok(c_resp) = client.post(rpc_url).json(&curve_payload).send().await {
                                        if let Ok(c_json) = c_resp.json::<serde_json::Value>().await {
                                            if let Some(data_arr) = c_json.get("result").and_then(|r| r.get("value")).and_then(|v| v.get("data")).and_then(|d| d.as_array()) {
                                                if let Some(b64) = data_arr.first().and_then(|d| d.as_str()) {
                                                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                                                        if let Some(pod) = crate::decoders::BondingCurveAccountPod::read_from_account(&bytes) {
                                                            let spot_price = pod.current_spot_price_sol();
                                                            info!(
                                                                "🔄 Restored active on-chain position for mint {}: {} tokens (current price: {:.10} SOL)",
                                                                mint, token_amount, spot_price
                                                            );
                                                            {
                                                                let mut cache = curve_cache.write().await;
                                                                cache.insert(mint, pod);
                                                            }
                                                            let mut lock = self.positions.write().await;
                                                            lock.insert(mint, Position {
                                                                mint,
                                                                entry_price_sol: spot_price,
                                                                highest_price_sol: spot_price,
                                                                token_balance: token_amount,
                                                                sol_invested_lamports: 200_000_000,
                                                                dev_wallet: Some(pod.creator),
                                                                dev_initial_balance: 0,
                                                                token_program: crate::constants::spl_token_2022_program_id(),
                                                            });
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
