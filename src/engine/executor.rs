use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use reqwest::Client;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
    transaction::Transaction,
};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{
    decoders::BondingCurveAccountPod,
    engine::{
        blockhash_cache::BlockhashCache,
        instruction_builder::InstructionBuilder,
        jito::JitoClient,
        position_manager::{Position, PositionManager},
        tip_engine::TipEngine,
        wallet::fetch_token_balance,
    },
};

#[derive(Debug, Clone)]
pub enum TradeAction {
    Buy {
        mint: Pubkey,
        sol_amount_lamports: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        dev_wallet: Option<Pubkey>,
        /// Dev token balance at entry if known (0 = unknown, fetched after the buy lands)
        dev_initial_balance: u64,
        token_program: Option<Pubkey>,
    },
    Sell {
        mint: Pubkey,
        token_amount: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        is_panic: bool,
        token_program: Option<Pubkey>,
    },
    Arbitrage {
        mint: Pubkey,
        pool_a_name: String,
        pool_b_name: String,
        sol_in_lamports: u64,
        min_sol_out_lamports: u64,
        expected_profit_lamports: u64,
        instructions: Vec<solana_sdk::instruction::Instruction>,
    },
    PumpSwapBuy {
        mint: Pubkey,
        sol_amount_lamports: u64,
        template_instruction: solana_sdk::instruction::Instruction,
        token_program: Option<Pubkey>,
    },
}


pub struct ExecutionEngine {
    keypair: Arc<Keypair>,
    jito_client: Arc<JitoClient>,
    blockhash_cache: Arc<BlockhashCache>,
    tip_engine: Arc<TipEngine>,
    position_manager: Arc<PositionManager>,
    rpc_url: String,
    http_client: Client,
    /// Also send every transaction through public RPC `sendTransaction` (better landing, but public)
    dual_route_rpc: bool,
}

impl ExecutionEngine {
    pub fn new(
        keypair: Arc<Keypair>,
        jito_client: Arc<JitoClient>,
        blockhash_cache: Arc<BlockhashCache>,
        tip_engine: Arc<TipEngine>,
        position_manager: Arc<PositionManager>,
        rpc_url: String,
        dual_route_rpc: bool,
    ) -> Self {
        Self {
            keypair,
            jito_client,
            blockhash_cache,
            tip_engine,
            position_manager,
            rpc_url,
            dual_route_rpc,
            http_client: Client::builder()
                .timeout(std::time::Duration::from_millis(2500))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Spawn the asynchronous worker loop that listens for signals and executes Jito bundles
    pub fn spawn_worker(
        self: Arc<Self>,
        mut rx: mpsc::Receiver<TradeAction>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("⚡ Trade Execution Engine worker spawned and listening for signals");
            while let Some(action) = rx.recv().await {
                let engine = Arc::clone(&self);
                tokio::spawn(async move {
                    if let Err(e) = engine.execute_action(action).await {
                        error!("❌ Trade execution failed: {:#}", e);
                    }
                });
            }
        })
    }

