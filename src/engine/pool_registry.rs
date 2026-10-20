use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
};
use std::{collections::HashMap, fs::File, io::Read, str::FromStr};

use crate::{
    constants::{
        derive_associated_user_token, derive_bonding_curve, raydium_amm_v4_program_id,
        spl_token_program_id, wsol_mint,
    },
    decoders::arb_math::{ArbDirection, ArbOpportunity, ConstantProductPool},
    engine::instruction_builder::{InstructionBuilder, RaydiumAmmAccounts},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTradeOrder {
    pub step: usize,
    pub pool_target: String,
    pub is_buy: bool,
    pub amount_lamports: u64,
    pub desc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTokenEntry {
    pub id: usize,
    pub symbol: String,
    pub name: String,
    pub mint: String,
    pub primary_dex: String,
    pub pool_a_name: String,
    pub pool_a_sol_reserves: u64,
    pub pool_a_token_reserves: u64,
    pub pool_a_fee_bps: u64,
    pub pool_b_name: String,
    pub pool_b_sol_reserves: u64,
    pub pool_b_token_reserves: u64,
    pub pool_b_fee_bps: u64,
    pub trades: Vec<HistoricalTradeOrder>,
}

#[derive(Debug, Clone)]
pub struct DualPoolPair {
    pub id: usize,
    pub symbol: String,
    pub name: String,
    pub mint: Pubkey,
    pub bonding_curve: Pubkey,
    pub pool_a_pump: ConstantProductPool,
    pub pool_b_raydium: ConstantProductPool,
    pub trades: Vec<HistoricalTradeOrder>,
}

impl DualPoolPair {
    /// Deterministically derive Raydium AMM accounts for this mint & user
    pub fn derive_raydium_accounts(&self, payer: &Pubkey) -> RaydiumAmmAccounts {
        let raydium_program = raydium_amm_v4_program_id();
        let spl_token = spl_token_program_id();
        let wsol = wsol_mint();

        // 1. Raydium AMM Authority PDA
        let (amm_authority, _) = Pubkey::find_program_address(&[b"amm authority"], &raydium_program);

        // 2. Serum Market PDA derived from mint
        let (serum_market, _) = Pubkey::find_program_address(&[b"serum_market", self.mint.as_ref()], &raydium_program);
        let (amm_id, _) = Pubkey::find_program_address(
            &[raydium_program.as_ref(), serum_market.as_ref(), b"amm_associated_seed"],
            &raydium_program,
        );

        let (amm_open_orders, _) = Pubkey::find_program_address(&[b"open_orders", amm_id.as_ref()], &raydium_program);
        let (amm_target_orders, _) = Pubkey::find_program_address(&[b"target_orders", amm_id.as_ref()], &raydium_program);
        let (pool_coin_token_account, _) = Pubkey::find_program_address(&[b"coin_vault", amm_id.as_ref()], &raydium_program);
        let (pool_pc_token_account, _) = Pubkey::find_program_address(&[b"pc_vault", amm_id.as_ref()], &raydium_program);

        let (serum_bids, _) = Pubkey::find_program_address(&[b"serum_bids", serum_market.as_ref()], &raydium_program);
        let (serum_asks, _) = Pubkey::find_program_address(&[b"serum_asks", serum_market.as_ref()], &raydium_program);
        let (serum_event_queue, _) = Pubkey::find_program_address(&[b"serum_event_queue", serum_market.as_ref()], &raydium_program);
        let (serum_coin_vault_account, _) = Pubkey::find_program_address(&[b"serum_coin_vault", serum_market.as_ref()], &raydium_program);
        let (serum_pc_vault_account, _) = Pubkey::find_program_address(&[b"serum_pc_vault", serum_market.as_ref()], &raydium_program);
        let (serum_vault_signer, _) = Pubkey::find_program_address(&[b"serum_vault_signer", serum_market.as_ref()], &raydium_program);

        let (user_token_ata, _) = derive_associated_user_token(payer, &self.mint, &spl_token);
        let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol, &spl_token);

        RaydiumAmmAccounts {
            amm_id,
            amm_authority,
            amm_open_orders,
            amm_target_orders,
            pool_coin_token_account,
            pool_pc_token_account,
            serum_program_id: raydium_program,
            serum_market,
            serum_bids,
            serum_asks,
            serum_event_queue,
            serum_coin_vault_account,
            serum_pc_vault_account,
            serum_vault_signer,
            user_source_token_account: user_token_ata,
            user_destination_token_account: user_wsol_ata,
            user_source_owner: *payer,
        }
    }

    /// Build the complete 4-instruction atomic cross-DEX arbitrage bundle:
    /// 1. Create User ATA (idempotent)
    /// 2. Leg 1 Swap (Buy on cheaper DEX)
    /// 3. Leg 2 Swap (Sell on higher DEX with min SOL floor)
    /// 4. Close User ATA (instantly refunding the ~0.00204 SOL rent to wallet!)
    pub fn build_atomic_arb_bundle(
        &self,
        payer: &Pubkey,
        opp: &ArbOpportunity,
    ) -> Vec<Instruction> {
        let spl_token = spl_token_program_id();
        let (user_token_ata, _) = derive_associated_user_token(payer, &self.mint, &spl_token);
        let mut ray_accounts = self.derive_raydium_accounts(payer);
        let creator = Pubkey::default();

        let mut instructions = Vec::with_capacity(5);

        // Setup: Create User ATA idempotent
        instructions.push(InstructionBuilder::create_ata_idempotent(
            payer,
            payer,
            &self.mint,
            &spl_token,
        ));

        match opp.direction {
            ArbDirection::PoolAToPoolB => {
                // Leg 1: Buy tokens on Pump.fun with sol_in
                let buy_ix = InstructionBuilder::build_buy_instruction(
                    payer,
                    &self.mint,
                    &creator,
                    opp.optimal_sol_in_lamports,
                    opp.optimal_sol_in_lamports,
                    &spl_token,
                );
                instructions.push(buy_ix);

                // Leg 2: Sell tokens on Raydium AMM for SOL (min_sol_out enforced!)
                ray_accounts.user_source_token_account = user_token_ata;
                let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol_mint(), &spl_token);
                ray_accounts.user_destination_token_account = user_wsol_ata;

                let ray_sell_ix = InstructionBuilder::build_raydium_amm_v4_swap(
                    &ray_accounts,
                    opp.expected_tokens_intermediate,
                    opp.expected_sol_out_lamports,
                );
                instructions.push(ray_sell_ix);
            }

            ArbDirection::PoolBToPoolA => {
                // Leg 1: Buy tokens on Raydium AMM with sol_in
                let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol_mint(), &spl_token);
                ray_accounts.user_source_token_account = user_wsol_ata;
                ray_accounts.user_destination_token_account = user_token_ata;

                let ray_buy_ix = InstructionBuilder::build_raydium_amm_v4_swap(
                    &ray_accounts,
                    opp.optimal_sol_in_lamports,
                    opp.expected_tokens_intermediate,
                );
                instructions.push(ray_buy_ix);

                // Leg 2: Sell tokens on Pump.fun for SOL (min_sol_out enforced!)
                let sell_ix = InstructionBuilder::build_sell_instruction(
                    payer,
                    &self.mint,
                    &creator,
                    opp.expected_tokens_intermediate,
                    opp.expected_sol_out_lamports,
                    &spl_token,
                );
                instructions.push(sell_ix);
            }
        }

        // 3. Cleanup: Close User Token ATA & refund the ~0.00204 SOL rent!
        let close_ix = InstructionBuilder::close_ata(&user_token_ata, payer, payer, &spl_token);
        instructions.push(close_ix);

        instructions
    }
}

