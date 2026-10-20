use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use std::str::FromStr;

/// Solend / Save Mainnet Program ID
pub const SOLEND_PROGRAM_ID: &str = "So1endDq2YkqhipRh3WViPa8hdiSpxWy6z3Z6tMCpAo";
pub const SOLEND_MAIN_MARKET: &str = "4UpD2fhY3WFACtuHd1gmvt3rvwvnmMm2gqUJeNiLsQGm";
pub const SOLEND_MARKET_AUTHORITY: &str = "GdQ144U8jgb9BsUeE8xT7Rdf1g4p6c4x1g9LqGv4u15t";
pub const SOLEND_SOL_RESERVE: &str = "8PbodeaosQP19SjYFx855UMqWx2wKZiTdmFCzF243qEP";
pub const SOLEND_SOL_VAULT: &str = "8UviNr4n1DnWTsBo3nmKm29xcAZYWcucisPXAbnmBLTr";

#[derive(Debug, Clone)]
pub struct FlashLoanAccounts {
    pub program_id: Pubkey,
    pub reserve: Pubkey,
    pub reserve_vault: Pubkey,
    pub market: Pubkey,
    pub market_authority: Pubkey,
}

impl Default for FlashLoanAccounts {
    fn default() -> Self {
        Self {
            program_id: Pubkey::from_str(SOLEND_PROGRAM_ID).unwrap_or_default(),
            reserve: Pubkey::from_str(SOLEND_SOL_RESERVE).unwrap_or_default(),
            reserve_vault: Pubkey::from_str(SOLEND_SOL_VAULT).unwrap_or_default(),
            market: Pubkey::from_str(SOLEND_MAIN_MARKET).unwrap_or_default(),
            market_authority: Pubkey::from_str(SOLEND_MARKET_AUTHORITY).unwrap_or_default(),
        }
    }
}

pub struct FlashLoanManager;

impl FlashLoanManager {
    /// Builds FlashBorrow instruction (Discriminator 14)
    /// Borrows `amount_lamports` of WSOL into `user_wsol_account`
    pub fn build_flash_borrow(
        accounts: &FlashLoanAccounts,
        user_wsol_account: &Pubkey,
        amount_lamports: u64,
    ) -> Instruction {
        let sysvar = solana_sdk::sysvar::instructions::id();
        let token_program = crate::constants::spl_token_program_id();

        let mut data = Vec::with_capacity(9);
        data.push(14u8); // FlashBorrow discriminator
        data.extend_from_slice(&amount_lamports.to_le_bytes());

        let account_metas = vec![
            AccountMeta::new(accounts.reserve_vault, false),
            AccountMeta::new(*user_wsol_account, false),
            AccountMeta::new(accounts.reserve, false),
            AccountMeta::new_readonly(accounts.market, false),
            AccountMeta::new_readonly(accounts.market_authority, false),
            AccountMeta::new_readonly(sysvar, false),
            AccountMeta::new_readonly(token_program, false),
        ];

        Instruction::new_with_bytes(accounts.program_id, &data, account_metas)
    }

    /// Builds FlashRepay instruction (Discriminator 15)
    /// Repays borrowed `amount_lamports` from `user_wsol_account` back to reserve
    pub fn build_flash_repay(
        accounts: &FlashLoanAccounts,
        user_wsol_account: &Pubkey,
        amount_lamports: u64,
        borrow_instruction_index: u8,
    ) -> Instruction {
        let sysvar = solana_sdk::sysvar::instructions::id();
        let token_program = crate::constants::spl_token_program_id();

        let mut data = Vec::with_capacity(10);
        data.push(15u8); // FlashRepay discriminator
        data.extend_from_slice(&amount_lamports.to_le_bytes());
        data.push(borrow_instruction_index);

        let account_metas = vec![
            AccountMeta::new(*user_wsol_account, false),
            AccountMeta::new(accounts.reserve_vault, false),
            AccountMeta::new(accounts.reserve, false),
            AccountMeta::new_readonly(accounts.market, false),
            AccountMeta::new_readonly(accounts.market_authority, false),
            AccountMeta::new_readonly(sysvar, false),
            AccountMeta::new_readonly(token_program, false),
        ];

        Instruction::new_with_bytes(accounts.program_id, &data, account_metas)
    }

    /// Calculate flash borrow fee (1 bps = 0.01%)
    pub fn calculate_fee(amount_lamports: u64) -> u64 {
        (amount_lamports as u128 * 1 / 10_000) as u64
    }
}
