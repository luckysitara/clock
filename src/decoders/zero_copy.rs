use solana_sdk::pubkey::Pubkey;

use crate::constants::{
    BONDING_CURVE_ACCOUNT_DISCRIMINATOR_U64, BUY_DISCRIMINATOR_U64, SELL_DISCRIMINATOR_ALT_U64,
    SELL_DISCRIMINATOR_U64,
};

/// 1-cycle 64-bit integer discriminator matching
#[inline(always)]
pub fn fast_is_discriminator(data: &[u8], target: u64) -> bool {
    if data.len() < 8 {
        return false;
    }
    let disc = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const u64) };
    disc == target
}

/// Zero-copy Plain-Old-Data (POD) struct for Pump.fun Buy instruction payload
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpFunBuyPod {
    pub amount: u64,
    pub max_sol_cost: u64,
}

/// Zero-copy Plain-Old-Data (POD) struct for Pump.fun Sell instruction payload
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpFunSellPod {
    pub amount: u64,
    pub min_sol_output: u64,
}

/// Zero-copy layout for Bonding Curve Account State
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondingCurveAccountPod {
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
    pub creator: Pubkey,
}

impl PumpFunBuyPod {
    /// Reads and validates Buy instruction parameters directly from raw byte slice in ~1 nanosecond.
    #[inline(always)]
    pub fn read_from_raw(data: &[u8]) -> Option<Self> {
        if data.len() < 24 {
            return None;
        }

        let disc = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const u64) };
        if disc != BUY_DISCRIMINATOR_U64 {
            return None;
        }

        let payload_ptr = unsafe { data.as_ptr().add(8) as *const Self };
        let pod = unsafe { std::ptr::read_unaligned(payload_ptr) };

        Some(Self {
            amount: u64::from_le(pod.amount),
            max_sol_cost: u64::from_le(pod.max_sol_cost),
        })
    }
}

impl PumpFunSellPod {
    /// Reads and validates Sell instruction parameters in ~1 nanosecond.
    #[inline(always)]
    pub fn read_from_raw(data: &[u8]) -> Option<Self> {
        if data.len() < 24 {
            return None;
        }

        let disc = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const u64) };
        if disc != SELL_DISCRIMINATOR_U64 && disc != SELL_DISCRIMINATOR_ALT_U64 {
            return None;
        }

        let payload_ptr = unsafe { data.as_ptr().add(8) as *const Self };
        let pod = unsafe { std::ptr::read_unaligned(payload_ptr) };

        Some(Self {
            amount: u64::from_le(pod.amount),
            min_sol_output: u64::from_le(pod.min_sol_output),
        })
    }
}

impl BondingCurveAccountPod {
    /// Zero-copy read of the Bonding Curve account data
    #[inline(always)]
    pub fn read_from_account(data: &[u8]) -> Option<Self> {
        // Must contain 8-byte discriminator + 41 bytes of state fields
        if data.len() < 49 {
            return None;
        }

        let disc = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const u64) };
        if disc != BONDING_CURVE_ACCOUNT_DISCRIMINATOR_U64 {
            return None;
        }

        let payload_ptr = unsafe { data.as_ptr().add(8) };

        unsafe {
            let virtual_token_reserves =
                u64::from_le(std::ptr::read_unaligned(payload_ptr as *const u64));
            let virtual_sol_reserves =
                u64::from_le(std::ptr::read_unaligned(payload_ptr.add(8) as *const u64));
            let real_token_reserves =
                u64::from_le(std::ptr::read_unaligned(payload_ptr.add(16) as *const u64));
            let real_sol_reserves =
                u64::from_le(std::ptr::read_unaligned(payload_ptr.add(24) as *const u64));
            let token_total_supply =
                u64::from_le(std::ptr::read_unaligned(payload_ptr.add(32) as *const u64));
            let complete = *payload_ptr.add(40) != 0;

            let creator = if data.len() >= 81 {
                let creator_bytes: [u8; 32] =
                    std::ptr::read_unaligned(payload_ptr.add(41) as *const [u8; 32]);
                Pubkey::new_from_array(creator_bytes)
            } else {
                Pubkey::default()
            };

            Some(Self {
                virtual_token_reserves,
                virtual_sol_reserves,
                real_token_reserves,
                real_sol_reserves,
                token_total_supply,
                complete,
                creator,
            })
        }
    }
}