pub fn derive_raydium_accounts_for_mint(mint: &Pubkey, payer: &Pubkey) -> RaydiumAmmAccounts {
    let raydium_program = raydium_amm_v4_program_id();
    let spl_token = spl_token_program_id();
    let wsol = wsol_mint();

    let (amm_authority, _) = Pubkey::find_program_address(&[b"amm authority"], &raydium_program);
    let (serum_market, _) = Pubkey::find_program_address(&[b"serum_market", mint.as_ref()], &raydium_program);
    let (amm_id, _) = Pubkey::find_program_address(
        &[raydium_program.as_ref(), serum_market.as_ref(), b"amm_associated_seed"],
        &raydium_program,
    );

    let (amm_open_orders, _) = Pubkey::find_program_address(&[b"open_orders", amm_id.as_ref()], &raydium_program);
    let (amm_target_orders, _) = Pubkey::find_program_address(&[b"target_orders", amm_id.as_ref()], &raydium_program);
    let (pool_coin_token_account, _) = Pubkey::find_program_address(&[b"coin_vault", amm_id.as_ref()], &raydium_program);
    let (pool_pc_token_account, _) = Pubkey::find_program_address(&[b"pc_vault", amm_id.as_ref()], &raydium_program);

    let (serum_bids, _) = Pubkey::find_program_address(&[b"serum_bids", serum_market.as_ref()], &raydium_program);
    let (serum_asks, _) = Pubkey::find_program_address(&[b"serum_asks", serum_market.as_ref()], &raydium_program);
    let (serum_event_queue, _) = Pubkey::find_program_address(&[b"serum_event_queue", serum_market.as_ref()], &raydium_program);
    let (serum_coin_vault_account, _) = Pubkey::find_program_address(&[b"serum_coin_vault", serum_market.as_ref()], &raydium_program);
    let (serum_pc_vault_account, _) = Pubkey::find_program_address(&[b"serum_pc_vault", serum_market.as_ref()], &raydium_program);
    let (serum_vault_signer, _) = Pubkey::find_program_address(&[b"serum_vault_signer", serum_market.as_ref()], &raydium_program);

    let (user_token_ata, _) = derive_associated_user_token(payer, mint, &spl_token);
    let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol, &spl_token);

    RaydiumAmmAccounts {
        amm_id,
        amm_authority,
        amm_open_orders,
        amm_target_orders,
        pool_coin_token_account,
        pool_pc_token_account,
        serum_program_id: raydium_program,
        serum_market,
        serum_bids,
        serum_asks,
        serum_event_queue,
        serum_coin_vault_account,
        serum_pc_vault_account,
        serum_vault_signer,
        user_source_token_account: user_token_ata,
        user_destination_token_account: user_wsol_ata,
        user_source_owner: *payer,
    }
}

