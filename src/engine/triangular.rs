use solana_sdk::pubkey::Pubkey;
use crate::decoders::arb_math::ConstantProductPool;

#[derive(Debug, Clone)]
pub struct TriangularCycle {
    pub name: String,
    pub asset_b_name: String,
    pub asset_c_name: String,
    pub pool_1_sol_to_b: ConstantProductPool,
    pub pool_2_b_to_c: ConstantProductPool,
    pub pool_3_c_to_sol: ConstantProductPool,
    pub pool_1_program: Pubkey,
    pub pool_2_program: Pubkey,
    pub pool_3_program: Pubkey,
}

#[derive(Debug, Clone)]
pub struct TriangularOpportunity {
    pub cycle_name: String,
    pub optimal_sol_in_lamports: u64,
    pub expected_sol_out_lamports: u64,
    pub gross_profit_lamports: u64,
    pub roi_bps: u64,
}

impl TriangularCycle {
    pub fn new(
        name: &str,
        asset_b_name: &str,
        asset_c_name: &str,
        pool_1: ConstantProductPool,
        pool_2: ConstantProductPool,
        pool_3: ConstantProductPool,
        p1_prog: Pubkey,
        p2_prog: Pubkey,
        p3_prog: Pubkey,
    ) -> Self {
        Self {
            name: name.to_string(),
            asset_b_name: asset_b_name.to_string(),
            asset_c_name: asset_c_name.to_string(),
            pool_1_sol_to_b: pool_1,
            pool_2_b_to_c: pool_2,
            pool_3_c_to_sol: pool_3,
            pool_1_program: p1_prog,
            pool_2_program: p2_prog,
            pool_3_program: p3_prog,
        }
    }

    /// Evaluates 3-hop cyclic profitability for a test input SOL amount
    pub fn evaluate_amount(&self, sol_in: u64, min_profit_lamports: u64) -> Option<TriangularOpportunity> {
        if sol_in == 0 {
            return None;
        }

        // Hop 1: SOL -> Asset B
        let out_b = self.pool_1_sol_to_b.swap_sol_for_tokens(sol_in)?;
        if out_b == 0 {
            return None;
        }

        // Hop 2: Asset B -> Asset C (using constant-product formula dy = (y * dx) / (x + dx))
        let dx_with_fee = (out_b as u128) * (10_000 - self.pool_2_b_to_c.fee_bps as u128);
        let num = dx_with_fee * (self.pool_2_b_to_c.token_reserves as u128);
        let den = ((self.pool_2_b_to_c.sol_reserves as u128) * 10_000) + dx_with_fee;
        if den == 0 {
            return None;
        }
        let out_c = (num / den) as u64;
        if out_c == 0 {
            return None;
        }

        // Hop 3: Asset C -> SOL
        let sol_out = self.pool_3_c_to_sol.swap_tokens_for_sol(out_c)?;

        if sol_out > sol_in + min_profit_lamports {
            let gross_profit = sol_out - sol_in;
            let roi_bps = (gross_profit as u128 * 10_000 / sol_in as u128) as u64;
            Some(TriangularOpportunity {
                cycle_name: self.name.clone(),
                optimal_sol_in_lamports: sol_in,
                expected_sol_out_lamports: sol_out,
                gross_profit_lamports: gross_profit,
                roi_bps,
            })
        } else {
            None
        }
    }

    /// Finds optimal input by scanning discretely between 0.05 SOL and max_sol (e.g. 0.45 SOL)
    pub fn find_optimal(&self, max_sol_lamports: u64, min_profit_lamports: u64) -> Option<TriangularOpportunity> {
        let step = 50_000_000; // 0.05 SOL steps
        let mut best: Option<TriangularOpportunity> = None;

        let mut current = 50_000_000;
        while current <= max_sol_lamports {
            if let Some(opp) = self.evaluate_amount(current, min_profit_lamports) {
                if let Some(ref current_best) = best {
                    if opp.gross_profit_lamports > current_best.gross_profit_lamports {
                        best = Some(opp);
                    }
                } else {
                    best = Some(opp);
                }
            }
            current += step;
        }

        best
    }
}
