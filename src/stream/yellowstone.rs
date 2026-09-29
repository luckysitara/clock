use anyhow::{Context, Result};
use futures::StreamExt;
use log::{error, info, warn};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, RwLock};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterTransactions,
};

use crate::{
    config::BotConfig,
    constants::{
        BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_U64, INITIAL_REAL_TOKEN_RESERVES,
        INITIAL_VIRTUAL_SOL_RESERVES, INITIAL_VIRTUAL_TOKEN_RESERVES, PUMPFUN_PROGRAM,
        SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64, TOTAL_TOKEN_SUPPLY,
    },
    decoders::{
        fast_is_discriminator, BondingCurveAccountPod, CreateInstructionView, FastPubkeyFilter,
        PumpFunBuyPod, PumpFunSellPod,
    },
    engine::{
        executor::TradeAction,
        position_manager::{ExitReason, PositionManager},
    },
};

pub struct YellowstoneStreamer {
    config: Arc<BotConfig>,
    curve_cache: Arc<RwLock<HashMap<Pubkey, BondingCurveAccountPod>>>,
    position_manager: Arc<PositionManager>,
    trade_sender: mpsc::Sender<TradeAction>,
}

impl YellowstoneStreamer {
    pub fn new(
        config: Arc<BotConfig>,
        curve_cache: Arc<RwLock<HashMap<Pubkey, BondingCurveAccountPod>>>,
        position_manager: Arc<PositionManager>,
        trade_sender: mpsc::Sender<TradeAction>,
    ) -> Self {
        Self {
            config,
            curve_cache,
            position_manager,
            trade_sender,
        }
    }

