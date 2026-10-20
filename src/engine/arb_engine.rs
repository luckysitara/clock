//! Arbitrage Engine
//!
//! Evaluates cross-DEX price discrepancies in real-time and constructs atomic
//! multi-hop transactions with on-chain minimum profit enforcement.

use solana_sdk::{instruction::Instruction, pubkey::Pubkey};
use crate::{
    decoders::arb_math::{find_best_arb, ArbDirection, ArbOpportunity, ConstantProductPool},
    engine::instruction_builder::{InstructionBuilder, RaydiumAmmAccounts},
};

pub struct ArbitrageEngine {
    pub min_profit_lamports: u64,
    pub max_trade_sol_lamports: u64,
}

impl ArbitrageEngine {
    pub fn new(min_profit_lamports: u64, max_trade_sol_lamports: u64) -> Self {
        Self {
            min_profit_lamports,
            max_trade_sol_lamports,
        }
    }

    /// Evaluates if an actionable, profitable arbitrage opportunity exists between two pools
    pub fn evaluate_pair(
        &self,
        pool_a: &ConstantProductPool,
        pool_b: &ConstantProductPool,
    ) -> Option<ArbOpportunity> {
        let mut opp = find_best_arb(pool_a, pool_b)?;

        // Cap input size if optimal size exceeds maximum trade limit
        if opp.optimal_sol_in_lamports > self.max_trade_sol_lamports {
            opp.optimal_sol_in_lamports = self.max_trade_sol_lamports;
            // Recalculate outputs with capped input
            let (buy_pool, sell_pool) = match opp.direction {
                ArbDirection::PoolAToPoolB => (pool_a, pool_b),
                ArbDirection::PoolBToPoolA => (pool_b, pool_a),
            };

            let tokens_out = buy_pool.swap_sol_for_tokens(opp.optimal_sol_in_lamports)?;
            let sol_out = sell_pool.swap_tokens_for_sol(tokens_out)?;

            if sol_out <= opp.optimal_sol_in_lamports {
                return None;
            }

            opp.expected_tokens_intermediate = tokens_out;
            opp.expected_sol_out_lamports = sol_out;
            opp.gross_profit_lamports = sol_out - opp.optimal_sol_in_lamports;
            opp.roi_bps = ((opp.gross_profit_lamports as u128 * 10_000)
                / opp.optimal_sol_in_lamports as u128) as u64;
        }

        // Must meet minimum profit threshold
        if opp.gross_profit_lamports < self.min_profit_lamports {
            return None;
        }

        Some(opp)
    }

    /// Builds atomic instructions for a 2-hop Arbitrage between Pump.fun and Raydium AMM v4
    ///
    /// The second swap enforces `min_sol_out >= input_sol + min_profit`, ensuring that
    /// if prices slip on either DEX, the entire Solana transaction reverts automatically!
    pub fn build_pumpfun_raydium_arb(
        &self,
        user: &Pubkey,
        mint: &Pubkey,
        creator: &Pubkey,
        token_program: &Pubkey,
        raydium_accounts: &RaydiumAmmAccounts,
        opp: &ArbOpportunity,
    ) -> Vec<Instruction> {
        let mut instructions = Vec::with_capacity(3);

        // 1. Ensure user's associated token account exists idempotently
        let create_ata =
            InstructionBuilder::create_ata_idempotent(user, user, mint, token_program);
        instructions.push(create_ata);

        // 2. Minimum SOL floor to enforce atomic profitability:
        // We require at least: input SOL + configured minimum profit
        let strictly_enforced_sol_floor =
            opp.optimal_sol_in_lamports.saturating_add(self.min_profit_lamports);

        match opp.direction {
            ArbDirection::PoolAToPoolB => {
                // Leg 1: Buy on Pump.fun (Pool A)
                let buy_pump = InstructionBuilder::build_buy_instruction(
                    user,
                    mint,
                    creator,
                    opp.expected_tokens_intermediate,
                    opp.optimal_sol_in_lamports,
                    token_program,
                );
                instructions.push(buy_pump);

                // Leg 2: Sell on Raydium AMM (Pool B)
                let sell_raydium = InstructionBuilder::build_raydium_amm_v4_swap(
                    raydium_accounts,
                    opp.expected_tokens_intermediate,
                    strictly_enforced_sol_floor,
                );
                instructions.push(sell_raydium);
            }
            ArbDirection::PoolBToPoolA => {
                // Leg 1: Buy on Raydium AMM (Pool B)
                let buy_raydium = InstructionBuilder::build_raydium_amm_v4_swap(
                    raydium_accounts,
                    opp.optimal_sol_in_lamports,
                    opp.expected_tokens_intermediate,
                );
                instructions.push(buy_raydium);

                // Leg 2: Sell on Pump.fun (Pool A)
                let sell_pump = InstructionBuilder::build_sell_instruction(
                    user,
                    mint,
                    creator,
                    opp.expected_tokens_intermediate,
                    strictly_enforced_sol_floor,
                    token_program,
                );
                instructions.push(sell_pump);
            }
        }

        instructions
    }
}
