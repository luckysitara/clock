use log::{info, warn};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
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
    pub async fn on_price_update(&self, mint: &Pubkey, current_price_sol: f64) -> Option<(Position, ExitReason)> {
        let mut lock = self.positions.write().await;
        if let Some(pos) = lock.get_mut(mint) {
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
                let removed = lock.remove(mint).unwrap();
                return Some((removed, ExitReason::TakeProfit));
            }
            // 2. Hard Stop Loss (cut dead drops immediately)
            else if current_price_sol <= hard_stop_price {
                warn!(
                    "🛑 Hard stop loss hit for {}: price dropped to {} SOL <= floor {} SOL (-{:.1}%)",
                    mint, current_price_sol, hard_stop_price, self.hard_stop_loss_pct * 100.0
                );
                let removed = lock.remove(mint).unwrap();
                return Some((removed, ExitReason::HardStopLoss));
            }
            // 3. Trailing Stop (lock in gains after initial +30% pump)
            else if current_price_sol <= trailing_stop_price && pos.highest_price_sol > pos.entry_price_sol * 1.30 {
                warn!(
                    "📉 Trailing stop hit for {}: price dropped from peak {} to {} SOL",
                    mint, pos.highest_price_sol, current_price_sol
                );
                let removed = lock.remove(mint).unwrap();
                return Some((removed, ExitReason::TrailingStop));
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
}
