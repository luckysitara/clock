use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Verified Liquid Staking Tokens on Solana Mainnet
#[derive(Debug, Clone)]
pub struct LstMarket {
    pub symbol: &'static str,
    pub mint: Pubkey,
    pub pool_account: Pubkey,
    /// Estimated fair exchange rate in micro-SOL per LST (e.g. 1_127_000_000 = 1.127 SOL)
    pub fair_rate_lamports: u64,
}

pub struct LstDepegEngine {
    pub markets: Vec<LstMarket>,
    /// Minimum discount threshold in basis points (40 bps = 0.40%)
    pub min_discount_bps: u64,
}

#[derive(Debug, Clone)]
pub struct DepegOpportunity {
    pub symbol: &'static str,
    pub mint: Pubkey,
    pub pool: Pubkey,
    pub current_price_lamports: u64,
    pub fair_price_lamports: u64,
    pub discount_bps: u64,
    pub expected_profit_lamports: u64,
}

impl LstDepegEngine {
    pub fn new(min_discount_bps: u64) -> Self {
        let markets = vec![
            LstMarket {
                symbol: "JitoSOL",
                mint: Pubkey::from_str("J1toso1uCk3RKmjeoKuwWYvgwpq26bqnuJaS52cvEZ28").unwrap(),
                pool_account: Pubkey::from_str("Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P81AFstb").unwrap_or_default(),
                fair_rate_lamports: 1_127_000_000, // ~1.127 SOL per JitoSOL
            },
            LstMarket {
                symbol: "mSOL",
                mint: Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap(),
                pool_account: Pubkey::from_str("7GgPYvm1jr8nj8ViTStrRpprVnfkJF2Y4KqgSp4nTGfZ").unwrap_or_default(),
                fair_rate_lamports: 1_225_000_000, // ~1.225 SOL per mSOL
            },
            LstMarket {
                symbol: "INF",
                mint: Pubkey::from_str("5oVNBeEEQvYi1x3ir8rewM7CJa8eedgn2qRCeau3m3qq").unwrap(),
                pool_account: Pubkey::from_str("5oVNBeEEQvYi1x3ir8rewM7CJa8eedgn2qRCeau3m3qq").unwrap_or_default(),
                fair_rate_lamports: 1_280_000_000, // ~1.280 SOL per INF
            },
        ];

        Self {
            markets,
            min_discount_bps,
        }
    }

    /// Check if an LST price is dislocated below fair value
    pub fn evaluate_price(
        &self,
        mint: &Pubkey,
        market_price_lamports: u64,
        trade_size_lamports: u64,
    ) -> Option<DepegOpportunity> {
        for m in &self.markets {
            if m.mint == *mint {
                if market_price_lamports < m.fair_rate_lamports {
                    let discount = m.fair_rate_lamports - market_price_lamports;
                    let discount_bps = (discount as u128 * 10_000 / m.fair_rate_lamports as u128) as u64;

                    if discount_bps >= self.min_discount_bps {
                        let gross_profit = (trade_size_lamports as u128 * discount_bps as u128 / 10_000) as u64;
                        return Some(DepegOpportunity {
                            symbol: m.symbol,
                            mint: m.mint,
                            pool: m.pool_account,
                            current_price_lamports: market_price_lamports,
                            fair_price_lamports: m.fair_rate_lamports,
                            discount_bps,
                            expected_profit_lamports: gross_profit,
                        });
                    }
                }
            }
        }
        None
    }
}
