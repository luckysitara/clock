use crate::constants::{
    associated_token_program_id, derive_associated_bonding_curve, derive_associated_user_token,
    derive_bonding_curve, derive_bonding_curve_v2, derive_creator_vault, derive_fee_config,
    get_random_buyback_fee_recipient,
    pump_fee_program_id,
    system_program_id, BUY_DISCRIMINATOR_BYTES, PUMPFUN_EVENT_AUTHORITY, PUMPFUN_FEE_RECIPIENT,
    PUMPFUN_GLOBAL, PUMPFUN_PROGRAM, SELL_DISCRIMINATOR_BYTES,
};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use std::str::FromStr;

pub struct InstructionBuilder;

impl InstructionBuilder {
    /// Create Idempotent Associated Token Account (ATA) Instruction
    pub fn create_ata_idempotent(
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
        token_program: &Pubkey,
    ) -> Instruction {
        let (ata, _) = derive_associated_user_token(wallet, mint, token_program);
        let ata_program = associated_token_program_id();
        let sys_prog = system_program_id();

        // Account order: payer, ata, wallet, mint, system_program, token_program
        let accounts = vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(sys_prog, false),
            AccountMeta::new_readonly(*token_program, false),
        ];

        // 1 = CreateIdempotent instruction
        Instruction::new_with_bytes(ata_program, &[1], accounts)
    }

    /// Build SPL Token CloseAccount Instruction (discriminator 9)
    /// Closes the user ATA after selling tokens, instantly refunding the ~0.00204 SOL rent to wallet
    pub fn close_ata(
        account: &Pubkey,
        destination: &Pubkey,
        owner: &Pubkey,
        token_program: &Pubkey,
    ) -> Instruction {
        let accounts = vec![
            AccountMeta::new(*account, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*owner, true),
        ];
        Instruction::new_with_bytes(*token_program, &[9u8], accounts)
    }

    /// Build Pump.fun Buy Instruction (18 accounts layout)
    pub fn build_buy_instruction(
        user: &Pubkey,
        mint: &Pubkey,
        creator: &Pubkey,
        amount: u64,
        max_sol_cost: u64,
        token_program: &Pubkey,
    ) -> Instruction {
        let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
        let global = Pubkey::from_str(PUMPFUN_GLOBAL).unwrap();
        let fee_recipient = Pubkey::from_str(PUMPFUN_FEE_RECIPIENT).unwrap();
        let event_authority = Pubkey::from_str(PUMPFUN_EVENT_AUTHORITY).unwrap();
        let sys_prog = system_program_id();
        let protocol_fee_program = pump_fee_program_id();

        let (bonding_curve, _) = derive_bonding_curve(mint);
        let (associated_bonding_curve, _) = derive_associated_bonding_curve(&bonding_curve, mint, token_program);
        let (associated_user, _) = derive_associated_user_token(user, mint, token_program);
        let (creator_vault, _) = derive_creator_vault(creator);
        let (fee_config, _) = derive_fee_config();

        let (global_volume_accumulator, _) = Pubkey::find_program_address(&[b"global_volume_accumulator"], &pump_program);
        let (user_volume_accumulator, _) = Pubkey::find_program_address(&[b"user_volume_accumulator", user.as_ref()], &pump_program);
        let (bonding_curve_v2, _) = derive_bonding_curve_v2(mint);
        let buyback_fee_recipient = get_random_buyback_fee_recipient();

        let mut data = Vec::with_capacity(25);
        data.extend_from_slice(&BUY_DISCRIMINATOR_BYTES);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&max_sol_cost.to_le_bytes());
        data.push(0); // track_volume: OptionBool (0 = None)

        let accounts = vec![
            AccountMeta::new_readonly(global, false),
            AccountMeta::new(fee_recipient, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(bonding_curve, false),
            AccountMeta::new(associated_bonding_curve, false),
            AccountMeta::new(associated_user, false),
            AccountMeta::new(*user, true),
            AccountMeta::new_readonly(sys_prog, false),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new(creator_vault, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(pump_program, false),
            AccountMeta::new(global_volume_accumulator, false),
            AccountMeta::new(user_volume_accumulator, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(protocol_fee_program, false),
            AccountMeta::new_readonly(bonding_curve_v2, false),
            AccountMeta::new(buyback_fee_recipient, false),
        ];

        Instruction::new_with_bytes(pump_program, &data, accounts)
    }

    /// Build Pump.fun Sell Instruction (16 accounts; no volume accumulators)
    pub fn build_sell_instruction(
        user: &Pubkey,
        mint: &Pubkey,
        creator: &Pubkey,
        amount: u64,
        min_sol_output: u64,
        token_program: &Pubkey,
    ) -> Instruction {
        let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
        let global = Pubkey::from_str(PUMPFUN_GLOBAL).unwrap();
        let fee_recipient = Pubkey::from_str(PUMPFUN_FEE_RECIPIENT).unwrap();
        let event_authority = Pubkey::from_str(PUMPFUN_EVENT_AUTHORITY).unwrap();
        let sys_prog = system_program_id();
        let protocol_fee_program = pump_fee_program_id();

        let (bonding_curve, _) = derive_bonding_curve(mint);
        let (associated_bonding_curve, _) = derive_associated_bonding_curve(&bonding_curve, mint, token_program);
        let (associated_user, _) = derive_associated_user_token(user, mint, token_program);
        let (creator_vault, _) = derive_creator_vault(creator);
        let (fee_config, _) = derive_fee_config();

        let (bonding_curve_v2, _) = derive_bonding_curve_v2(mint);
        let buyback_fee_recipient = get_random_buyback_fee_recipient();

        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&SELL_DISCRIMINATOR_BYTES);
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&min_sol_output.to_le_bytes());

        let accounts = vec![
            AccountMeta::new_readonly(global, false),
            AccountMeta::new(fee_recipient, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(bonding_curve, false),
            AccountMeta::new(associated_bonding_curve, false),
            AccountMeta::new(associated_user, false),
            AccountMeta::new(*user, true),
            AccountMeta::new_readonly(sys_prog, false),
            AccountMeta::new(creator_vault, false),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(pump_program, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(protocol_fee_program, false),
            AccountMeta::new_readonly(bonding_curve_v2, false),
            AccountMeta::new(buyback_fee_recipient, false),
        ];

        Instruction::new_with_bytes(pump_program, &data, accounts)
    }

    /// Build Compute Budget Unit Price (Priority Fee in micro-lamports per CU)
    pub fn set_compute_unit_price(micro_lamports: u64) -> Instruction {
        let compute_budget_program =
            Pubkey::from_str("ComputeBudget111111111111111111111111111111").unwrap();
        let mut data = Vec::with_capacity(9);
        data.push(3); // SetComputeUnitPrice instruction discriminator
        data.extend_from_slice(&micro_lamports.to_le_bytes());
        Instruction::new_with_bytes(compute_budget_program, &data, vec![])
    }

    /// Build Compute Budget Unit Limit
    pub fn set_compute_unit_limit(units: u32) -> Instruction {
        let compute_budget_program =
            Pubkey::from_str("ComputeBudget111111111111111111111111111111").unwrap();
        let mut data = Vec::with_capacity(5);
        data.push(2); // SetComputeUnitLimit instruction discriminator
        data.extend_from_slice(&units.to_le_bytes());
        Instruction::new_with_bytes(compute_budget_program, &data, vec![])
    }

    /// Build Raydium AMM v4 Swap Instruction (SwapBaseIn discriminator 9)
    pub fn build_raydium_amm_v4_swap(
        accounts: &RaydiumAmmAccounts,
        amount_in: u64,
        min_amount_out: u64,
    ) -> Instruction {
        let raydium_program = crate::constants::raydium_amm_v4_program_id();
        let spl_token = crate::constants::spl_token_program_id();

        let mut data = Vec::with_capacity(17);
        data.push(9u8); // SwapBaseIn discriminator
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_amount_out.to_le_bytes());

        let account_metas = vec![
            AccountMeta::new_readonly(spl_token, false),
            AccountMeta::new(accounts.amm_id, false),
            AccountMeta::new_readonly(accounts.amm_authority, false),
            AccountMeta::new(accounts.amm_open_orders, false),
            AccountMeta::new(accounts.amm_target_orders, false),
            AccountMeta::new(accounts.pool_coin_token_account, false),
            AccountMeta::new(accounts.pool_pc_token_account, false),
            AccountMeta::new_readonly(accounts.serum_program_id, false),
            AccountMeta::new(accounts.serum_market, false),
            AccountMeta::new(accounts.serum_bids, false),
            AccountMeta::new(accounts.serum_asks, false),
            AccountMeta::new(accounts.serum_event_queue, false),
            AccountMeta::new(accounts.serum_coin_vault_account, false),
            AccountMeta::new(accounts.serum_pc_vault_account, false),
            AccountMeta::new_readonly(accounts.serum_vault_signer, false),
            AccountMeta::new(accounts.user_source_token_account, false),
            AccountMeta::new(accounts.user_destination_token_account, false),
            AccountMeta::new(accounts.user_source_owner, true),
        ];

        Instruction::new_with_bytes(raydium_program, &data, account_metas)
    }

    /// Clone and customize a live PumpSwap AMM Buy instruction for our wallet
    pub fn build_pumpswap_amm_buy_from_template(
        original_accounts: &[AccountMeta],
        user: &Pubkey,
        user_wsol_account: &Pubkey,
        user_destination_token: &Pubkey,
        amount_tokens: u64,
        max_sol_cost_lamports: u64,
    ) -> Option<Instruction> {
        if original_accounts.len() < 7 {
            return None;
        }

        let mut metas = original_accounts.to_vec();
        // Account 1: User (signer)
        metas[1] = AccountMeta::new(*user, true);
        // Account 2: User WSOL Source Account (writable)
        metas[2] = AccountMeta::new(*user_wsol_account, false);
        // Account 6: User Destination Token ATA (writable)
        metas[6] = AccountMeta::new(*user_destination_token, false);

        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&BUY_DISCRIMINATOR_BYTES);
        data.extend_from_slice(&amount_tokens.to_le_bytes());
        data.extend_from_slice(&max_sol_cost_lamports.to_le_bytes());

        let pumpswap_prog = crate::constants::pumpswap_amm_program_id();
        Some(Instruction::new_with_bytes(pumpswap_prog, &data, metas))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RaydiumAmmAccounts {
    pub amm_id: Pubkey,
    pub amm_authority: Pubkey,
    pub amm_open_orders: Pubkey,
    pub amm_target_orders: Pubkey,
    pub pool_coin_token_account: Pubkey,
    pub pool_pc_token_account: Pubkey,
    pub serum_program_id: Pubkey,
    pub serum_market: Pubkey,
    pub serum_bids: Pubkey,
    pub serum_asks: Pubkey,
    pub serum_event_queue: Pubkey,
    pub serum_coin_vault_account: Pubkey,
    pub serum_pc_vault_account: Pubkey,
    pub serum_vault_signer: Pubkey,
    pub user_source_token_account: Pubkey,
    pub user_destination_token_account: Pubkey,
    pub user_source_owner: Pubkey,
}

