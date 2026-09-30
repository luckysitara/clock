use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use reqwest::Client;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    signer::Signer,
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
}

pub struct ExecutionEngine {
    keypair: Arc<Keypair>,
    jito_client: Arc<JitoClient>,
    blockhash_cache: Arc<BlockhashCache>,
    tip_engine: Arc<TipEngine>,
    position_manager: Arc<PositionManager>,
    rpc_url: String,
    http_client: Client,
}

impl ExecutionEngine {
    pub fn new(
        keypair: Arc<Keypair>,
        jito_client: Arc<JitoClient>,
        blockhash_cache: Arc<BlockhashCache>,
        tip_engine: Arc<TipEngine>,
        position_manager: Arc<PositionManager>,
        rpc_url: String,
    ) -> Self {
        Self {
            keypair,
            jito_client,
            blockhash_cache,
            tip_engine,
            position_manager,
            rpc_url,
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

    pub async fn execute_action(&self, action: TradeAction) -> Result<()> {
        let blockhash = self.blockhash_cache.get_latest().await;
        let payer_pubkey = self.keypair.pubkey();

        match action {
            TradeAction::Buy {
                mint,
                sol_amount_lamports,
                slippage_bps,
                curve_state,
                dev_wallet,
                token_program,
            } => {
                let calc = curve_state
                    .calculate_buy_output(sol_amount_lamports, slippage_bps)
                    .context("Curve calculation failed or curve is complete")?;

                let token_prog = token_program.unwrap_or_else(|| {
                    crate::constants::spl_token_2022_program_id()
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

                // Dual-Routing: Dispatch simultaneously to Triton RPC sendTransaction + Jito Block Engines
                if let Ok(serialized) = bincode::serialize(&bundle_tx) {
                    let base58_tx = bs58::encode(&serialized).into_string();
                    let rpc_payload = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "sendTransaction",
                        "params": [
                            base58_tx,
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

                let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
                info!("📡 Dual-routed transaction dispatched (sig: {} | Jito: {}). Verifying on-chain confirmation...", sig, bundle_id);

                let landed = self.wait_for_confirmation(&sig, 15, std::time::Duration::from_millis(400)).await;
                if landed {
                    info!("🎉 TRADE CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Tokens: {} | Sol Spent: {:.4} SOL", sig, mint, calc.tokens_out, sol_amount_lamports as f64 / 1e9);
                    self.position_manager
                        .add_position(Position {
                            mint,
                            entry_price_sol: calc.effective_price_sol,
                            highest_price_sol: calc.effective_price_sol,
                            token_balance: calc.tokens_out,
                            sol_invested_lamports: sol_amount_lamports,
                            dev_wallet,
                            dev_initial_balance: 0,
                            token_program: token_prog,
                            is_selling: false,
                        })
                        .await;
                } else {
                    warn!("⚠️ Bundle for mint {} did not land on-chain (dropped or outbid). Capital preserved (0 SOL spent).", mint);
                }
            }

            TradeAction::Sell {
                mint,
                token_amount,
                slippage_bps,
                curve_state,
                is_panic,
                token_program,
            } => {
                let calc = curve_state
                    .calculate_sell_output(token_amount, slippage_bps)
                    .context("Curve sell calculation failed")?;

                let token_prog = token_program.unwrap_or_else(|| {
                    crate::constants::spl_token_2022_program_id()
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

                let mut confirmed = false;
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

                    // Dual-Routing for exits: Dispatch simultaneously to Triton RPC + Jito
                    if let Ok(serialized) = bincode::serialize(&bundle_tx) {
                        let base58_tx = bs58::encode(&serialized).into_string();
                        let rpc_payload = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "sendTransaction",
                            "params": [
                                base58_tx,
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

                    let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
                    info!("📡 Dual-routed sell dispatched [attempt {}/3] (sig: {} | Jito: {})...", attempt, sig, bundle_id);

                    let landed = self.wait_for_confirmation(&sig, 12, std::time::Duration::from_millis(350)).await;
                    if landed {
                        info!("🎉 SELL CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Tokens Sold: {} | Min Floor: {:.4} SOL", sig, mint, token_amount, sell_min_floor as f64 / 1e9);
                        self.position_manager.remove_position(&mint).await;
                        confirmed = true;
                        break;
                    } else {
                        warn!("⚠️ Sell attempt {} for mint {} did not land. Retrying immediately with 1-lamport floor...", attempt, mint);
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }

                if !confirmed {
                    error!("❌ All 3 sell attempts failed for mint {}. Resetting is_selling flag for next price update retry.", mint);
                    self.position_manager.unmark_selling(&mint).await;
                }
            }
        }

        Ok(())
    }
}
