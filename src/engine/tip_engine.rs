#[derive(Debug, Clone)]
pub struct TipEngine {
    pub min_tip_lamports: u64,
    pub max_tip_lamports: u64,
    pub aggression_factor: f64,
    /// Max tip as a fraction of trade value for entries and normal exits
    pub max_tip_pct_of_trade: f64,
    /// Max tip as a fraction of trade value for panic exits and sell retries
    pub max_panic_tip_pct_of_trade: f64,
}

/// Jito's minimum accepted tip
pub const JITO_MIN_TIP_LAMPORTS: u64 = 1_000;

impl TipEngine {
    pub fn new(min_tip_lamports: u64, max_tip_lamports: u64, aggression_factor: f64) -> Self {
        Self {
            min_tip_lamports,
            max_tip_lamports,
            aggression_factor,
            max_tip_pct_of_trade: 0.01,
            max_panic_tip_pct_of_trade: 0.03,
        }
    }

    /// Override the per-trade tip caps (fractions of trade value, e.g. 0.01 = 1%)
    pub fn with_trade_caps(mut self, normal_pct: f64, panic_pct: f64) -> Self {
        self.max_tip_pct_of_trade = normal_pct;
        self.max_panic_tip_pct_of_trade = panic_pct;
        self
    }

    /// Tip for an entry or normal exit: the default tip, capped at a share of the trade value
    pub fn trade_tip(&self, trade_value_lamports: u64) -> u64 {
        Self::cap(self.default_tip(), trade_value_lamports, self.max_tip_pct_of_trade)
    }

    /// Tip for a panic exit or retry: the max tip, capped at a (larger) share of the trade value
    pub fn panic_tip(&self, trade_value_lamports: u64) -> u64 {
        Self::cap(self.max_tip_lamports, trade_value_lamports, self.max_panic_tip_pct_of_trade)
    }

    fn cap(base: u64, trade_value_lamports: u64, pct: f64) -> u64 {
        let cap = (trade_value_lamports as f64 * pct) as u64;
        base.min(cap).max(JITO_MIN_TIP_LAMPORTS)
    }

    /// Calculate optimal Jito tip given expected gross profit in lamports
    pub fn calculate_optimal_tip(&self, gross_profit_lamports: u64) -> Option<u64> {
        // If profit does not cover the minimum viable tip, negative EV -> abort
        if gross_profit_lamports <= self.min_tip_lamports {
            return None;
        }

        let calculated_tip = (gross_profit_lamports as f64 * self.aggression_factor) as u64;

        let final_tip = calculated_tip
            .max(self.min_tip_lamports)
            .min(self.max_tip_lamports);

        Some(final_tip)
    }

    /// Default baseline tip for regular execution
    pub fn default_tip(&self) -> u64 {
        self.min_tip_lamports
    }
}
