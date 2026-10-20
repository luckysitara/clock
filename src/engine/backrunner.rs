use solana_sdk::pubkey::Pubkey;

/// Toxic Flow / Retail Dislocation Backrunner
pub struct BackrunEngine {
    /// Minimum retail swap size in SOL to trigger dislocation check (e.g. 5.0 SOL)
    pub min_retail_sol_spent: u64,
    /// Minimum price dislocation percentage (in basis points, e.g. 100 bps = 1.0%)
    pub min_dislocation_bps: u64,
}

#[derive(Debug, Clone)]
pub struct BackrunSignal {
    pub mint: Pubkey,
    pub retail_swapper: Pubkey,
    pub sol_spent: u64,
    pub dislocation_bps: u64,
    pub recommended_size_lamports: u64,
}

impl BackrunEngine {
    pub fn new(min_retail_sol_spent: u64, min_dislocation_bps: u64) -> Self {
        Self {
            min_retail_sol_spent,
            min_dislocation_bps,
        }
    }

    /// Evaluates if an incoming retail swap created a backrunnable price dislocation
    pub fn evaluate_swap(
        &self,
        mint: &Pubkey,
        swapper: &Pubkey,
        sol_spent_lamports: u64,
        pool_sol_reserves: u64,
    ) -> Option<BackrunSignal> {
        if sol_spent_lamports < self.min_retail_sol_spent {
            return None;
        }

        if pool_sol_reserves == 0 {
            return None;
        }

        // Approximate price impact = sol_spent / pool_sol_reserves
        let impact_bps = (sol_spent_lamports as u128 * 10_000 / pool_sol_reserves as u128) as u64;

        if impact_bps >= self.min_dislocation_bps {
            // Recommend sizing at ~10% to 20% of the retail impact
            let size = (sol_spent_lamports / 5).clamp(100_000_000, 450_000_000); // 0.10 to 0.45 SOL bullet
            Some(BackrunSignal {
                mint: *mint,
                retail_swapper: *swapper,
                sol_spent: sol_spent_lamports,
                dislocation_bps: impact_bps,
                recommended_size_lamports: size,
            })
        } else {
            None
        }
    }
}
