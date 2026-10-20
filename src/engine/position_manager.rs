use log::{info, warn};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::engine::wallet::{fetch_bonding_curve, fetch_token_holdings};

#[derive(Debug, Clone)]
pub struct Position {
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub entry_price_sol: f64,
    pub highest_price_sol: f64,
    pub token_balance: u64,
    pub sol_invested_lamports: u64,
    pub dev_wallet: Option<Pubkey>,
    /// Dev token balance when we entered (0 = unknown → any dev sell is treated as a dump)
    pub dev_initial_balance: u64,
    pub token_program: Pubkey,
    pub is_selling: bool,
    /// Wallets whose buy we copied; when any of them sells, we exit
    pub leaders: Vec<Pubkey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    TrailingStop,
    TakeProfit,
    HardStopLoss,
    DevDumpPanic,
    BondingCurveGraduated,
}

/// Outcome of trying to reserve capital for a new buy
#[derive(Debug, Clone, PartialEq)]
pub enum Reservation {
    Reserved,
    AlreadyActive,
    CapitalLimit { committed_sol: f64 },
}

/// Entry data persisted to disk so restarts keep the real entry price and stops
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPosition {
    pub mint: String,
    pub entry_price_sol: f64,
    pub highest_price_sol: f64,
    pub sol_invested_lamports: u64,
    pub dev_wallet: Option<String>,
    pub dev_initial_balance: u64,
    #[serde(default)]
    pub leaders: Vec<String>,
}

/// Read a positions state file (empty if missing or unreadable)
pub fn load_saved_positions(path: &std::path::Path) -> HashMap<Pubkey, SavedPosition> {
    let Ok(bytes) = std::fs::read(path) else { return HashMap::new() };
    match serde_json::from_slice::<Vec<SavedPosition>>(&bytes) {
        Ok(records) => records
            .into_iter()
            .filter_map(|r| Pubkey::from_str(&r.mint).ok().map(|m| (m, r)))
            .collect(),
        Err(e) => {
            warn!("⚠️ Could not parse {}: {}", path.display(), e);
            HashMap::new()
        }
    }
}

#[derive(Default)]
struct State {
    positions: HashMap<Pubkey, Position>,
    /// Bonding curve PDA → mint, so account updates resolve in O(1) without PDA derivation
    curve_to_mint: HashMap<Pubkey, Pubkey>,
    /// Buys in flight (sent but not yet confirmed): mint → reserved lamports
    pending: HashMap<Pubkey, u64>,
}

impl State {
    fn committed_lamports(&self) -> u64 {
        self.positions.values().map(|p| p.sol_invested_lamports).sum::<u64>()
            + self.pending.values().sum::<u64>()
    }
}

pub struct PositionManager {
    state: Arc<RwLock<State>>,
    trailing_stop_pct: f64,
    hard_stop_loss_pct: f64,
    take_profit_pct: f64,
    dev_dump_threshold_pct: f64,
    state_path: Option<PathBuf>,
}

