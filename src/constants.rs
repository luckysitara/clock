use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// Program IDs
pub const PUMPFUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const PUMPFUN_FEE_RECIPIENT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
pub const PUMPFUN_GLOBAL: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
pub const PUMPFUN_EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
pub const PUMPSWAP_PROGRAM: &str = "BSfD6SHZigAfDWSQUAcsqqEGEHtNuAJaJWPGVNjek4c5";
pub const RAYDIUM_CPMM_PROGRAM: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

// 8-byte Raw Anchor Discriminators
pub const CREATE_DISCRIMINATOR_BYTES: [u8; 8] = [24, 30, 200, 40, 5, 28, 7, 119];
pub const BUY_DISCRIMINATOR_BYTES: [u8; 8] = [102, 6, 61, 18, 1, 218, 235, 234];
pub const SELL_DISCRIMINATOR_BYTES: [u8; 8] = [51, 230, 133, 164, 1, 127, 131, 173];
pub const SELL_DISCRIMINATOR_ALT_BYTES: [u8; 8] = [51, 230, 250, 225, 87, 251, 11, 54];
pub const BONDING_CURVE_ACCOUNT_DISCRIMINATOR_BYTES: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];

// Fast 64-bit Little-Endian Constants for single-cycle CPU register comparisons
pub const CREATE_DISCRIMINATOR_U64: u64 = u64::from_le_bytes(CREATE_DISCRIMINATOR_BYTES);
pub const BUY_DISCRIMINATOR_U64: u64 = u64::from_le_bytes(BUY_DISCRIMINATOR_BYTES);
pub const SELL_DISCRIMINATOR_U64: u64 = u64::from_le_bytes(SELL_DISCRIMINATOR_BYTES);
pub const SELL_DISCRIMINATOR_ALT_U64: u64 = u64::from_le_bytes(SELL_DISCRIMINATOR_ALT_BYTES);
pub const BONDING_CURVE_ACCOUNT_DISCRIMINATOR_U64: u64 =
    u64::from_le_bytes(BONDING_CURVE_ACCOUNT_DISCRIMINATOR_BYTES);

// Bonding Curve Defaults
pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
pub const INITIAL_VIRTUAL_TOKEN_RESERVES: u64 = 1_073_000_000_000_000;
pub const INITIAL_VIRTUAL_SOL_RESERVES: u64 = 30_000_000_000;
pub const INITIAL_REAL_TOKEN_RESERVES: u64 = 793_100_000_000_000;
pub const TOTAL_TOKEN_SUPPLY: u64 = 1_000_000_000_000_000;
pub const GRADUATION_REAL_SOL_LAMPORTS: u64 = 85_000_000_000; // ~85 SOL

// Jito MEV Tip Wallets
pub const JITO_TIP_WALLETS: [&str; 8] = [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

// Helper to derive pump.fun PDAs
pub fn derive_bonding_curve(mint: &Pubkey) -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"bonding-curve", mint.as_ref()], &pump_program)
}

pub fn derive_bonding_curve_v2(mint: &Pubkey) -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"bonding-curve-v2", mint.as_ref()], &pump_program)
}

pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

pub fn derive_associated_bonding_curve(bonding_curve: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> (Pubkey, u8) {
    let ata_program = associated_token_program_id();
    Pubkey::find_program_address(
        &[
            bonding_curve.as_ref(),
            token_program.as_ref(),
            mint.as_ref(),
        ],
        &ata_program,
    )
}

pub fn derive_associated_user_token(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> (Pubkey, u8) {
    let ata_program = associated_token_program_id();
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program,
    )
}

pub const PUMP_FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

pub const BUYBACK_FEE_RECIPIENTS: [&str; 8] = [
    "5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD",
    "9M4giFFMxmFGXtc3feFzRai56WbBqehoSeRE5GK7gf7",
    "GXPFM2caqTtQYC2cJ5yJRi9VDkpsYZXzYdwYpGnLmtDL",
    "3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR",
    "5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6",
    "EHAAiTxcdDwQ3U4bU6YcMsQGaekdzLS3B5SmYo46kJtL",
    "5eHhjP8JaYkz83CWwvGU2uMUXefd3AazWGx4gpcuEEYD",
    "A7hAgCzFw14fejgCp387JUJRMNyz4j89JKnhtKU8piqW",
];

pub fn get_random_buyback_fee_recipient() -> Pubkey {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let idx = COUNTER.fetch_add(1, Ordering::Relaxed) % BUYBACK_FEE_RECIPIENTS.len();
    Pubkey::from_str(BUYBACK_FEE_RECIPIENTS[idx]).unwrap()
}

pub fn derive_creator_vault(creator: &Pubkey) -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"creator-vault", creator.as_ref()], &pump_program)
}

pub fn derive_global_volume_accumulator() -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"global_volume_accumulator"], &pump_program)
}

pub fn derive_user_volume_accumulator(user: &Pubkey) -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"user_volume_accumulator", user.as_ref()], &pump_program)
}

pub fn derive_fee_config() -> (Pubkey, u8) {
    let pump_program = Pubkey::from_str(PUMPFUN_PROGRAM).unwrap();
    let fee_program = Pubkey::from_str(PUMP_FEE_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"fee_config", pump_program.as_ref()], &fee_program)
}

#[inline(always)]
pub fn pump_fee_program_id() -> Pubkey {
    Pubkey::from_str(PUMP_FEE_PROGRAM).unwrap()
}

#[inline(always)]
pub fn spl_token_program_id() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

#[inline(always)]
pub fn spl_token_2022_program_id() -> Pubkey {
    Pubkey::from_str(TOKEN_2022_PROGRAM).unwrap()
}

#[inline(always)]
pub fn associated_token_program_id() -> Pubkey {
    Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap()
}

#[inline(always)]
pub fn system_program_id() -> Pubkey {
    Pubkey::from_str("11111111111111111111111111111111").unwrap()
}

/// Zero-dependency native SOL transfer instruction (discriminator 2)
pub fn create_transfer_instruction(
    from: &Pubkey,
    to: &Pubkey,
    lamports: u64,
) -> solana_sdk::instruction::Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes()); // SystemInstruction::Transfer = 2
    data.extend_from_slice(&lamports.to_le_bytes());
    solana_sdk::instruction::Instruction::new_with_bytes(
        system_program_id(),
        &data,
        vec![
            solana_sdk::instruction::AccountMeta::new(*from, true),
            solana_sdk::instruction::AccountMeta::new(*to, false),
        ],
    )
}