pub fn build_atomic_arb_bundle_for_mint(
    payer: &Pubkey,
    mint: &Pubkey,
    opp: &ArbOpportunity,
) -> Vec<Instruction> {
    let spl_token = spl_token_program_id();
    let (user_token_ata, _) = derive_associated_user_token(payer, mint, &spl_token);
    let mut ray_accounts = derive_raydium_accounts_for_mint(mint, payer);
    let creator = Pubkey::default();

    let mut instructions = Vec::with_capacity(2);

    match opp.direction {
        ArbDirection::PoolAToPoolB => {
            let buy_ix = InstructionBuilder::build_buy_instruction(
                payer,
                mint,
                &creator,
                opp.optimal_sol_in_lamports,
                opp.optimal_sol_in_lamports,
                &spl_token,
            );
            instructions.push(buy_ix);

            ray_accounts.user_source_token_account = user_token_ata;
            let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol_mint(), &spl_token);
            ray_accounts.user_destination_token_account = user_wsol_ata;

            let ray_sell_ix = InstructionBuilder::build_raydium_amm_v4_swap(
                &ray_accounts,
                opp.expected_tokens_intermediate,
                opp.expected_sol_out_lamports,
            );
            instructions.push(ray_sell_ix);
        }

        ArbDirection::PoolBToPoolA => {
            let (user_wsol_ata, _) = derive_associated_user_token(payer, &wsol_mint(), &spl_token);
            ray_accounts.user_source_token_account = user_wsol_ata;
            ray_accounts.user_destination_token_account = user_token_ata;

            let ray_buy_ix = InstructionBuilder::build_raydium_amm_v4_swap(
                &ray_accounts,
                opp.optimal_sol_in_lamports,
                opp.expected_tokens_intermediate,
            );
            instructions.push(ray_buy_ix);

            let sell_ix = InstructionBuilder::build_sell_instruction(
                payer,
                mint,
                &creator,
                opp.expected_tokens_intermediate,
                opp.expected_sol_out_lamports,
                &spl_token,
            );
            instructions.push(sell_ix);
        }
    }

    instructions
}

pub struct PoolRegistry {
    pairs: Vec<DualPoolPair>,
    pair_map: HashMap<Pubkey, usize>,
}

impl PoolRegistry {
    pub fn load_from_json(path: &str) -> Result<Self> {
        let mut file = File::open(path).context(format!("Failed to open dataset at {}", path))?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let raw_entries: Vec<HistoricalTokenEntry> = serde_json::from_str(&content)
            .context("Failed to parse historical tokens JSON")?;

        let mut pairs = Vec::with_capacity(raw_entries.len());
        let mut pair_map = HashMap::with_capacity(raw_entries.len());

        for entry in raw_entries {
            if let Ok(mint) = Pubkey::from_str(&entry.mint) {
                let (bonding_curve, _) = derive_bonding_curve(&mint);
                let pool_a = ConstantProductPool::new(
                    entry.pool_a_sol_reserves,
                    entry.pool_a_token_reserves,
                    entry.pool_a_fee_bps,
                );
                let pool_b = ConstantProductPool::new(
                    entry.pool_b_sol_reserves,
                    entry.pool_b_token_reserves,
                    entry.pool_b_fee_bps,
                );

                let pair = DualPoolPair {
                    id: entry.id,
                    symbol: entry.symbol,
                    name: entry.name,
                    mint,
                    bonding_curve,
                    pool_a_pump: pool_a,
                    pool_b_raydium: pool_b,
                    trades: entry.trades,
                };

                let idx = pairs.len();
                pair_map.insert(mint, idx);
                pairs.push(pair);
            }
        }

        Ok(Self { pairs, pair_map })
    }

    pub fn pairs(&self) -> &[DualPoolPair] {
        &self.pairs
    }

    pub fn get_by_mint(&self, mint: &Pubkey) -> Option<&DualPoolPair> {
        self.pair_map.get(mint).map(|&idx| &self.pairs[idx])
    }
}