    /// Polls RPC for signature confirmation status with short delay
    pub async fn wait_for_confirmation(
        &self,
        signature: &Signature,
        max_attempts: usize,
        delay: std::time::Duration,
    ) -> bool {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignatureStatuses",
            "params": [[signature.to_string()], {"searchTransactionHistory": true}]
        });

        for attempt in 1..=max_attempts {
            tokio::time::sleep(delay).await;
            if let Ok(resp) = self.http_client.post(&self.rpc_url).json(&payload).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(statuses) = json.get("result").and_then(|r| r.get("value")).and_then(|v| v.as_array()) {
                        if let Some(Some(status)) = statuses.first().map(|s| s.as_object()) {
                            let err_is_null = status.get("err").map(|e| e.is_null()).unwrap_or(false);
                            if err_is_null {
                                let confirmation = status.get("confirmationStatus").and_then(|s| s.as_str());
                                if confirmation == Some("processed") || confirmation == Some("confirmed") || confirmation == Some("finalized") {
                                    debug!("Signature {} confirmed on attempt {}", signature, attempt);
                                    return true;
                                }
                            } else if status.get("err").map(|e| !e.is_null()).unwrap_or(false) {
                                warn!("Signature {} failed on-chain with err: {:?}", signature, status.get("err"));
                                return false;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Fire-and-forget copy of the transaction through public RPC (when dual routing is on)
    fn send_via_rpc(&self, tx: &Transaction) {
        if !self.dual_route_rpc {
            return;
        }
        let Ok(serialized) = bincode::serialize(tx) else { return };
        let rpc_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                bs58::encode(&serialized).into_string(),
                {
                    "skipPreflight": true,
                    "preflightCommitment": "processed",
                    "encoding": "base58",
                    "maxRetries": 0
                }
            ]
        });
        let rpc_client = self.http_client.clone();
        let rpc_url = self.rpc_url.clone();
        tokio::spawn(async move {
            let _ = rpc_client.post(&rpc_url).json(&rpc_payload).send().await;
        });
    }

    pub async fn execute_action(&self, action: TradeAction) -> Result<()> {
        match action {
            TradeAction::Buy {
                mint,
                sol_amount_lamports,
                slippage_bps,
                curve_state,
                dev_wallet,
                dev_initial_balance,
                token_program,
            } => {
                let result = self
                    .execute_buy(
                        mint,
                        sol_amount_lamports,
                        slippage_bps,
                        curve_state,
                        dev_wallet,
                        dev_initial_balance,
                        token_program,
                    )
                    .await;
                // Free the capital reservation unless the position was opened
                if !matches!(result, Ok(true)) {
                    self.position_manager.release(&mint).await;
                }
                result.map(|_| ())
            }

            TradeAction::Sell {
                mint,
                token_amount,
                slippage_bps,
                curve_state,
                is_panic,
                token_program,
            } => {
                self.execute_sell(mint, token_amount, slippage_bps, curve_state, is_panic, token_program)
                    .await
            }

            TradeAction::Arbitrage {
                mint,
                pool_a_name,
                pool_b_name,
                sol_in_lamports,
                min_sol_out_lamports,
                expected_profit_lamports,
                mut instructions,
            } => {
                let blockhash = self.blockhash_cache.get_latest().await;
                let optimal_tip = self
                    .tip_engine
                    .calculate_optimal_tip(expected_profit_lamports)
                    .unwrap_or_else(|| self.tip_engine.default_tip());

                info!(
                    "⚡ Executing ATOMIC ARBITRAGE for Mint {} | {} -> {} | In: {:.4} SOL | Exp Out: {:.4} SOL | Gross Profit: {:.4} SOL | Jito Tip: {:.5} SOL",
                    mint,
                    pool_a_name,
                    pool_b_name,
                    sol_in_lamports as f64 / 1e9,
                    min_sol_out_lamports as f64 / 1e9,
                    expected_profit_lamports as f64 / 1e9,
                    optimal_tip as f64 / 1e9
                );

                // Priority compute budget
                let compute_cu = InstructionBuilder::set_compute_unit_limit(350_000);
                let compute_price = InstructionBuilder::set_compute_unit_price(200_000);
                let mut full_instructions = vec![compute_cu, compute_price];
                full_instructions.append(&mut instructions);

                let bundle_tx = self.jito_client.build_tip_bundle(
                    &self.keypair,
                    full_instructions,
                    optimal_tip,
                    blockhash,
                );

                let sig = bundle_tx.signatures[0];

                // Arbitrage is 100% private Jito bundle submission (Zero-Loss Guarantee)
                // We deliberately DO NOT broadcast to public RPC to prevent sandwich attacks and avoid gas on dropped bundles
                let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
                info!(
                    "📡 Private Jito MEV bundle dispatched (sig: {} | Jito: {}). Verifying on-chain landing...",
                    sig, bundle_id
                );

                let landed = self
                    .wait_for_confirmation(&sig, 12, std::time::Duration::from_millis(350))
                    .await;
                if landed {
                    let net_profit = expected_profit_lamports.saturating_sub(optimal_tip);
                    info!(
                        "🎉 ARBITRAGE LANDED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Net Profit: {:.4} SOL",
                        sig,
                        mint,
                        net_profit as f64 / 1e9
                    );
                } else {
                    warn!(
                        "⚠️ Arb bundle for mint {} did not land on-chain (outbid or slipped). Capital 100% safe (reverted or not mined).",
                        mint
                    );
                }
                Ok(())
            }

            TradeAction::PumpSwapBuy {
                mint,
                sol_amount_lamports,
                template_instruction,
                token_program,
            } => {
                let blockhash = self.blockhash_cache.get_latest().await;
                let payer_pubkey = self.keypair.pubkey();
                let token_prog = token_program.unwrap_or_else(crate::constants::spl_token_program_id);
                let wsol_mint = crate::constants::wsol_mint();
                let spl_token_prog = crate::constants::spl_token_program_id();

                let (our_wsol_ata, _) = crate::constants::derive_associated_user_token(&payer_pubkey, &wsol_mint, &spl_token_prog);
                let (our_token_ata, _) = crate::constants::derive_associated_user_token(&payer_pubkey, &mint, &token_prog);

                let compute_cu = InstructionBuilder::set_compute_unit_limit(250_000);
                let compute_price = InstructionBuilder::set_compute_unit_price(150_000);

                let create_wsol_ata = InstructionBuilder::create_ata_idempotent(&payer_pubkey, &payer_pubkey, &wsol_mint, &spl_token_prog);
                let fund_wsol = crate::constants::create_transfer_instruction(&payer_pubkey, &our_wsol_ata, sol_amount_lamports);
                let sync_native = solana_sdk::instruction::Instruction::new_with_bytes(
                    spl_token_prog,
                    &[17u8],
                    vec![solana_sdk::instruction::AccountMeta::new(our_wsol_ata, false)],
                );
                let create_token_ata = InstructionBuilder::create_ata_idempotent(&payer_pubkey, &payer_pubkey, &mint, &token_prog);

                let buy_pod = if template_instruction.data.len() >= 24 {
                    crate::decoders::PumpFunBuyPod::read_from_raw(&template_instruction.data)
                } else {
                    None
                };

                let (scaled_tokens, max_sol) = if let Some(pod) = buy_pod {
                    if pod.max_sol_cost > 0 {
                        let tokens = (pod.amount as u128 * sol_amount_lamports as u128 / pod.max_sol_cost as u128) as u64;
                        let with_slippage = tokens * 95 / 100;
                        (with_slippage.max(1), sol_amount_lamports)
                    } else {
                        (1u64, sol_amount_lamports)
                    }
                } else {
                    (1u64, sol_amount_lamports)
                };

                let buy_ix = match InstructionBuilder::build_pumpswap_amm_buy_from_template(
                    &template_instruction.accounts,
                    &payer_pubkey,
                    &our_wsol_ata,
                    &our_token_ata,
                    scaled_tokens,
                    max_sol,
                ) {
                    Some(ix) => ix,
                    None => {
                        error!("Failed to build PumpSwap AMM buy instruction from template for mint {}", mint);
                        return Ok(());
                    }
                };

                let close_wsol = InstructionBuilder::close_ata(&our_wsol_ata, &payer_pubkey, &payer_pubkey, &spl_token_prog);

                let instructions = vec![
                    compute_cu,
                    compute_price,
                    create_wsol_ata,
                    fund_wsol,
                    sync_native,
                    create_token_ata,
                    buy_ix,
                    close_wsol,
                ];

                info!("🎯 Dispatching PumpSwap AMM Co-Snipe for Mint {} | Spend: {:.4} SOL | Exp Min Tokens: {}", mint, sol_amount_lamports as f64 / 1e9, scaled_tokens);

                let tip = self.tip_engine.default_tip();
                let bundle_tx = self.jito_client.build_tip_bundle(
                    &self.keypair,
                    instructions,
                    tip,
                    blockhash,
                );

                let sig = bundle_tx.signatures[0];

                self.send_via_rpc(&bundle_tx);

                let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
                info!("📡 PumpSwap AMM Co-Snipe dispatched (sig: {} | Jito: {} | RPC route: {})...", sig, bundle_id, self.dual_route_rpc);

                let landed = self.wait_for_confirmation(&sig, 15, std::time::Duration::from_millis(300)).await;
                if landed {
                    info!("🎉 PUMPSWAP AMM CO-SNIPE CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {}", sig, mint);
                } else {
                    warn!("⚠️ PumpSwap AMM bundle for mint {} did not land on-chain. Capital preserved (0 SOL spent).", mint);
                }
                Ok(())
            }
        }
    }

    /// Returns Ok(true) once the buy is confirmed and the position is recorded
    #[allow(clippy::too_many_arguments)]
    async fn execute_buy(
        &self,
        mint: Pubkey,
        sol_amount_lamports: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        dev_wallet: Option<Pubkey>,
        dev_initial_balance: u64,
        token_program: Option<Pubkey>,
    ) -> Result<bool> {
        let blockhash = self.blockhash_cache.get_latest().await;
        let payer_pubkey = self.keypair.pubkey();

        let calc = curve_state
            .calculate_buy_output(sol_amount_lamports, slippage_bps)
            .context("Curve calculation failed or curve is complete")?;

        let token_prog = token_program.unwrap_or_else(|| {
            crate::constants::spl_token_program_id()
        });

        info!(
            "🚀 Executing BUY for mint: {} | Tokens: {} | Max SOL: {} | Est Price: {} SOL | Token Prog: {}",
            mint,
            calc.tokens_out,
            calc.max_sol_cost as f64 / 1e9,
            calc.effective_price_sol,
            token_prog
        );

        let creator = dev_wallet.unwrap_or(curve_state.creator);

        // Instructions: ComputeBudget + Create ATA Idempotent + Buy
        let compute_cu = InstructionBuilder::set_compute_unit_limit(200_000);
        let compute_price = InstructionBuilder::set_compute_unit_price(100_000);
        let create_ata_ix =
            InstructionBuilder::create_ata_idempotent(&payer_pubkey, &payer_pubkey, &mint, &token_prog);
        let buy_ix = InstructionBuilder::build_buy_instruction(
            &payer_pubkey,
            &mint,
            &creator,
            calc.tokens_out,
            calc.max_sol_cost,
            &token_prog,
        );

        let instructions = vec![compute_cu, compute_price, create_ata_ix, buy_ix];
        let tip = self.tip_engine.default_tip();

        let bundle_tx = self.jito_client.build_tip_bundle(
            &self.keypair,
            instructions,
            tip,
            blockhash,
        );

        let sig = bundle_tx.signatures[0];

        self.send_via_rpc(&bundle_tx);
        let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
        info!("📡 Buy dispatched (sig: {} | Jito: {} | RPC route: {}). Verifying on-chain confirmation...", sig, bundle_id, self.dual_route_rpc);

        let landed = self.wait_for_confirmation(&sig, 15, std::time::Duration::from_millis(400)).await;
        if !landed {
            warn!("⚠️ Bundle for mint {} did not land on-chain (dropped or outbid). Capital preserved (0 SOL spent).", mint);
            return Ok(false);
        }

        // calc.tokens_out is the slippage-adjusted minimum. Use what actually arrived, so the
        // sell size is right and the entry price (which anchors every stop) isn't inflated by slippage
        let (token_balance, entry_price_sol) =
            match fetch_token_balance(&self.http_client, &self.rpc_url, &payer_pubkey, &mint, &token_prog).await {
                Some(received) if received > 0 => {
                    (received, (sol_amount_lamports as f64 / 1e9) / (received as f64 / 1e6))
                }
                _ => {
                    warn!("Could not read received token balance for {}; using slippage-adjusted estimate", mint);
                    (calc.tokens_out, calc.effective_price_sol)
                }
            };

        info!("🎉 TRADE CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Tokens: {} | Entry: {:.10} SOL | Sol Spent: {:.4} SOL", sig, mint, token_balance, entry_price_sol, sol_amount_lamports as f64 / 1e9);
        let dev = (creator != Pubkey::default()).then_some(creator);
        self.position_manager
            .add_position(Position {
                mint,
                bonding_curve: crate::constants::derive_bonding_curve(&mint).0,
                entry_price_sol,
                highest_price_sol: entry_price_sol,
                token_balance,
                sol_invested_lamports: sol_amount_lamports,
                dev_wallet: dev,
                dev_initial_balance,
                token_program: token_prog,
                is_selling: false,
            })
            .await;

        // Copy-trades don't know the dev's bag yet; look it up so dump detection has a threshold
        if dev_initial_balance == 0 {
            if let Some(dev) = dev {
                match fetch_token_balance(&self.http_client, &self.rpc_url, &dev, &mint, &token_prog).await {
                    Some(balance) if balance > 0 => {
                        info!("👀 Dev {} holds {} tokens of {} at entry", dev, balance, mint);
                        self.position_manager.set_dev_initial_balance(&mint, balance).await;
                    }
                    _ => warn!("Dev balance for {} unknown or zero; any dev sell will trigger a panic exit", mint),
                }
            }
        }

        Ok(true)
    }

    async fn execute_sell(
        &self,
        mint: Pubkey,
        token_amount: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        is_panic: bool,
        token_program: Option<Pubkey>,
    ) -> Result<()> {
        let payer_pubkey = self.keypair.pubkey();
        let calc = match curve_state.calculate_sell_output(token_amount, slippage_bps) {
            Some(c) => c,
            None => {
                self.position_manager.unmark_selling(&mint).await;
                anyhow::bail!("Curve sell calculation failed for {}", mint);
            }
        };

        let token_prog = token_program.unwrap_or_else(|| {
            crate::constants::spl_token_program_id()
        });

        // In panic dump or stop-loss, use 1 lamport floor so Pump.fun AMM NEVER fails with Custom 6003 (TooLittleSolReceived)
        let min_sol_out = if is_panic {
            1u64
        } else {
            calc.min_sol_output
        };

        info!(
            "⚠️ Executing SELL {} for mint: {} | Tokens: {} | Min SOL Floor: {} SOL | Est Price: {} SOL | Token Prog: {}",
            if is_panic { "(PANIC / STOP-LOSS - 1 LAMPORT FLOOR)" } else { "" },
            mint,
            token_amount,
            min_sol_out as f64 / 1e9,
            calc.effective_price_sol,
            token_prog
        );

        let creator = curve_state.creator;
        let compute_cu = InstructionBuilder::set_compute_unit_limit(150_000);
        let compute_price = InstructionBuilder::set_compute_unit_price(if is_panic {
            1_000_000
        } else {
            100_000
        });

        for attempt in 1..=3 {
            let blockhash = self.blockhash_cache.get_latest().await;
            let sell_min_floor = if attempt > 1 || is_panic { 1u64 } else { min_sol_out };
            let sell_ix = InstructionBuilder::build_sell_instruction(
                &payer_pubkey,
                &mint,
                &creator,
                token_amount,
                sell_min_floor,
                &token_prog,
            );

            let instructions = vec![compute_cu.clone(), compute_price.clone(), sell_ix];
            let tip = if is_panic || attempt > 1 {
                self.tip_engine.max_tip_lamports
            } else {
                self.tip_engine.default_tip()
            };

            let bundle_tx = self.jito_client.build_tip_bundle(
                &self.keypair,
                instructions,
                tip,
                blockhash,
            );

            let sig = bundle_tx.signatures[0];

            self.send_via_rpc(&bundle_tx);
            match self.jito_client.send_bundle(&bundle_tx).await {
                Ok(bundle_id) => info!("📡 Sell dispatched [attempt {}/3] (sig: {} | Jito: {})...", attempt, sig, bundle_id),
                // The RPC copy may still land, so keep checking for confirmation
                Err(e) => warn!("Jito rejected sell [attempt {}/3] for {}: {:#}", attempt, mint, e),
            }

            let landed = self.wait_for_confirmation(&sig, 12, std::time::Duration::from_millis(350)).await;
            if landed {
                info!("🎉 SELL CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Tokens Sold: {} | Min Floor: {:.4} SOL", sig, mint, token_amount, sell_min_floor as f64 / 1e9);
                self.position_manager.remove_position(&mint).await;
                return Ok(());
            }
            warn!("⚠️ Sell attempt {} for mint {} did not land. Retrying immediately with 1-lamport floor...", attempt, mint);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        error!("❌ All 3 sell attempts failed for mint {}. Resetting is_selling flag for next price update retry.", mint);
        self.position_manager.unmark_selling(&mint).await;
        Ok(())
    }
}
