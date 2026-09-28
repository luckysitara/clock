#[derive(Debug, Clone)]
pub struct TipEngine {
    pub min_tip_lamports: u64,
    pub max_tip_lamports: u64,
    pub aggression_factor: f64,
}

impl TipEngine {
    pub fn new(min_tip_lamports: u64, max_tip_lamports: u64, aggression_factor: f64) -> Self {
        Self {
            min_tip_lamports,
            max_tip_lamports,
            aggression_factor,
        }
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
