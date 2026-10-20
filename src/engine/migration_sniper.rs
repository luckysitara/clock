use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use crate::decoders::BondingCurveAccountPod;

/// High-conviction Pump.fun Graduation / Raydium Migration Sniper
pub struct MigrationSniper {
    /// Bullet size in lamports (e.g. 0.10 SOL)
    pub bullet_size_lamports: u64,
    /// Maximum slippage tolerance in basis points (500 bps = 5%)
    pub slippage_bps: u64,
    /// Set of already sniped mints to prevent duplicate entries
    seen_mints: HashSet<Pubkey>,
    /// Minimum SOL threshold on curve to be considered graduating (84 SOL)
    pub graduation_sol_threshold: u64,
}

impl MigrationSniper {
    pub fn new(bullet_size_lamports: u64) -> Self {
        Self {
            bullet_size_lamports,
            slippage_bps: 500, // 5% slippage protection
            seen_mints: HashSet::new(),
            graduation_sol_threshold: 84_000_000_000, // 84 SOL
        }
    }

    /// Evaluates if a curve has graduated or reached 100% threshold
    pub fn is_graduating(&mut self, mint: &Pubkey, curve: &BondingCurveAccountPod) -> bool {
        if self.seen_mints.contains(mint) {
            return false;
        }

        let is_complete = curve.complete || curve.real_sol_reserves >= self.graduation_sol_threshold;
        if is_complete {
            self.seen_mints.insert(*mint);
            return true;
        }

        false
    }

    /// Mark a mint as handled
    pub fn mark_handled(&mut self, mint: &Pubkey) {
        self.seen_mints.insert(*mint);
    }
}
