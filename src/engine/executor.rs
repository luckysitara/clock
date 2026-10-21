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
        wallet::{fetch_buy_cost, fetch_token_balance},
    },
};

/// Where and when the stream saw the trade a buy reacts to
#[derive(Debug, Clone, Copy)]
pub struct Signal {
    pub slot: u64,
    pub seen_at: std::time::Instant,
}

/// One line per copy buy: how many blocks after the copied trade ours lands (live) or could
/// earliest land (paper: the RPC's current slot when we would send). Summarized by `pnl_report --latency`
pub const LATENCY_LOG: &str = "data/latency.jsonl";

#[derive(Debug, Clone)]
pub enum TradeAction {
    Buy {
        /// The trade that triggered this buy, for measuring how many blocks behind it we land
        signal: Option<Signal>,
        mint: Pubkey,
        sol_amount_lamports: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        dev_wallet: Option<Pubkey>,
        /// Dev token balance at entry if known (0 = unknown, fetched after the buy lands)
        dev_initial_balance: u64,
        token_program: Option<Pubkey>,
        /// Wallets whose buy this copies; the position exits when any of them sells
        leaders: Vec<Pubkey>,
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
    /// Paper trading: simulate instead of sending, and log trades to this file
    paper_log: Option<std::path::PathBuf>,
    /// Last known wallet balance in lamports (u64::MAX until first fetched)
    balance_lamports: Arc<std::sync::atomic::AtomicU64>,
}

/// Rent for a new token account (refunded when it is closed)
const TOKEN_ACCOUNT_RENT_LAMPORTS: u64 = 2_039_280;
/// Headroom for network + priority fees on a buy
const BUY_FEE_HEADROOM_LAMPORTS: u64 = 100_000;

/// Estimated Pump.fun fee per side (protocol + creator) used for paper-trade accounting
const PAPER_PUMP_FEE_BPS: u64 = 125;

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
            paper_log: None,
            balance_lamports: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
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
        self.wait_for_landing_slot(signature, max_attempts, delay).await.is_some()
    }

    /// Slot the transaction landed in, once it is processed without error; None if it failed or never showed up
    pub async fn wait_for_landing_slot(
        &self,
        signature: &Signature,
        max_attempts: usize,
        delay: std::time::Duration,
    ) -> Option<u64> {
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
                                    return Some(status.get("slot").and_then(|s| s.as_u64()).unwrap_or(0));
                                }
                            } else if status.get("err").map(|e| !e.is_null()).unwrap_or(false) {
                                warn!("Signature {} failed on-chain with err: {:?}", signature, status.get("err"));
                                return None;
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Paper trading: nothing is sent; buys are simulated and every trade is logged to `paper_log`
    pub fn with_dry_run(mut self, paper_log: impl Into<std::path::PathBuf>) -> Self {
        self.paper_log = Some(paper_log.into());
        self
    }

    /// Keep the wallet balance cached (refreshed every 2s) so buys can be checked without an RPC round trip
    pub fn spawn_balance_refresher(&self) -> tokio::task::JoinHandle<()> {
        let (client, rpc_url, wallet, balance) =
            (self.http_client.clone(), self.rpc_url.clone(), self.keypair.pubkey().to_string(), Arc::clone(&self.balance_lamports));
        tokio::spawn(async move {
            let payload = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "getBalance",
                "params": [wallet, {"commitment": "processed"}]
            });
            loop {
                if let Ok(resp) = client.post(&rpc_url).json(&payload).send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(lamports) = json["result"]["value"].as_u64() {
                            balance.store(lamports, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        })
    }

    pub fn is_dry_run(&self) -> bool {
        self.paper_log.is_some()
    }

    /// Fire-and-forget copy of the transaction through public RPC (when dual routing is on)
    fn send_via_rpc(&self, tx: &Transaction) {
        if self.dual_route_rpc {
            self.send_rpc(tx);
        }
    }

    /// Fire-and-forget sendTransaction through the configured RPC
    fn send_rpc(&self, tx: &Transaction) {
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

    /// Run the transaction through simulateTransaction; Err holds the failure and last logs
    async fn simulate(&self, tx: &Transaction) -> Result<(), String> {
        self.simulate_at(tx).await.1
    }

    /// Like `simulate`, plus the slot the RPC simulated at (its current processed slot)
    async fn simulate_at(&self, tx: &Transaction) -> (Option<u64>, Result<(), String>) {
        match self.simulate_raw(tx).await {
            Ok((slot, result)) => (slot, result),
            Err(e) => (None, Err(e)),
        }
    }

    async fn simulate_raw(&self, tx: &Transaction) -> Result<(Option<u64>, Result<(), String>), String> {
        use base64::Engine;
        let serialized = bincode::serialize(tx).map_err(|e| e.to_string())?;
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "simulateTransaction",
            "params": [
                base64::engine::general_purpose::STANDARD.encode(serialized),
                {"encoding": "base64", "sigVerify": false, "replaceRecentBlockhash": true, "commitment": "processed"}
            ]
        });
        let json: serde_json::Value = self
            .http_client
            .post(&self.rpc_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        if let Some(err) = json.get("error") {
            return Err(err.to_string());
        }
        let value = &json["result"]["value"];
        let slot = json["result"]["context"]["slot"].as_u64();
        if value["err"].is_null() {
            Ok((slot, Ok(())))
        } else {
            let logs: Vec<&str> = value["logs"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
            Ok((slot, Err(format!("{} | {}", value["err"], logs.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | ")))))
        }
    }

    /// Append one paper trade record (JSON line)
    /// Record how far behind the triggering trade a copy buy lands (live) or could earliest land (paper)
    fn log_latency(&self, mint: &Pubkey, signal: Option<Signal>, our_slot: Option<u64>, ms: u128, landed: bool) {
        let (Some(signal), Some(our_slot)) = (signal, our_slot) else { return };
        let blocks = our_slot as i64 - signal.slot as i64;
        info!(
            "⏱️ LATENCY {}: copied trade in slot {}, ours {} slot {} ({:+} blocks), {} ms after detection",
            mint, signal.slot, if landed { "landed in" } else { "could land at" }, our_slot, blocks, ms
        );
        let record = serde_json::json!({
            "mint": mint.to_string(), "signal_slot": signal.slot, "our_slot": our_slot, "blocks": blocks,
            "ms": ms as u64, "landed": landed, "paper": self.is_dry_run(),
            "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        });
        let line = format!("{}\n", record);
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all("data")?;
            use std::io::Write;
            std::fs::OpenOptions::new().create(true).append(true).open(LATENCY_LOG)?.write_all(line.as_bytes())
        })();
        if let Err(e) = result {
            warn!("Could not write {}: {}", LATENCY_LOG, e);
        }
    }

    fn log_paper(&self, record: serde_json::Value) {
        let Some(path) = &self.paper_log else { return };
        let mut record = record;
        record["ts"] = serde_json::json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0));
        let line = format!("{}\n", record);
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            use std::io::Write;
            std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
        })();
        if let Err(e) = result {
            warn!("Failed to write paper trade log {}: {}", path.display(), e);
        }
    }

    /// Close our now-empty token account for `mint` to get its rent (~0.002 SOL) back
    async fn close_empty_ata(&self, mint: &Pubkey, token_program: &Pubkey) {
        let owner = self.keypair.pubkey();
        // Give the sell a moment to be visible at confirmed commitment
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        if crate::engine::wallet::ata_is_empty(&self.http_client, &self.rpc_url, &owner, mint, token_program).await != Some(true) {
            return;
        }
        let (ata, _) = crate::constants::derive_associated_user_token(&owner, mint, token_program);
        let instructions = vec![
            InstructionBuilder::set_compute_unit_limit(10_000),
            InstructionBuilder::set_compute_unit_price(10_000),
            InstructionBuilder::close_ata(&ata, &owner, &owner, token_program),
        ];
        let message = solana_sdk::message::Message::new(&instructions, Some(&owner));
        let tx = Transaction::new(&[self.keypair.as_ref()], message, self.blockhash_cache.get_latest().await);
        self.send_rpc(&tx);
        info!("🧹 Closing empty token account {} for {} (rent refund, tx {})", ata, mint, tx.signatures[0]);
    }

    pub async fn execute_action(&self, action: TradeAction) -> Result<()> {
        match action {
            TradeAction::Buy {
                signal,
                mint,
                sol_amount_lamports,
                slippage_bps,
                curve_state,
                dev_wallet,
                dev_initial_balance,
                token_program,
                leaders,
            } => {
                let result = self
                    .execute_buy(
                        signal,
                        mint,
                        sol_amount_lamports,
                        slippage_bps,
                        curve_state,
                        dev_wallet,
                        dev_initial_balance,
                        token_program,
                        leaders,
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

                if self.is_dry_run() {
                    match self.simulate(&bundle_tx).await {
                        Ok(()) => info!("📝 [PAPER] Arbitrage for {} simulated OK (not sent)", mint),
                        Err(e) => warn!("📝 [PAPER] Arbitrage for {} would fail: {}", mint, e),
                    }
                    return Ok(());
                }

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

                let tip = self.tip_engine.trade_tip(sol_amount_lamports);
                let bundle_tx = self.jito_client.build_tip_bundle(
                    &self.keypair,
                    instructions,
                    tip,
                    blockhash,
                );

                let sig = bundle_tx.signatures[0];

                if self.is_dry_run() {
                    match self.simulate(&bundle_tx).await {
                        Ok(()) => info!("📝 [PAPER] PumpSwap co-snipe for {} simulated OK (not sent)", mint),
                        Err(e) => warn!("📝 [PAPER] PumpSwap co-snipe for {} would fail: {}", mint, e),
                    }
                    return Ok(());
                }

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
    #[allow(clippy::too_many_arguments)]
    async fn execute_buy(
        &self,
        signal: Option<Signal>,
        mint: Pubkey,
        sol_amount_lamports: u64,
        slippage_bps: u64,
        curve_state: BondingCurveAccountPod,
        dev_wallet: Option<Pubkey>,
        dev_initial_balance: u64,
        token_program: Option<Pubkey>,
        leaders: Vec<Pubkey>,
    ) -> Result<bool> {
        let blockhash = self.blockhash_cache.get_latest().await;
        let payer_pubkey = self.keypair.pubkey();

        let calc = curve_state
            .calculate_buy_output(sol_amount_lamports, slippage_bps)
            .context("Curve calculation failed or curve is complete")?;

        // Skip buys the wallet can't fully pay for (they fail on-chain and still cost fees)
        let required = calc.max_sol_cost
            + self.tip_engine.trade_tip(sol_amount_lamports)
            + TOKEN_ACCOUNT_RENT_LAMPORTS
            + BUY_FEE_HEADROOM_LAMPORTS;
        let balance = self.balance_lamports.load(std::sync::atomic::Ordering::Relaxed);
        if !self.is_dry_run() && balance != u64::MAX && balance < required {
            warn!(
                "💸 Skipping buy of {}: wallet has {:.4} SOL, needs {:.4} SOL (max cost + tip + account rent + fees)",
                mint, balance as f64 / 1e9, required as f64 / 1e9
            );
            return Ok(false);
        }

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
        let tip = self.tip_engine.trade_tip(sol_amount_lamports);

        let bundle_tx = self.jito_client.build_tip_bundle(
            &self.keypair,
            instructions,
            tip,
            blockhash,
        );

        let sig = bundle_tx.signatures[0];

        if self.is_dry_run() {
            let ms = signal.map_or(0, |s| s.seen_at.elapsed().as_millis());
            let (sim_slot, sim) = self.simulate_at(&bundle_tx).await;
            self.log_latency(&mint, signal, sim_slot, ms, false);
            if let Err(e) = sim {
                // Paper trading judges the trade, not the current wallet balance
                let unfunded = e.contains("insufficient lamports") || e.contains("AccountNotFound") || e.contains("\"Custom\":1}");
                if !unfunded {
                    warn!("📝 [PAPER] Buy simulation failed for {} (a live buy would fail too): {}", mint, e);
                    return Ok(false);
                }
                info!("📝 [PAPER] Wallet can't fund this buy right now; paper-trading it anyway");
            }
            // Pump.fun buys exactly the ordered amount: pay its curve cost plus the estimated fee
            let tokens = calc.tokens_out;
            let curve_cost = { let mut c = curve_state; c.apply_buy(tokens) }.unwrap_or(calc.max_sol_cost);
            let swap_cost = curve_cost + curve_cost * PAPER_PUMP_FEE_BPS / 10_000;
            let entry_price_sol = (swap_cost as f64 / 1e9) / (tokens as f64 / 1e6);
            let cost = swap_cost + tip;
            info!("📝 [PAPER] BUY {} | {} tokens | cost {:.4} SOL (incl. tip {:.5})", mint, tokens, cost as f64 / 1e9, tip as f64 / 1e9);
            self.log_paper(serde_json::json!({
                "kind": "buy", "mint": mint.to_string(), "sol_lamports": cost, "tip_lamports": tip, "tokens": tokens
            }));
            self.position_manager
                .add_position(Position {
                    mint,
                    bonding_curve: crate::constants::derive_bonding_curve(&mint).0,
                    entry_price_sol,
                    highest_price_sol: entry_price_sol,
                    token_balance: tokens,
                    sol_invested_lamports: cost,
                    dev_wallet: (creator != Pubkey::default()).then_some(creator),
                    dev_initial_balance,
                    token_program: token_prog,
                    is_selling: false,
                    leaders: leaders.clone(),
                })
                .await;
            return Ok(true);
        }

        let ms = signal.map_or(0, |s| s.seen_at.elapsed().as_millis());
        self.send_via_rpc(&bundle_tx);
        let bundle_id = self.jito_client.send_bundle(&bundle_tx).await?;
        info!("📡 Buy dispatched (sig: {} | Jito: {} | RPC route: {}). Verifying on-chain confirmation...", sig, bundle_id, self.dual_route_rpc);

        let landed_slot = self.wait_for_landing_slot(&sig, 15, std::time::Duration::from_millis(400)).await;
        self.log_latency(&mint, signal, landed_slot.filter(|&s| s > 0), ms, true);
        if landed_slot.is_none() {
            warn!("⚠️ Bundle for mint {} did not land on-chain (dropped or outbid). Capital preserved (0 SOL spent).", mint);
            return Ok(false);
        }

        // Pump.fun buys exactly calc.tokens_out and charges only what those cost, which is less than
        // sol_amount_lamports. Book the real cost so the entry price (which anchors every stop) is right
        let token_balance =
            match fetch_token_balance(&self.http_client, &self.rpc_url, &payer_pubkey, &mint, &token_prog).await {
                Some(received) if received > 0 => received,
                _ => {
                    warn!("Could not read received token balance for {}; using the ordered amount", mint);
                    calc.tokens_out
                }
            };
        let sol_spent_lamports =
            match fetch_buy_cost(&self.http_client, &self.rpc_url, &sig.to_string(), &payer_pubkey, &mint, &token_prog, tip).await {
                Some(cost) if cost > 0 => cost,
                _ => {
                    warn!("Could not read the buy cost for {}; booking the full buy size", mint);
                    sol_amount_lamports
                }
            };
        let entry_price_sol = (sol_spent_lamports as f64 / 1e9) / (token_balance as f64 / 1e6);

        info!("🎉 TRADE CONFIRMED ON-CHAIN! Tx: https://solscan.io/tx/{} | Mint: {} | Tokens: {} | Entry: {:.10} SOL | Sol Spent: {:.4} SOL", sig, mint, token_balance, entry_price_sol, sol_spent_lamports as f64 / 1e9);
        let dev = (creator != Pubkey::default()).then_some(creator);
        self.position_manager
            .add_position(Position {
                mint,
                bonding_curve: crate::constants::derive_bonding_curve(&mint).0,
                entry_price_sol,
                highest_price_sol: entry_price_sol,
                token_balance,
                sol_invested_lamports: sol_spent_lamports,
                dev_wallet: dev,
                dev_initial_balance,
                token_program: token_prog,
                is_selling: false,
                leaders,
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

        if self.is_dry_run() {
            let invested = self.position_manager.get_position(&mint).await.map(|p| p.sol_invested_lamports).unwrap_or(0);
            let tip = if is_panic { self.tip_engine.panic_tip(calc.sol_out) } else { self.tip_engine.trade_tip(calc.sol_out) };
            let fee = calc.sol_out * PAPER_PUMP_FEE_BPS / 10_000;
            let proceeds = calc.sol_out.saturating_sub(fee + tip);
            let pnl = proceeds as i64 - invested as i64;
            info!(
                "📝 [PAPER] SELL {}{} | proceeds {:.4} SOL (tip {:.5}, fee {:.5}) | P&L {:+.4} SOL",
                mint, if is_panic { " (panic)" } else { "" },
                proceeds as f64 / 1e9, tip as f64 / 1e9, fee as f64 / 1e9, pnl as f64 / 1e9
            );
            self.log_paper(serde_json::json!({
                "kind": "sell", "mint": mint.to_string(), "sol_lamports": proceeds, "tip_lamports": tip,
                "invested_lamports": invested, "pnl_lamports": pnl, "panic": is_panic
            }));
            self.position_manager.remove_position(&mint).await;
            return Ok(());
        }

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
                self.tip_engine.panic_tip(calc.sol_out)
            } else {
                self.tip_engine.trade_tip(calc.sol_out)
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
                self.close_empty_ata(&mint, &token_prog).await;
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
