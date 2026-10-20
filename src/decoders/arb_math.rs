//! Cross-DEX Arbitrage Mathematical Formulation & Optimal Trade Size Engine
//!
//! Provides closed-form analytical solutions for maximizing net SOL profit in 2-hop
//! constant-product AMM arbitrage (e.g. Pump.fun, Raydium CPMM / AMM v4, Orca, Meteora).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConstantProductPool {
    pub sol_reserves: u64,   // x (in lamports)
    pub token_reserves: u64, // y (in raw token units)
    pub fee_bps: u64,        // e.g. 25 bps = 0.25%
}

impl ConstantProductPool {
    pub fn new(sol_reserves: u64, token_reserves: u64, fee_bps: u64) -> Self {
        Self {
            sol_reserves,
            token_reserves,
            fee_bps,
        }
    }

    /// Spot price of token in SOL
    #[inline(always)]
    pub fn spot_price_sol(&self) -> f64 {
        if self.token_reserves == 0 {
            return 0.0;
        }
        self.sol_reserves as f64 / self.token_reserves as f64
    }

    /// Simulate discrete swap of SOL in -> Token out using integer arithmetic with fee truncation
    pub fn swap_sol_for_tokens(&self, sol_in: u64) -> Option<u64> {
        if self.sol_reserves == 0 || self.token_reserves == 0 || sol_in == 0 {
            return None;
        }

        let sol_in_u128 = sol_in as u128;
        let fee_lamports = sol_in_u128.checked_mul(self.fee_bps as u128)? / 10_000;
        let effective_sol_in = sol_in_u128.checked_sub(fee_lamports)?;

        let x = self.sol_reserves as u128;
        let y = self.token_reserves as u128;
        let k = x.checked_mul(y)?;

        let new_x = x.checked_add(effective_sol_in)?;
        let new_y = k.checked_div(new_x)?;
        let tokens_out = y.checked_sub(new_y)?;

        Some(tokens_out as u64)
    }

    /// Simulate discrete swap of Token in -> SOL out using integer arithmetic with fee truncation
    pub fn swap_tokens_for_sol(&self, tokens_in: u64) -> Option<u64> {
        if self.sol_reserves == 0 || self.token_reserves == 0 || tokens_in == 0 {
            return None;
        }

        let tokens_in_u128 = tokens_in as u128;
        let fee_tokens = tokens_in_u128.checked_mul(self.fee_bps as u128)? / 10_000;
        let effective_tokens_in = tokens_in_u128.checked_sub(fee_tokens)?;

        let x = self.sol_reserves as u128;
        let y = self.token_reserves as u128;
        let k = x.checked_mul(y)?;

        let new_y = y.checked_add(effective_tokens_in)?;
        let new_x = k.checked_div(new_y)?;
        let sol_out = x.checked_sub(new_x)?;

        Some(sol_out as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArbDirection {
    PoolAToPoolB, // Buy tokens on Pool A with SOL, Sell tokens on Pool B for SOL
    PoolBToPoolA, // Buy tokens on Pool B with SOL, Sell tokens on Pool A for SOL
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArbOpportunity {
    pub direction: ArbDirection,
    pub optimal_sol_in_lamports: u64,
    pub expected_tokens_intermediate: u64,
    pub expected_sol_out_lamports: u64,
    pub gross_profit_lamports: u64,
    pub roi_bps: u64,
}

/// Calculate the exact optimal trade size Δx* and projected profit for a specific direction
pub fn calculate_optimal_arb_directional(
    buy_pool: &ConstantProductPool,
    sell_pool: &ConstantProductPool,
    direction: ArbDirection,
) -> Option<ArbOpportunity> {
    if buy_pool.sol_reserves == 0
        || buy_pool.token_reserves == 0
        || sell_pool.sol_reserves == 0
        || sell_pool.token_reserves == 0
    {
        return None;
    }

    let gamma_a = 1.0 - (buy_pool.fee_bps as f64 / 10_000.0);
    let gamma_b = 1.0 - (sell_pool.fee_bps as f64 / 10_000.0);

    let x_a = buy_pool.sol_reserves as f64;
    let y_a = buy_pool.token_reserves as f64;
    let x_b = sell_pool.sol_reserves as f64;
    let y_b = sell_pool.token_reserves as f64;

    // Check pre-condition: gamma_a * gamma_b * (x_b / y_b) > (x_a / y_a)
    let p_a = x_a / y_a;
    let p_b = x_b / y_b;
    if gamma_a * gamma_b * p_b <= p_a {
        return None; // No profitable opportunity
    }

    // N = gamma_a * gamma_b * x_b * y_a
    let n = gamma_a * gamma_b * x_b * y_a;
    // D0 = x_a * y_b
    let d0 = x_a * y_b;
    // D1 = gamma_a * y_b + gamma_a * gamma_b * y_a
    let d1 = (gamma_a * y_b) + (gamma_a * gamma_b * y_a);

    let sqrt_n_d0 = (n * d0).sqrt();
    if sqrt_n_d0 <= d0 {
        return None;
    }

    let delta_x_optimal = (sqrt_n_d0 - d0) / d1;
    if delta_x_optimal <= 0.0 {
        return None;
    }

    let delta_x_lamports = delta_x_optimal as u64;
    if delta_x_lamports == 0 {
        return None;
    }

    // Run discrete simulated swaps to account for integer rounding and exact protocol fee truncation
    let tokens_out = buy_pool.swap_sol_for_tokens(delta_x_lamports)?;
    if tokens_out == 0 {
        return None;
    }

    let sol_out = sell_pool.swap_tokens_for_sol(tokens_out)?;
    if sol_out <= delta_x_lamports {
        return None;
    }

    let gross_profit = sol_out - delta_x_lamports;
    let roi_bps = ((gross_profit as u128 * 10_000) / delta_x_lamports as u128) as u64;

    Some(ArbOpportunity {
        direction,
        optimal_sol_in_lamports: delta_x_lamports,
        expected_tokens_intermediate: tokens_out,
        expected_sol_out_lamports: sol_out,
        gross_profit_lamports: gross_profit,
        roi_bps,
    })
}

/// Evaluates both directions (A -> B and B -> A) and returns the most profitable opportunity, if any.
pub fn find_best_arb(
    pool_a: &ConstantProductPool,
    pool_b: &ConstantProductPool,
) -> Option<ArbOpportunity> {
    let opp_a_to_b = calculate_optimal_arb_directional(pool_a, pool_b, ArbDirection::PoolAToPoolB);
    let opp_b_to_a = calculate_optimal_arb_directional(pool_b, pool_a, ArbDirection::PoolBToPoolA);

    match (opp_a_to_b, opp_b_to_a) {
        (Some(a), Some(b)) => {
            if a.gross_profit_lamports >= b.gross_profit_lamports {
                Some(a)
            } else {
                Some(b)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}