    /// Connect to Triton Yellowstone gRPC with retry loop
    pub async fn run_loop(&self) -> Result<()> {
        let target_pubkeys: Vec<Pubkey> = self
            .config
            .target_wallets
            .iter()
            .filter_map(|s| Pubkey::from_str(s).ok())
            .collect();

        let whale_filter = FastPubkeyFilter::new(&target_pubkeys);

        info!(
            "🔌 Connecting to Triton Yellowstone gRPC at: {}",
            self.config.yellowstone_grpc_url
        );
        if self.config.yellowstone_x_token.is_some() {
            info!("🔑 Triton x-token authentication provided");
        } else {
            warn!("⚠️ No x-token provided; Triton Yellowstone gRPC usually requires an x-token header");
        }

        loop {
            match self.stream_session(&whale_filter).await {
                Ok(_) => {
                    warn!("Yellowstone stream closed gracefully, reconnecting in 1s...");
                }
                Err(e) => {
                    error!("❌ Yellowstone gRPC stream error: {:#}. Reconnecting in 2s...", e);
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn stream_session(&self, whale_filter: &FastPubkeyFilter) -> Result<()> {
        let mut builder = GeyserGrpcClient::build_from_shared(self.config.yellowstone_grpc_url.clone())
            .context("Failed to build gRPC client from endpoint URL")?;

        if self.config.yellowstone_grpc_url.starts_with("https://") {
            let tls = yellowstone_grpc_proto::tonic::transport::ClientTlsConfig::new().with_enabled_roots();
            builder = builder.tls_config(tls).context("Failed to configure TLS for Yellowstone gRPC")?;
        }

        if let Some(token) = &self.config.yellowstone_x_token {
            builder = builder
                .x_token(Some(token.clone()))
                .context("Failed to set Triton x-token")?;
        }

        let mut client = builder
            .connect()
            .await
            .context("Failed to connect to Triton Yellowstone gRPC endpoint")?;

        info!("✅ Connected to Triton Yellowstone gRPC. Configuring subscriptions...");

        // 1. Transaction filter: Pump.fun program + Target Top Whales
        let mut account_include = vec![PUMPFUN_PROGRAM.to_string()];
        account_include.extend(self.config.target_wallets.clone());

        let mut transactions = HashMap::new();
        transactions.insert(
            "pumpfun_whales".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false), // Only successful / pre-cleared transactions
                signature: None,
                account_include,
                account_exclude: vec![],
                account_required: vec![],
            },
        );

        // 2. Account filter: Real-time updates for all Pump.fun bonding curves
        let mut accounts = HashMap::new();
        accounts.insert(
            "pumpfun_curves".to_string(),
            SubscribeRequestFilterAccounts {
                account: vec![],
                owner: vec![PUMPFUN_PROGRAM.to_string()],
                filters: vec![],
                nonempty_txn_signature: None,
            },
        );

        let request = SubscribeRequest {
            accounts,
            slots: HashMap::new(),
            transactions,
            transactions_status: HashMap::new(),
            entry: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            commitment: Some(CommitmentLevel::Processed as i32),
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        };

        let (_sink, mut stream) = client
            .subscribe_with_request(Some(request))
            .await
            .context("Failed to open subscribe stream with Triton")?;

        info!("🎯 Stream active! Monitoring Processed slot data for Top 20 whales and launches...");

        while let Some(msg_res) = stream.next().await {
            let msg = match msg_res {
                Ok(m) => m,
                Err(e) => {
                    error!("Error receiving gRPC message: {}", e);
                    break;
                }
            };

            match msg.update_oneof {
                // Real-time Bonding Curve Account State Updates
                Some(UpdateOneof::Account(acc_update)) => {
                    if let Some(account_info) = acc_update.account {
                        if let Ok(pubkey) = Pubkey::try_from(account_info.pubkey.as_slice()) {
                            if let Some(curve_pod) =
                                BondingCurveAccountPod::read_from_account(&account_info.data)
                            {
                                let current_price = curve_pod.current_spot_price_sol();

                                // Update in-memory curve cache
                                {
                                    let mut cache = self.curve_cache.write().await;
                                    cache.insert(pubkey, curve_pod);
                                }

                                // Check active position exits
                                if let Some((pos, reason)) = self
                                    .position_manager
                                    .on_price_update(&pubkey, current_price)
                                    .await
                                {
                                    let is_panic = reason == ExitReason::DevDumpPanic || reason == ExitReason::HardStopLoss;
                                    let _ = self
                                        .trade_sender
                                        .send(TradeAction::Sell {
                                            mint: pos.mint,
                                            token_amount: pos.token_balance,
                                            slippage_bps: self.config.slippage_bps,
                                            curve_state: curve_pod,
                                            is_panic,
                                        })
                                        .await;
                                }

                                if curve_pod.complete {
                                    info!(
                                        "🎓 GRADUATION DETECTED: Curve {} reached 100%! Ready for PumpSwap migration.",
                                        pubkey
                                    );
                                }
                            }
                        }
                    }
                }

                // Real-time Transactions from Top 20 Whales & Token Launches
                Some(UpdateOneof::Transaction(tx_update)) => {
                    let tx_info = match tx_update.transaction {
                        Some(t) => t,
                        None => continue,
                    };
                    let tx = match tx_info.transaction {
                        Some(t) => t,
                        None => continue,
                    };
                    let msg_data = match tx.message {
                        Some(m) => m,
                        None => continue,
                    };

                    // Extract account keys
                    let account_keys: Vec<[u8; 32]> = msg_data
                        .account_keys
                        .iter()
                        .filter_map(|k| k.as_slice().try_into().ok())
                        .collect();

                    // Check if one of our Top 20 whale wallets is involved
                    let matched_whale = whale_filter.matches_any(&account_keys);

                    // Scan instructions
                    for ix in &msg_data.instructions {
                        let data = &ix.data;

                        // 1. BUY INSTRUCTION
                        if fast_is_discriminator(data, BUY_DISCRIMINATOR_U64) {
                            if let Some(whale) = matched_whale {
                                if let Some(buy_pod) = PumpFunBuyPod::read_from_raw(data) {
                                    let whale_sol_spent = buy_pod.max_sol_cost as f64 / 1e9;
                                    info!(
                                        "🚨 WHALE DETECTED [{}]: Bought on Pump.fun with up to {} SOL",
                                        whale, whale_sol_spent
                                    );

                                    // Mint is typically account index #2 in Buy instruction
                                    if ix.accounts.len() > 2 {
                                        let mint_idx = ix.accounts[2] as usize;
                                        if mint_idx < account_keys.len() {
                                            let mint =
                                                Pubkey::new_from_array(account_keys[mint_idx]);

                                            if !self.position_manager.has_position(&mint).await {
                                                let current_invested = self.position_manager.total_invested_sol().await;
                                                if current_invested + self.config.copy_trade_amount_sol <= self.config.max_position_sol {
                                                    // Get current bonding curve state
                                                    let curve_state = {
                                                        let cache = self.curve_cache.read().await;
                                                        cache.get(&mint).copied()
                                                    };

                                                    if let Some(curve) = curve_state {
                                                        if !curve.complete {
                                                            let my_sol_lamports = (self
                                                                .config
                                                                .copy_trade_amount_sol
                                                                * 1e9)
                                                                as u64;

                                                            info!(
                                                                "⚡ TRIGGERING WHALE COPY-TRADE BUY: {} SOL on Mint {}",
                                                                self.config.copy_trade_amount_sol, mint
                                                            );

                                                            let copy_slippage = std::cmp::max(self.config.slippage_bps, 1500);
                                                            let _ = self
                                                                .trade_sender
                                                                .send(TradeAction::Buy {
                                                                    mint,
                                                                    sol_amount_lamports: my_sol_lamports,
                                                                    slippage_bps: copy_slippage,
                                                                    curve_state: curve,
                                                                    dev_wallet: None,
                                                                })
                                                                .await;
                                                        }
                                                    }
                                                } else {
                                                    info!(
                                                        "⏸️ Max open capital reached ({:.2}/{:.2} SOL). Skipping whale copy-trade on {}",
                                                        current_invested, self.config.max_position_sol, mint
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // 2. SELL INSTRUCTION (Check for Dev Dump or Whale Exit)
                        else if fast_is_discriminator(data, SELL_DISCRIMINATOR_U64)
                            || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64)
                        {
                            if let Some(sell_pod) = PumpFunSellPod::read_from_raw(data) {
                                if ix.accounts.len() > 2 {
                                    let mint_idx = ix.accounts[2] as usize;
                                    if mint_idx < account_keys.len() {
                                        let mint = Pubkey::new_from_array(account_keys[mint_idx]);
                                        let signer = Pubkey::new_from_array(account_keys[0]);

                                        // Check if this is the token creator dumping
                                        if let Some(pos) = self
                                            .position_manager
                                            .check_dev_dump(&mint, &signer, sell_pod.amount)
                                            .await
                                        {
                                            let curve_state = {
                                                let cache = self.curve_cache.read().await;
                                                cache.get(&mint).copied()
                                            };

                                            if let Some(curve) = curve_state {
                                                let _ = self
                                                    .trade_sender
                                                    .send(TradeAction::Sell {
                                                        mint,
                                                        token_amount: pos.token_balance,
                                                        slippage_bps: self.config.slippage_bps,
                                                        curve_state: curve,
                                                        is_panic: true,
                                                    })
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // 3. CREATE INSTRUCTION (First-Block Launch Snipe)
                        else if fast_is_discriminator(data, CREATE_DISCRIMINATOR_U64) {
                            if let Some(view) = CreateInstructionView::parse(data) {
                                if ix.accounts.len() > 0 {
                                    let mint_idx = ix.accounts[0] as usize;
                                    if mint_idx < account_keys.len() {
                                        let mint = Pubkey::new_from_array(account_keys[mint_idx]);
                                        let creator = view.creator.unwrap_or_else(|| {
                                            if ix.accounts.len() > 7 && (ix.accounts[7] as usize) < account_keys.len() {
                                                Pubkey::new_from_array(account_keys[ix.accounts[7] as usize])
                                            } else {
                                                Pubkey::new_from_array(account_keys[0])
                                            }
                                        });
                                        info!(
                                            "🌟 NEW TOKEN LAUNCH: [{}] \"{}\" | Mint: {} | Dev: {}",
                                            view.symbol, view.name, mint, creator
                                        );

                                        // Option C Hybrid: First-Block Launch Snipe
                                        if !self.position_manager.has_position(&mint).await {
                                            let current_invested = self.position_manager.total_invested_sol().await;
                                            if current_invested + self.config.copy_trade_amount_sol <= self.config.max_position_sol {
                                                let mut initial_curve = BondingCurveAccountPod {
                                                    virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                                                    virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                                                    real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                                                    real_sol_reserves: 0,
                                                    token_total_supply: TOTAL_TOKEN_SUPPLY,
                                                    complete: false,
                                                    creator,
                                                };

                                                // Inspect if creator bought initial tokens in the same transaction
                                                let mut dev_bought_sol = 0.0;
                                                for other_ix in &msg_data.instructions {
                                                    if fast_is_discriminator(&other_ix.data, BUY_DISCRIMINATOR_U64) {
                                                        if let Some(buy_pod) = PumpFunBuyPod::read_from_raw(&other_ix.data) {
                                                            if other_ix.accounts.len() > 2 {
                                                                let m_idx = other_ix.accounts[2] as usize;
                                                                if m_idx < account_keys.len() && account_keys[m_idx] == mint.to_bytes() {
                                                                    let dev_sol = initial_curve.apply_buy(buy_pod.amount);
                                                                    dev_bought_sol = dev_sol.unwrap_or(0) as f64 / 1e9;
                                                                    info!(
                                                                        "🎯 Dev initial buy detected in create tx: {} tokens (~{:.4} SOL). Curve updated.",
                                                                        buy_pod.amount,
                                                                        dev_bought_sol
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                {
                                                    let mut cache = self.curve_cache.write().await;
                                                    cache.insert(mint, initial_curve);
                                                }

                                                // Smart Money / Dev Concurrence Filter:
                                                // Only snipe launch if dev put serious SOL into the curve (>= min_creator_buy_sol)
                                                // OR if a target whale/sniper is in the creation transaction!
                                                let is_smart_money_in_launch = matched_whale.is_some();
                                                if dev_bought_sol < self.config.min_creator_buy_sol && !is_smart_money_in_launch {
                                                    info!(
                                                        "🛡️ Skipping unbacked launch [{}] {}: Dev bought only {:.4} SOL (min: {:.2} SOL) and no target snipers/whales entered.",
                                                        view.symbol, mint, dev_bought_sol, self.config.min_creator_buy_sol
                                                    );
                                                    continue;
                                                }

                                                let my_sol_lamports = (self.config.copy_trade_amount_sol * 1e9) as u64;
                                                let snipe_slippage = std::cmp::max(self.config.slippage_bps, 2500);

                                                info!(
                                                    "⚡ TRIGGERING FIRST-BLOCK SNIPE: {} SOL on [{}] Mint: {} (Dev Buy: {:.4} SOL | slippage: {} bps)",
                                                    self.config.copy_trade_amount_sol, view.symbol, mint, dev_bought_sol, snipe_slippage
                                                );

                                                let _ = self
                                                    .trade_sender
                                                    .send(TradeAction::Buy {
                                                        mint,
                                                        sol_amount_lamports: my_sol_lamports,
                                                        slippage_bps: snipe_slippage,
                                                        curve_state: initial_curve,
                                                        dev_wallet: Some(creator),
                                                    })
                                                    .await;
                                            } else {
                                                info!(
                                                    "⏸️ Max open capital reached ({:.2}/{:.2} SOL). Skipping snipe on [{}]",
                                                    current_invested, self.config.max_position_sol, view.symbol
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        Ok(())
    }
}
