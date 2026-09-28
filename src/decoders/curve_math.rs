use crate::decoders::zero_copy::BondingCurveAccountPod;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveCalculationResult {
    pub tokens_out: u64,
    pub max_sol_cost: u64,
    pub effective_price_sol: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SellCalculationResult {
    pub sol_out: u64,
    pub min_sol_output: u64,
    pub effective_price_sol: f64,
}

impl BondingCurveAccountPod {
    /// Calculate the token output and max SOL cost (including slippage) for a Buy order
    pub fn calculate_buy_output(
        &self,
        sol_in_lamports: u64,
        slippage_bps: u64,
    ) -> Option<CurveCalculationResult> {
        if self.complete || self.virtual_sol_reserves == 0 || self.virtual_token_reserves == 0 {
            return None;
        }

        // Use u128 to prevent overflow when computing the invariant k = x * y
        let x = self.virtual_sol_reserves as u128;
        let y = self.virtual_token_reserves as u128;
        let k = x.checked_mul(y)?;

        let delta_x = sol_in_lamports as u128;
        let new_x = x.checked_add(delta_x)?;

        let new_y = k.checked_div(new_x)?;
        let mut tokens_out = y.checked_sub(new_y)?;

        // Cap at available real token reserves
        if tokens_out > self.real_token_reserves as u128 {
            tokens_out = self.real_token_reserves as u128;
        }

        let tokens_out_u64 = tokens_out as u64;
        if tokens_out_u64 == 0 {
            return None;
        }

        // Apply slippage to max SOL cost
        // e.g. 500 bps = 5% extra SOL willingness
        let max_sol_cost = ((sol_in_lamports as u128)
            .checked_mul(10_000 + slippage_bps as u128)?
            .checked_div(10_000)?) as u64;

        let effective_price_sol = (sol_in_lamports as f64 / 1_000_000_000.0)
            / (tokens_out_u64 as f64 / 1_000_000.0);

        Some(CurveCalculationResult {
            tokens_out: tokens_out_u64,
            max_sol_cost,
            effective_price_sol,
        })
    }

    /// Calculate the SOL output and min SOL floor (with slippage) for a Sell order
    pub fn calculate_sell_output(
        &self,
        tokens_in: u64,
        slippage_bps: u64,
    ) -> Option<SellCalculationResult> {
        if self.virtual_sol_reserves == 0 || self.virtual_token_reserves == 0 {
            return None;
        }

        let x = self.virtual_sol_reserves as u128;
        let y = self.virtual_token_reserves as u128;
        let k = x.checked_mul(y)?;

        let delta_y = tokens_in as u128;
        let new_y = y.checked_add(delta_y)?;

        let new_x = k.checked_div(new_y)?;
        let mut sol_out = x.checked_sub(new_x)?;

        // Cap at real SOL reserves in curve
        if sol_out > self.real_sol_reserves as u128 {
            sol_out = self.real_sol_reserves as u128;
        }

        let sol_out_u64 = sol_out as u64;

        // Apply slippage tolerance to min SOL output floor
        let min_sol_output = ((sol_out as u128)
            .checked_mul(10_000 - slippage_bps.min(9_999) as u128)?
            .checked_div(10_000)?) as u64;

        let effective_price_sol = (sol_out_u64 as f64 / 1_000_000_000.0)
            / (tokens_in as f64 / 1_000_000.0);

        Some(SellCalculationResult {
            sol_out: sol_out_u64,
            min_sol_output,
            effective_price_sol,
        })
    }

    /// Spot price in SOL per token
    #[inline(always)]
    pub fn current_spot_price_sol(&self) -> f64 {
        if self.virtual_token_reserves == 0 {
            return 0.0;
        }
        (self.virtual_sol_reserves as f64 / 1_000_000_000.0)
            / (self.virtual_token_reserves as f64 / 1_000_000.0)
    }

    /// Progress toward graduation (0.0 to 100.0%)
    #[inline(always)]
    pub fn graduation_progress_pct(&self) -> f64 {
        if self.complete {
            return 100.0;
        }
        ((self.real_sol_reserves as f64 / 85_000_000_000.0) * 100.0).min(100.0)
    }
}