impl PositionManager {
    pub fn new(
        trailing_stop_pct: f64,
        hard_stop_loss_pct: f64,
        take_profit_pct: f64,
        dev_dump_threshold_pct: f64,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(State::default())),
            trailing_stop_pct,
            hard_stop_loss_pct,
            take_profit_pct,
            dev_dump_threshold_pct,
            state_path: None,
        }
    }

    /// Persist open positions to this JSON file on every open/close
    pub fn with_state_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.state_path = Some(path.into());
        self
    }

    /// Atomically check for an existing/pending position and the capital limit, then reserve.
    /// Every `Reserved` result must be followed by `add_position` or `release`.
    pub async fn try_reserve(&self, mint: &Pubkey, lamports: u64, max_lamports: u64) -> Reservation {
        let mut state = self.state.write().await;
        if state.positions.contains_key(mint) || state.pending.contains_key(mint) {
            return Reservation::AlreadyActive;
        }
        let committed = state.committed_lamports();
        if committed + lamports > max_lamports {
            return Reservation::CapitalLimit {
                committed_sol: committed as f64 / 1e9,
            };
        }
        state.pending.insert(*mint, lamports);
        Reservation::Reserved
    }

    /// Release a reservation whose buy did not land
    pub async fn release(&self, mint: &Pubkey) {
        self.state.write().await.pending.remove(mint);
    }

    /// Add new acquired position (clears its pending reservation)
    pub async fn add_position(&self, position: Position) {
        let mut state = self.state.write().await;
        info!(
            "🎯 Opened position for mint {}: entry price {} SOL, balance {} tokens",
            position.mint, position.entry_price_sol, position.token_balance
        );
        state.pending.remove(&position.mint);
        state.curve_to_mint.insert(position.bonding_curve, position.mint);
        state.positions.insert(position.mint, position);
        self.persist(&state);
    }

    /// Record the dev's token balance once it is known (e.g. fetched after entry)
    pub async fn set_dev_initial_balance(&self, mint: &Pubkey, balance: u64) {
        let mut state = self.state.write().await;
        if let Some(pos) = state.positions.get_mut(mint) {
            pos.dev_initial_balance = balance;
            self.persist(&state);
        }
    }

    /// Check price update against Take Profit, Hard Stop Loss, and Trailing Stop
    pub async fn on_price_update(&self, curve_or_mint: &Pubkey, current_price_sol: f64) -> Option<(Position, ExitReason)> {
        // Fast path: almost every Pump.fun curve update is for a token we don't hold
        let mint = {
            let state = self.state.read().await;
            match state.curve_to_mint.get(curve_or_mint) {
                Some(m) => *m,
                None if state.positions.contains_key(curve_or_mint) => *curve_or_mint,
                None => return None,
            }
        };

        let mut state = self.state.write().await;
        let pos = state.positions.get_mut(&mint)?;
        if pos.is_selling {
            return None;
        }

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
            pos.is_selling = true;
            return Some((pos.clone(), ExitReason::TakeProfit));
        }
        // 2. Hard Stop Loss (cut dead drops immediately)
        else if current_price_sol <= hard_stop_price {
            warn!(
                "🛑 Hard stop loss hit for {}: price dropped to {} SOL <= floor {} SOL (-{:.1}%)",
                mint, current_price_sol, hard_stop_price, self.hard_stop_loss_pct * 100.0
            );
            pos.is_selling = true;
            return Some((pos.clone(), ExitReason::HardStopLoss));
        }
        // 3. Trailing Stop (lock in gains after initial +30% pump)
        else if current_price_sol <= trailing_stop_price && pos.highest_price_sol > pos.entry_price_sol * 1.30 {
            warn!(
                "📉 Trailing stop hit for {}: price dropped from peak {} to {} SOL",
                mint, pos.highest_price_sol, current_price_sol
            );
            pos.is_selling = true;
            return Some((pos.clone(), ExitReason::TrailingStop));
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
        let mut state = self.state.write().await;
        let pos = state.positions.get_mut(mint)?;
        if pos.is_selling || pos.dev_wallet != Some(*signer) {
            return None;
        }

        // Unknown dev balance: any dev sell counts as a dump
        let threshold = if pos.dev_initial_balance > 0 {
            ((pos.dev_initial_balance as f64 * self.dev_dump_threshold_pct) as u64).max(1)
        } else {
            1
        };

        if sell_amount >= threshold {
            warn!(
                "🚨 DEV DUMP DETECTED for {}: dev sold {} tokens (threshold {}, dev entry balance {})! Triggering PANIC SELL!",
                mint, sell_amount, threshold, pos.dev_initial_balance
            );
            pos.is_selling = true;
            return Some(pos.clone());
        }
        None
    }

    /// A wallet we copied into this position just sold: exit with it
    pub async fn check_leader_exit(&self, mint: &Pubkey, seller: &Pubkey) -> Option<Position> {
        let mut state = self.state.write().await;
        let pos = state.positions.get_mut(mint)?;
        if pos.is_selling || !pos.leaders.contains(seller) {
            return None;
        }
        warn!("🏃 LEADER EXIT for {}: copied wallet {} is selling, exiting with it", mint, seller);
        pos.is_selling = true;
        Some(pos.clone())
    }

    /// Remove position once sell is confirmed on-chain
    pub async fn remove_position(&self, mint: &Pubkey) -> Option<Position> {
        let mut state = self.state.write().await;
        let removed = state.positions.remove(mint);
        if let Some(pos) = &removed {
            state.curve_to_mint.remove(&pos.bonding_curve);
            self.persist(&state);
        }
        removed
    }

    /// Reset is_selling flag if a sell transaction completely fails or drops
    pub async fn unmark_selling(&self, mint: &Pubkey) {
        let mut state = self.state.write().await;
        if let Some(pos) = state.positions.get_mut(mint) {
            pos.is_selling = false;
        }
    }

    /// Mint of the open position on this bonding curve, if any
    pub async fn mint_for_curve(&self, bonding_curve: &Pubkey) -> Option<Pubkey> {
        self.state.read().await.curve_to_mint.get(bonding_curve).copied()
    }

    /// Get position copy
    pub async fn get_position(&self, mint: &Pubkey) -> Option<Position> {
        self.state.read().await.positions.get(mint).cloned()
    }

    /// SOL committed to open positions plus buys still in flight
    pub async fn total_invested_sol(&self) -> f64 {
        self.state.read().await.committed_lamports() as f64 / 1e9
    }

    /// Lamports committed to open positions plus buys still in flight
    pub async fn total_invested_lamports(&self) -> u64 {
        self.state.read().await.committed_lamports()
    }

    /// Check if we already have an active or pending position for a mint
    pub async fn has_position(&self, mint: &Pubkey) -> bool {
        let state = self.state.read().await;
        state.positions.contains_key(mint) || state.pending.contains_key(mint)
    }

    /// Number of active open positions
    pub async fn position_count(&self) -> usize {
        self.state.read().await.positions.len()
    }

    fn persist(&self, state: &State) {
        let Some(path) = &self.state_path else { return };
        let records: Vec<SavedPosition> = state
            .positions
            .values()
            .map(|p| SavedPosition {
                mint: p.mint.to_string(),
                entry_price_sol: p.entry_price_sol,
                highest_price_sol: p.highest_price_sol,
                sol_invested_lamports: p.sol_invested_lamports,
                dev_wallet: p.dev_wallet.map(|d| d.to_string()),
                dev_initial_balance: p.dev_initial_balance,
                leaders: p.leaders.iter().map(|l| l.to_string()).collect(),
            })
            .collect();

        let result = serde_json::to_vec_pretty(&records)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, path)
            });
        if let Err(e) = result {
            warn!("⚠️ Failed to persist positions to {}: {}", path.display(), e);
        }
    }

    /// Recover open token positions from on-chain wallet token accounts on startup
    pub async fn recover_open_positions(
        &self,
        rpc_url: &str,
        wallet: &Pubkey,
        curve_cache: &Arc<RwLock<HashMap<Pubkey, crate::decoders::BondingCurveAccountPod>>>,
    ) {
        let saved = self
            .state_path
            .as_deref()
            .map(load_saved_positions)
            .unwrap_or_default();
        let client = reqwest::Client::new();

        let holdings = match fetch_token_holdings(&client, rpc_url, wallet).await {
            Ok(h) => h,
            Err(e) => {
                // Don't rewrite the state file from an incomplete view of the wallet
                warn!("⚠️ Position recovery failed, starting with no open positions: {:#}", e);
                return;
            }
        };

        for holding in holdings {
            let mint = holding.mint;
            let pod = match fetch_bonding_curve(&client, rpc_url, &mint).await {
                Ok(Some(pod)) => pod,
                // Not a Pump.fun token (or RPC hiccup): not ours to manage
                Ok(None) => continue,
                Err(e) => {
                    warn!("⚠️ Could not fetch bonding curve for {}: {:#}", mint, e);
                    continue;
                }
            };

            if pod.complete {
                warn!(
                    "🎓 Skipping recovery of {}: bonding curve has graduated, sell it on PumpSwap/Raydium manually",
                    mint
                );
                continue;
            }

            let bonding_curve = crate::constants::derive_bonding_curve(&mint).0;
            let spot_price = pod.current_spot_price_sol();
            let position = match saved.get(&mint) {
                Some(rec) => {
                    info!(
                        "🔄 Restored position for mint {} from state file: {} tokens, entry {:.10} SOL (current {:.10} SOL)",
                        mint, holding.amount, rec.entry_price_sol, spot_price
                    );
                    Position {
                        mint,
                        bonding_curve,
                        entry_price_sol: rec.entry_price_sol,
                        highest_price_sol: rec.highest_price_sol.max(spot_price),
                        token_balance: holding.amount,
                        sol_invested_lamports: rec.sol_invested_lamports,
                        dev_wallet: rec
                            .dev_wallet
                            .as_deref()
                            .and_then(|d| Pubkey::from_str(d).ok())
                            .or(Some(pod.creator)),
                        dev_initial_balance: rec.dev_initial_balance,
                        token_program: holding.token_program,
                        is_selling: false,
                        leaders: rec.leaders.iter().filter_map(|l| Pubkey::from_str(l).ok()).collect(),
                    }
                }
                None => {
                    // No record of the real entry: use current price and current value of the bag
                    let current_value = pod
                        .calculate_sell_output(holding.amount, 0)
                        .map(|c| c.sol_out)
                        .unwrap_or(0);
                    warn!(
                        "🔄 Restored untracked position for mint {}: {} tokens (no entry record; using current price {:.10} SOL and value {:.4} SOL as entry)",
                        mint, holding.amount, spot_price, current_value as f64 / 1e9
                    );
                    Position {
                        mint,
                        bonding_curve,
                        entry_price_sol: spot_price,
                        highest_price_sol: spot_price,
                        token_balance: holding.amount,
                        sol_invested_lamports: current_value,
                        dev_wallet: Some(pod.creator),
                        dev_initial_balance: 0,
                        token_program: holding.token_program,
                        is_selling: false,
                        leaders: Vec::new(),
                    }
                }
            };

            curve_cache.write().await.insert(bonding_curve, pod);
            let mut state = self.state.write().await;
            state.curve_to_mint.insert(bonding_curve, mint);
            state.positions.insert(mint, position);
        }

        // Rewrite the state file so positions closed while offline are dropped
        let state = self.state.read().await;
        self.persist(&state);
    }
}
