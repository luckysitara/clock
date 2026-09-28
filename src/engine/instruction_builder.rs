use crate::constants::{
    associated_token_program_id, derive_associated_bonding_curve, derive_associated_user_token,
    derive_bonding_curve, derive_bonding_curve_v2, derive_creator_vault, derive_fee_config,
    derive_global_volume_accumulator, derive_user_volume_accumulator,
    get_random_buyback_fee_recipient, pump_fee_program_id, spl_token_program_id,
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
    pub fn create_ata_idempotent(payer: &Pubkey, wallet: &Pubkey, mint: &Pubkey) -> Instruction {
        let (ata, _) = derive_associated_user_token(wallet, mint);
        let ata_program = associated_token_program_id();
        let token_program = spl_token_program_id();
        let sys_prog = system_program_id();

        // Account order: payer, ata, wallet, mint, system_program, token_program
        let accounts = vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(sys_prog, false),
            AccountMeta::new_readonly(token_program, false),
        ];

        // 1 = CreateIdempotent instruction
        Instruction::new_with_bytes(ata_program, &[1], accounts)
    }

    /// Build Pump.fun Buy Instruction (18 accounts layout)
    pub fn build_buy_instruction(
        user: &Pubkey,
        mint: &Pubkey,
        creator: &Pubkey,
        amount: u64,
        max_sol_cost: u64,
    ) -> Instruction {
        let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
        let global = Pubkey::from_str(PUMPFUN_GLOBAL).unwrap();
        let fee_recipient = Pubkey::from_str(PUMPFUN_FEE_RECIPIENT).unwrap();
        let event_authority = Pubkey::from_str(PUMPFUN_EVENT_AUTHORITY).unwrap();
        let token_program = spl_token_program_id();
        let sys_prog = system_program_id();
        let protocol_fee_program = pump_fee_program_id();

        let (bonding_curve, _) = derive_bonding_curve(mint);
        let (associated_bonding_curve, _) = derive_associated_bonding_curve(&bonding_curve, mint);
        let (associated_user, _) = derive_associated_user_token(user, mint);
        let (creator_vault, _) = derive_creator_vault(creator);
        let (global_vol_acc, _) = derive_global_volume_accumulator();
        let (user_vol_acc, _) = derive_user_volume_accumulator(user);
        let (fee_config, _) = derive_fee_config();
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
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new(creator_vault, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(pump_program, false),
            AccountMeta::new_readonly(global_vol_acc, false),
            AccountMeta::new(user_vol_acc, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(protocol_fee_program, false),
            // Remaining accounts
            AccountMeta::new_readonly(bonding_curve_v2, false),
            AccountMeta::new(buyback_fee_recipient, false),
        ];

        Instruction::new_with_bytes(pump_program, &data, accounts)
    }

    /// Build Pump.fun Sell Instruction (16 accounts layout)
    pub fn build_sell_instruction(
        user: &Pubkey,
        mint: &Pubkey,
        creator: &Pubkey,
        amount: u64,
        min_sol_output: u64,
    ) -> Instruction {
        let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
        let global = Pubkey::from_str(PUMPFUN_GLOBAL).unwrap();
        let fee_recipient = Pubkey::from_str(PUMPFUN_FEE_RECIPIENT).unwrap();
        let event_authority = Pubkey::from_str(PUMPFUN_EVENT_AUTHORITY).unwrap();
        let token_program = spl_token_program_id();
        let sys_prog = system_program_id();
        let protocol_fee_program = pump_fee_program_id();

        let (bonding_curve, _) = derive_bonding_curve(mint);
        let (associated_bonding_curve, _) = derive_associated_bonding_curve(&bonding_curve, mint);
        let (associated_user, _) = derive_associated_user_token(user, mint);
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
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(pump_program, false),
            AccountMeta::new_readonly(fee_config, false),
            AccountMeta::new_readonly(protocol_fee_program, false),
            // Remaining accounts
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
}
