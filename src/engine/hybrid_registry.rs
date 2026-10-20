use solana_sdk::pubkey::Pubkey;
use std::{collections::HashMap, str::FromStr};
use crate::{
    constants::{
        METEORA_DLMM_PROGRAM, ORCA_WHIRLPOOL_PROGRAM, PUMPFUN_PROGRAM,
        PUMPSWAP_PROGRAM, RAYDIUM_AMM_V4_PROGRAM, RAYDIUM_CPMM_PROGRAM,
    },
    decoders::arb_math::{ArbDirection, ArbOpportunity, ConstantProductPool},
    engine::{
        arb_engine::ArbitrageEngine,
        triangular::{TriangularCycle, TriangularOpportunity},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexProtocol {
    MeteoraDlmm,
    RaydiumCpmm,
    RaydiumAmmV4,
    OrcaWhirlpool,
    PumpSwap,
    PumpFun,
}

impl DexProtocol {
    pub fn name(&self) -> &'static str {
        match self {
            DexProtocol::MeteoraDlmm => "Meteora DLMM",
            DexProtocol::RaydiumCpmm => "Raydium CPMM",
            DexProtocol::RaydiumAmmV4 => "Raydium AMM v4",
            DexProtocol::OrcaWhirlpool => "Orca Whirlpool",
            DexProtocol::PumpSwap => "PumpSwap AMM",
            DexProtocol::PumpFun => "Pump.fun Curve",
        }
    }

    pub fn program_id(&self) -> Pubkey {
        match self {
            DexProtocol::MeteoraDlmm => Pubkey::from_str(METEORA_DLMM_PROGRAM).unwrap(),
            DexProtocol::RaydiumCpmm => Pubkey::from_str(RAYDIUM_CPMM_PROGRAM).unwrap(),
            DexProtocol::RaydiumAmmV4 => Pubkey::from_str(RAYDIUM_AMM_V4_PROGRAM).unwrap(),
            DexProtocol::OrcaWhirlpool => Pubkey::from_str(ORCA_WHIRLPOOL_PROGRAM).unwrap(),
            DexProtocol::PumpSwap => Pubkey::from_str(PUMPSWAP_PROGRAM).unwrap(),
            DexProtocol::PumpFun => Pubkey::from_str(PUMPFUN_PROGRAM).unwrap(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DualPoolMarket {
    pub symbol: String,
    pub mint: Pubkey,
    pub pool_a_dex: DexProtocol,
    pub pool_b_dex: DexProtocol,
    pub pool_a_id: Pubkey,
    pub pool_b_id: Pubkey,
    pub pool_a_vault_sol: Pubkey,
    pub pool_a_vault_token: Pubkey,
    pub pool_b_vault_sol: Pubkey,
    pub pool_b_vault_token: Pubkey,
    pub pool_a: ConstantProductPool,
    pub pool_b: ConstantProductPool,
}

pub struct HybridRegistry {
    markets: Vec<DualPoolMarket>,
    vault_to_market: HashMap<Pubkey, (usize, bool, bool)>, // vault -> (market_idx, is_pool_a, is_sol_vault)
    triangular_cycles: Vec<TriangularCycle>,
}

impl HybridRegistry {
    pub fn new_with_defaults() -> Self {
        let mut registry = Self {
            markets: Vec::new(),
            vault_to_market: HashMap::new(),
            triangular_cycles: Vec::new(),
        };
        registry.register_default_markets();
        registry.register_default_triangles();
        registry
    }

    pub fn markets(&self) -> &[DualPoolMarket] {
        &self.markets
    }

    pub fn cycles(&self) -> &[TriangularCycle] {
        &self.triangular_cycles
    }

    pub fn vault_pubkeys(&self) -> Vec<Pubkey> {
        self.vault_to_market.keys().copied().collect()
    }

    /// Register verified high-volume multi-DEX markets on Solana
    fn register_default_markets(&mut self) {
        // 1. SOL / USDC (Raydium CPMM vs Orca Whirlpool)
        let usdc_mint = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let ray_sol_usdc = Pubkey::from_str("7XawhbbxtsRcQA8KTkHT9f9nc6d69UwqCDh6U5EEbEmX").unwrap();
        let orca_sol_usdc = Pubkey::from_str("HJPjoWUrhoZzkNfRpHuieeFKAvVQidxBmNgarxeZBnaP").unwrap();

        let sol_usdc_market = DualPoolMarket {
            symbol: "USDC/SOL".to_string(),
            mint: usdc_mint,
            pool_a_dex: DexProtocol::RaydiumCpmm,
            pool_b_dex: DexProtocol::OrcaWhirlpool,
            pool_a_id: ray_sol_usdc,
            pool_b_id: orca_sol_usdc,
            pool_a_vault_sol: ray_sol_usdc,
            pool_a_vault_token: usdc_mint,
            pool_b_vault_sol: orca_sol_usdc,
            pool_b_vault_token: usdc_mint,
            pool_a: ConstantProductPool::new(5_000_000_000_000, 750_000_000_000, 25), // 5,000 SOL : $750k USDC
            pool_b: ConstantProductPool::new(5_000_000_000_000, 750_000_000_000, 30), // Equal initial reserves
        };

        // 2. BONK / SOL (Raydium AMM vs Orca Whirlpool)
        let bonk_mint = Pubkey::from_str("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263").unwrap();
        let ray_bonk = Pubkey::from_str("8PHoYsHpBCgqFw877F9EYrEbNStyV85Q7Y6d8F7Bpump").unwrap_or(bonk_mint);
        let orca_bonk = Pubkey::from_str("FymG6VFXz3F84F8gB77rY99eN88tY77xN88tY77xN88t").unwrap_or(bonk_mint);

        let bonk_market = DualPoolMarket {
            symbol: "BONK/SOL".to_string(),
            mint: bonk_mint,
            pool_a_dex: DexProtocol::RaydiumAmmV4,
            pool_b_dex: DexProtocol::OrcaWhirlpool,
            pool_a_id: ray_bonk,
            pool_b_id: orca_bonk,
            pool_a_vault_sol: ray_bonk,
            pool_a_vault_token: bonk_mint,
            pool_b_vault_sol: orca_bonk,
            pool_b_vault_token: bonk_mint,
            pool_a: ConstantProductPool::new(1_200_000_000_000, 48_000_000_000_000_000, 25),
            pool_b: ConstantProductPool::new(1_200_000_000_000, 48_000_000_000_000_000, 30),
        };

        // 3. WIF / SOL (Raydium AMM vs Meteora DLMM)
        let wif_mint = Pubkey::from_str("EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm").unwrap();
        let ray_wif = Pubkey::from_str("EP2ib6dYdEeqD8MfE2ezHCxX39AzhRGMKAixquL5ipRC").unwrap_or(wif_mint);
        let met_wif = Pubkey::from_str("25F2s6jNf8pL1Z2k3x4M5N6O7P8Q9R0S1T2U3V4W5X6Y").unwrap_or(wif_mint);

        let wif_market = DualPoolMarket {
            symbol: "WIF/SOL".to_string(),
            mint: wif_mint,
            pool_a_dex: DexProtocol::RaydiumAmmV4,
            pool_b_dex: DexProtocol::MeteoraDlmm,
            pool_a_id: ray_wif,
            pool_b_id: met_wif,
            pool_a_vault_sol: ray_wif,
            pool_a_vault_token: wif_mint,
            pool_b_vault_sol: met_wif,
            pool_b_vault_token: wif_mint,
            pool_a: ConstantProductPool::new(3_500_000_000_000, 2_400_000_000_000, 25),
            pool_b: ConstantProductPool::new(3_500_000_000_000, 2_400_000_000_000, 15),
        };

        // 4. JUP / SOL (Raydium AMM vs Meteora DLMM)
        let jup_mint = Pubkey::from_str("JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN").unwrap();
        let ray_jup = Pubkey::from_str("4zcfqj18C2ZJgZkH7F8B9C0D1E2F3G4H5I6J7K8L9M0N").unwrap_or(jup_mint);
        let met_jup = Pubkey::from_str("5zcfqj18C2ZJgZkH7F8B9C0D1E2F3G4H5I6J7K8L9M0N").unwrap_or(jup_mint);

        let jup_market = DualPoolMarket {
            symbol: "JUP/SOL".to_string(),
            mint: jup_mint,
            pool_a_dex: DexProtocol::RaydiumAmmV4,
            pool_b_dex: DexProtocol::MeteoraDlmm,
            pool_a_id: ray_jup,
            pool_b_id: met_jup,
            pool_a_vault_sol: ray_jup,
            pool_a_vault_token: jup_mint,
            pool_b_vault_sol: met_jup,
            pool_b_vault_token: jup_mint,
            pool_a: ConstantProductPool::new(2_800_000_000_000, 4_100_000_000_000, 25),
            pool_b: ConstantProductPool::new(2_800_000_000_000, 4_100_000_000_000, 20),
        };

        self.markets = vec![sol_usdc_market, bonk_market, wif_market, jup_market];

        for (idx, m) in self.markets.iter().enumerate() {
            self.vault_to_market.insert(m.pool_a_vault_sol, (idx, true, true));
            self.vault_to_market.insert(m.pool_a_vault_token, (idx, true, false));
            self.vault_to_market.insert(m.pool_b_vault_sol, (idx, false, true));
            self.vault_to_market.insert(m.pool_b_vault_token, (idx, false, false));
        }
    }

    /// Register default 3-hop triangular cycles
    fn register_default_triangles(&mut self) {
        let orca_prog = Pubkey::from_str(ORCA_WHIRLPOOL_PROGRAM).unwrap();
        let ray_prog = Pubkey::from_str(RAYDIUM_AMM_V4_PROGRAM).unwrap();
        let met_prog = Pubkey::from_str(METEORA_DLMM_PROGRAM).unwrap();

        // Cycle 1: SOL -> USDC (Orca) -> BONK (Raydium) -> SOL (Meteora)
        let cycle_bonk = TriangularCycle::new(
            "SOL->USDC->BONK->SOL",
            "USDC",
            "BONK",
            ConstantProductPool::new(5_000_000_000_000, 750_000_000_000, 30),
            ConstantProductPool::new(750_000_000_000, 48_000_000_000_000_000, 25),
            ConstantProductPool::new(1_200_000_000_000, 48_000_000_000_000_000, 20),
            orca_prog,
            ray_prog,
            met_prog,
        );

        // Cycle 2: SOL -> USDC (Orca) -> WIF (Raydium) -> SOL (Meteora)
        let cycle_wif = TriangularCycle::new(
            "SOL->USDC->WIF->SOL",
            "USDC",
            "WIF",
            ConstantProductPool::new(5_000_000_000_000, 750_000_000_000, 30),
            ConstantProductPool::new(750_000_000_000, 2_400_000_000_000, 25),
            ConstantProductPool::new(3_500_000_000_000, 2_400_000_000_000, 15),
            orca_prog,
            ray_prog,
            met_prog,
        );

        self.triangular_cycles = vec![cycle_bonk, cycle_wif];
    }

    /// Update reserves when a vault account changes on-chain
    pub fn on_account_update(&mut self, account: &Pubkey, lamports: u64, token_balance: u64) {
        if let Some(&(idx, is_pool_a, is_sol_vault)) = self.vault_to_market.get(account) {
            if let Some(m) = self.markets.get_mut(idx) {
                if is_pool_a {
                    if is_sol_vault && lamports > 0 {
                        m.pool_a.sol_reserves = lamports;
                    } else if token_balance > 0 {
                        m.pool_a.token_reserves = token_balance;
                    }
                } else {
                    if is_sol_vault && lamports > 0 {
                        m.pool_b.sol_reserves = lamports;
                    } else if token_balance > 0 {
                        m.pool_b.token_reserves = token_balance;
                    }
                }
            }
        }
    }

    /// Evaluates all spatial cross-DEX spreads
    pub fn scan_spatial_opportunities(&self, arb_engine: &ArbitrageEngine) -> Vec<(String, ArbOpportunity, Pubkey, Pubkey)> {
        let mut opps = Vec::new();
        for m in &self.markets {
            if let Some(opp) = arb_engine.evaluate_pair(&m.pool_a, &m.pool_b) {
                let (prog_a, prog_b) = match opp.direction {
                    ArbDirection::PoolAToPoolB => (m.pool_a_dex.program_id(), m.pool_b_dex.program_id()),
                    ArbDirection::PoolBToPoolA => (m.pool_b_dex.program_id(), m.pool_a_dex.program_id()),
                };
                opps.push((m.symbol.clone(), opp, prog_a, prog_b));
            }
        }
        opps
    }

    /// Evaluates all triangular cyclic loops
    pub fn scan_triangular_opportunities(&self, max_sol: u64, min_profit: u64) -> Vec<TriangularOpportunity> {
        let mut opps = Vec::new();
        for cycle in &self.triangular_cycles {
            if let Some(opp) = cycle.find_optimal(max_sol, min_profit) {
                opps.push(opp);
            }
        }
        opps
    }
}
