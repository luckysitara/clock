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
        executor::{Signal, TradeAction},
        position_manager::{ExitReason, PositionManager, Reservation},
    },
};

#[derive(Clone, Copy)]
struct RawInstructionView<'a> {
    program_id_index: usize,
    accounts: &'a [u8],
    data: &'a [u8],
}

/// Reconnect when the stream delivers nothing for this long
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub struct YellowstoneStreamer {
    config: Arc<BotConfig>,
    curve_cache: Arc<RwLock<HashMap<Pubkey, BondingCurveAccountPod>>>,
    position_manager: Arc<PositionManager>,
    trade_sender: mpsc::Sender<TradeAction>,
    rpc_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    /// Recent signal-wallet buys per mint: (wallet, when)
    signal_hits: std::sync::Mutex<HashMap<Pubkey, Vec<(Pubkey, std::time::Instant)>>>,
}

impl YellowstoneStreamer {
    pub fn new(
        config: Arc<BotConfig>,
        curve_cache: Arc<RwLock<HashMap<Pubkey, BondingCurveAccountPod>>>,
        position_manager: Arc<PositionManager>,
        trade_sender: mpsc::Sender<TradeAction>,
    ) -> Self {
        let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new(
            config.solana_rpc_url.clone(),
        ));
        Self {
            config,
            curve_cache,
            position_manager,
            trade_sender,
            rpc_client,
            signal_hits: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Record a signal-wallet buy; returns the distinct signal wallets that bought `mint`
    /// within the window once there are enough of them (and resets the mint)
    fn record_signal_buy(&self, mint: Pubkey, wallet: Pubkey) -> Option<Vec<Pubkey>> {
        let window = Duration::from_secs(self.config.signal_window_secs);
        let now = std::time::Instant::now();
        let mut hits = self.signal_hits.lock().unwrap_or_else(|e| e.into_inner());
        hits.retain(|_, v| {
            v.retain(|(_, t)| now.duration_since(*t) <= window);
            !v.is_empty()
        });
        let entry = hits.entry(mint).or_default();
        if !entry.iter().any(|(w, _)| *w == wallet) {
            entry.push((wallet, now));
        }
        if entry.len() >= self.config.signal_min_wallets {
            let wallets = entry.iter().map(|(w, _)| *w).collect();
            hits.remove(&mint);
            Some(wallets)
        } else {
            None
        }
    }

    /// Reserve capital, fetch the curve and queue a copy buy that exits when any leader sells
    async fn trigger_copy_buy(&self, mint: Pubkey, token_prog: Pubkey, leaders: Vec<Pubkey>, label: &str, signal: Signal) {
        let max_position_lamports = (self.config.max_position_sol * 1e9) as u64;
        let my_sol_lamports = (self.config.copy_trade_amount_sol * 1e9) as u64;
        match self.position_manager.try_reserve(&mint, my_sol_lamports, max_position_lamports).await {
            Reservation::Reserved => {}
            Reservation::AlreadyActive => return,
            Reservation::CapitalLimit { committed_sol } => {
                info!(
                    "⏸️ Max open capital reached ({:.2}/{:.2} SOL). Skipping {} on {}",
                    committed_sol, self.config.max_position_sol, label, mint
                );
                return;
            }
        }

        let curve = match self.get_curve(&mint).await {
            Some(c) if !c.complete => c,
            _ => {
                self.position_manager.release(&mint).await;
                return;
            }
        };

        // A creator buying its own token is setting up a dump onto copiers, not trading
        if leaders.contains(&curve.creator) {
            info!("🛡️ Skipping {} on {}: the copied wallet created this token", label, mint);
            self.position_manager.release(&mint).await;
            return;
        }

        info!(
            "⚡ TRIGGERING {}: {} SOL on Mint {} (exits when {} leader(s) sell)",
            label, self.config.copy_trade_amount_sol, mint, leaders.len()
        );
        if self
            .trade_sender
            .send(TradeAction::Buy {
                signal: Some(signal),
                mint,
                sol_amount_lamports: my_sol_lamports,
                slippage_bps: std::cmp::max(self.config.slippage_bps, 1500),
                curve_state: curve,
                dev_wallet: None,
                dev_initial_balance: 0,
                token_program: Some(token_prog),
                leaders,
            })
            .await
            .is_err()
        {
            self.position_manager.release(&mint).await;
        }
    }

    /// Bonding curve state from the cache (keyed by mint or curve PDA), falling back to RPC
    async fn get_curve(&self, mint: &Pubkey) -> Option<BondingCurveAccountPod> {
        let (bonding_curve, _) = crate::constants::derive_bonding_curve(mint);
        {
            let cache = self.curve_cache.read().await;
            if let Some(c) = cache.get(mint).or_else(|| cache.get(&bonding_curve)) {
                return Some(*c);
            }
        }
        let acc = self.rpc_client.get_account(&bonding_curve).await.ok()?;
        let pod = BondingCurveAccountPod::read_from_account(&acc.data)?;
        self.curve_cache.write().await.insert(bonding_curve, pod);
        Some(pod)
    }

    /// Sell positions held past MAX_HOLD_MINS
    async fn sell_expired(&self) {
        let max_age = Duration::from_secs(self.config.max_hold_mins * 60);
        for pos in self.position_manager.take_expired(max_age).await {
            match self.get_curve(&pos.mint).await {
                Some(curve) if !curve.complete => {
                    let _ = self
                        .trade_sender
                        .send(TradeAction::Sell {
                            mint: pos.mint,
                            token_amount: pos.token_balance,
                            slippage_bps: self.config.slippage_bps,
                            curve_state: curve,
                            is_panic: false,
                            token_program: Some(pos.token_program),
                        })
                        .await;
                }
                _ => {
                    warn!("⌛ Can't sell {} on the bonding curve (no curve or graduated)", pos.mint);
                }
            }
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
        let signal_pubkeys: Vec<Pubkey> = self
            .config
            .signal_wallets
            .iter()
            .filter_map(|s| Pubkey::from_str(s).ok())
            .collect();
        let signal_filter = FastPubkeyFilter::new(&signal_pubkeys);

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
            match self.stream_session(&whale_filter, &signal_filter, !signal_pubkeys.is_empty()).await {
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

    async fn stream_session(
        &self,
        whale_filter: &FastPubkeyFilter,
        signal_filter: &FastPubkeyFilter,
        signals_enabled: bool,
    ) -> Result<()> {
        let max_position_lamports = (self.config.max_position_sol * 1e9) as u64;
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

        info!("🎯 Stream active! Monitoring Processed slot data for {} target wallets and launches...", self.config.target_wallets.len());

        let mut last_hold_check = std::time::Instant::now();
        loop {
            if self.config.max_hold_mins > 0 && last_hold_check.elapsed() >= Duration::from_secs(15) {
                last_hold_check = std::time::Instant::now();
                self.sell_expired().await;
            }
            // Pump.fun never goes quiet for this long; silence means a dead connection
            // (e.g. a network drop that left the socket open), so reconnect
            let msg_res = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
                Ok(Some(m)) => m,
                Ok(None) => break,
                Err(_) => anyhow::bail!("no stream data for {}s, connection presumed dead", STREAM_IDLE_TIMEOUT.as_secs()),
            };
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

                                // Update in-memory curve cache (graduated curves are dropped; size is capped)
                                let newly_graduated = {
                                    let mut cache = self.curve_cache.write().await;
                                    if curve_pod.complete {
                                        cache.remove(&pubkey).map_or(false, |prev| !prev.complete)
                                    } else {
                                        if cache.len() >= self.config.max_curve_cache_entries
                                            && !cache.contains_key(&pubkey)
                                        {
                                            info!(
                                                "🧹 Curve cache reached {} entries, clearing (misses fall back to RPC)",
                                                cache.len()
                                            );
                                            cache.clear();
                                        }
                                        cache.insert(pubkey, curve_pod);
                                        false
                                    }
                                };

                                // Check active position exits (a graduated curve can't be sold into)
                                let exit = if curve_pod.complete {
                                    None
                                } else {
                                    self.position_manager.on_price_update(&pubkey, current_price).await
                                };
                                if let Some((pos, reason)) = exit {
                                    let is_panic = reason == ExitReason::DevDumpPanic || reason == ExitReason::HardStopLoss || reason == ExitReason::TrailingStop;
                                    let _ = self
                                        .trade_sender
                                        .send(TradeAction::Sell {
                                            mint: pos.mint,
                                            token_amount: pos.token_balance,
                                            slippage_bps: self.config.slippage_bps,
                                            curve_state: curve_pod,
                                            is_panic,
                                            token_program: Some(pos.token_program),
                                        })
                                        .await;
                                }

                                if newly_graduated {
                                    info!(
                                        "🎓 GRADUATION DETECTED: Curve {} reached 100%! Ready for PumpSwap migration.",
                                        pubkey
                                    );
                                    if let Some(mint) = self.position_manager.mint_for_curve(&pubkey).await {
                                        warn!(
                                            "🎓 HELD TOKEN GRADUATED: {} can no longer be sold on the bonding curve; sell it on PumpSwap/Raydium manually",
                                            mint
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // Real-time Transactions from Top Whales & Token Launches (including Inner CPI bot trades)
                Some(UpdateOneof::Transaction(tx_update)) => {
                    let trigger = Signal { slot: tx_update.slot, seen_at: std::time::Instant::now() };
                    let tx_info = match tx_update.transaction {
                        Some(t) => t,
                        None => continue,
                    };

                    // Skip transactions that failed on-chain
                    if tx_info.meta.as_ref().map_or(false, |m| m.err.is_some()) {
                        continue;
                    }

                    let tx = match tx_info.transaction {
                        Some(t) => t,
                        None => continue,
                    };
                    let msg_data = match tx.message {
                        Some(m) => m,
                        None => continue,
                    };

                    // Extract static account keys and loaded addresses from lookup tables (v0 tx support)
                    let mut account_keys: Vec<[u8; 32]> = msg_data
                        .account_keys
                        .iter()
                        .filter_map(|k| k.as_slice().try_into().ok())
                        .collect();

                    if let Some(ref meta) = tx_info.meta {
                        for k in &meta.loaded_writable_addresses {
                            if let Ok(arr) = k.as_slice().try_into() {
                                account_keys.push(arr);
                            }
                        }
                        for k in &meta.loaded_readonly_addresses {
                            if let Ok(arr) = k.as_slice().try_into() {
                                account_keys.push(arr);
                            }
                        }
                    }

                    // Check if one of our target whale wallets is involved
                    let matched_whale = whale_filter.matches_any(&account_keys);
                    let matched_signal = if signals_enabled { signal_filter.matches_any(&account_keys) } else { None };

                    let pumpfun_bytes = crate::constants::pumpfun_program_id().to_bytes();

                    // Flatten both top-level and inner CPI instructions
                    let mut all_instructions: Vec<RawInstructionView> =
                        Vec::with_capacity(msg_data.instructions.len() + 16);

                    for ix in &msg_data.instructions {
                        all_instructions.push(RawInstructionView {
                            program_id_index: ix.program_id_index as usize,
                            accounts: &ix.accounts,
                            data: &ix.data,
                        });
                    }

                    if let Some(ref meta) = tx_info.meta {
                        for inner in &meta.inner_instructions {
                            for inner_ix in &inner.instructions {
                                all_instructions.push(RawInstructionView {
                                    program_id_index: inner_ix.program_id_index as usize,
                                    accounts: &inner_ix.accounts,
                                    data: &inner_ix.data,
                                });
                            }
                        }
                    }

                    let mut processed_mints = std::collections::HashSet::new();

                    // Scan all instructions (both direct calls and router CPI calls)
                    for ix in &all_instructions {
                        let prog_idx = ix.program_id_index;
                        if prog_idx >= account_keys.len() || account_keys[prog_idx] != pumpfun_bytes {
                            continue;
                        }

                        let data = ix.data;

                        // 1. BUY INSTRUCTION
                        if fast_is_discriminator(data, BUY_DISCRIMINATOR_U64) {
                            if matched_whale.is_none() && matched_signal.is_none() {
                                continue;
                            }
                            // The buyer (account #6) or the fee payer must be the tracked wallet,
                            // not merely some account referenced elsewhere in the transaction
                            let buyer_key = ix.accounts.get(6).and_then(|&i| account_keys.get(i as usize));
                            let fee_payer_key = account_keys.get(..1);
                            let find = |filter: &FastPubkeyFilter| {
                                buyer_key
                                    .and_then(|k| filter.matches_any(std::slice::from_ref(k)))
                                    .or_else(|| fee_payer_key.and_then(|k| filter.matches_any(k)))
                            };
                            let whale = matched_whale.and_then(|_| find(whale_filter));
                            let signal = matched_signal.and_then(|_| find(signal_filter));
                            if whale.is_none() && signal.is_none() {
                                continue;
                            }
                            let Some(buy_pod) = PumpFunBuyPod::read_from_raw(data) else { continue };

                            // Mint is account index #2 in Buy instruction
                            let Some(&mint_idx) = ix.accounts.get(2) else { continue };
                            let Some(mint_bytes) = account_keys.get(mint_idx as usize) else { continue };
                            let mint = Pubkey::new_from_array(*mint_bytes);
                            if processed_mints.contains(&mint) {
                                continue;
                            }

                            let token_2022 = crate::constants::spl_token_2022_program_id();
                            let spl_token = crate::constants::spl_token_program_id();
                            let token_prog = ix
                                .accounts
                                .get(8)
                                .and_then(|&i| account_keys.get(i as usize))
                                .map(|k| Pubkey::new_from_array(*k))
                                .filter(|p| *p == token_2022 || *p == spl_token)
                                .unwrap_or_else(|| {
                                    if account_keys.iter().any(|k| *k == token_2022.to_bytes()) {
                                        token_2022
                                    } else {
                                        spl_token
                                    }
                                });

                            if let Some(whale) = whale {
                                // max_sol_cost includes the whale's slippage allowance; price the actual
                                // token amount on the cached curve when we have it
                                let estimated_cost = {
                                    let (bonding_curve, _) = crate::constants::derive_bonding_curve(&mint);
                                    let cache = self.curve_cache.read().await;
                                    cache
                                        .get(&bonding_curve)
                                        .or_else(|| cache.get(&mint))
                                        .and_then(|c| {
                                            let mut curve = *c;
                                            curve.apply_buy(buy_pod.amount)
                                        })
                                };
                                let whale_sol_spent = estimated_cost
                                    .unwrap_or(buy_pod.max_sol_cost)
                                    .min(buy_pod.max_sol_cost) as f64
                                    / 1e9;
                                info!(
                                    "🚨 WHALE DETECTED [{}]: Bought {} on Pump.fun for ~{:.4} SOL (max {:.4} SOL)",
                                    whale, mint, whale_sol_spent, buy_pod.max_sol_cost as f64 / 1e9
                                );

                                if whale_sol_spent < self.config.min_whale_buy_sol {
                                    info!(
                                        "🛡️ Skipping low-conviction whale buy [{}]: Spent only {:.4} SOL (< {:.2} SOL floor)",
                                        whale, whale_sol_spent, self.config.min_whale_buy_sol
                                    );
                                    continue;
                                }

                                processed_mints.insert(mint);
                                self.trigger_copy_buy(mint, token_prog, vec![whale], "WHALE COPY-TRADE BUY", trigger).await;
                            } else if let Some(signal_wallet) = signal {
                                // Fast snipers are a signal, not copied one by one: buy only when
                                // several of them pile into the same token within the window
                                if let Some(cluster) = self.record_signal_buy(mint, signal_wallet) {
                                    info!(
                                        "📶 SIGNAL CLUSTER on {}: {} signal wallets bought within {}s",
                                        mint, cluster.len(), self.config.signal_window_secs
                                    );
                                    processed_mints.insert(mint);
                                    self.trigger_copy_buy(mint, token_prog, cluster, "SIGNAL-CLUSTER BUY", trigger).await;
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
                                        let seller = if ix.accounts.len() > 6 && (ix.accounts[6] as usize) < account_keys.len() {
                                            Pubkey::new_from_array(account_keys[ix.accounts[6] as usize])
                                        } else {
                                            Pubkey::new_from_array(account_keys[0])
                                        };

                                        // Check if this is the token creator dumping
                                        let dev_dump_pos = match self
                                            .position_manager
                                            .check_dev_dump(&mint, &seller, sell_pod.amount)
                                            .await
                                        {
                                            Some(pos) => Some(pos),
                                            None => {
                                                let fee_payer = Pubkey::new_from_array(account_keys[0]);
                                                if fee_payer != seller {
                                                    self.position_manager
                                                        .check_dev_dump(&mint, &fee_payer, sell_pod.amount)
                                                        .await
                                                } else {
                                                    None
                                                }
                                            }
                                        };

                                        // Otherwise: did a wallet we copied into this position just sell?
                                        let dev_dump_pos = match dev_dump_pos {
                                            Some(pos) => Some(pos),
                                            None if !self.config.leader_exit => None,
                                            None => match self.position_manager.check_leader_exit(&mint, &seller).await {
                                                Some(pos) => Some(pos),
                                                None => {
                                                    let fee_payer = Pubkey::new_from_array(account_keys[0]);
                                                    if fee_payer != seller {
                                                        self.position_manager.check_leader_exit(&mint, &fee_payer).await
                                                    } else {
                                                        None
                                                    }
                                                }
                                            },
                                        };

                                        if let Some(pos) = dev_dump_pos {
                                            match self.get_curve(&mint).await {
                                                Some(curve) => {
                                                    let _ = self
                                                        .trade_sender
                                                        .send(TradeAction::Sell {
                                                            mint,
                                                            token_amount: pos.token_balance,
                                                            slippage_bps: self.config.slippage_bps,
                                                            curve_state: curve,
                                                            is_panic: true,
                                                            token_program: Some(pos.token_program),
                                                        })
                                                        .await;
                                                }
                                                None => {
                                                    error!("❌ Dev dump on {} but curve state unavailable; panic sell not sent", mint);
                                                    self.position_manager.unmark_selling(&mint).await;
                                                }
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

                                        // A later Buy ix in this same tx must not trigger a second (copy-trade) buy
                                        if !processed_mints.insert(mint) {
                                            continue;
                                        }

                                        // Replay every buy of this mint in the create tx (top-level and inner CPI)
                                        // on a fresh curve, attributing SOL to the dev and to target whales
                                        let mut initial_curve = BondingCurveAccountPod {
                                            virtual_token_reserves: INITIAL_VIRTUAL_TOKEN_RESERVES,
                                            virtual_sol_reserves: INITIAL_VIRTUAL_SOL_RESERVES,
                                            real_token_reserves: INITIAL_REAL_TOKEN_RESERVES,
                                            real_sol_reserves: 0,
                                            token_total_supply: TOTAL_TOKEN_SUPPLY,
                                            complete: false,
                                            creator,
                                        };
                                        let mut dev_bought_sol = 0.0;
                                        let mut dev_tokens: u64 = 0;
                                        let mut whale_bought_sol = 0.0;
                                        let mut launch_whales: Vec<Pubkey> = Vec::new();
                                        for other_ix in &all_instructions {
                                            if account_keys.get(other_ix.program_id_index) != Some(&pumpfun_bytes)
                                                || !fast_is_discriminator(other_ix.data, BUY_DISCRIMINATOR_U64)
                                            {
                                                continue;
                                            }
                                            let Some(buy_pod) = PumpFunBuyPod::read_from_raw(other_ix.data) else { continue };
                                            let is_this_mint = other_ix
                                                .accounts
                                                .get(2)
                                                .and_then(|&i| account_keys.get(i as usize))
                                                == Some(&mint.to_bytes());
                                            if !is_this_mint {
                                                continue;
                                            }
                                            let sol = initial_curve.apply_buy(buy_pod.amount).unwrap_or(0) as f64 / 1e9;
                                            let buyer = other_ix
                                                .accounts
                                                .get(6)
                                                .and_then(|&i| account_keys.get(i as usize))
                                                .or_else(|| account_keys.first());
                                            if buyer == Some(&creator.to_bytes()) {
                                                dev_bought_sol += sol;
                                                dev_tokens = dev_tokens.saturating_add(buy_pod.amount);
                                            }
                                            if let Some(w) = buyer.and_then(|k| whale_filter.matches_any(std::slice::from_ref(k))) {
                                                whale_bought_sol += sol;
                                                if !launch_whales.contains(&w) {
                                                    launch_whales.push(w);
                                                }
                                                info!("🐋 Whale {} bought {:.4} SOL in create tx of {}", w, sol, mint);
                                            }
                                        }
                                        if dev_tokens > 0 {
                                            info!(
                                                "🎯 Dev initial buy detected in create tx: {} tokens (~{:.4} SOL). Curve updated.",
                                                dev_tokens, dev_bought_sol
                                            );
                                        }

                                        self.curve_cache.write().await.insert(mint, initial_curve);

                                        // Strict Smart Money Concurrence Filter:
                                        // Anonymous devs use fake 10+ SOL dev buys to bait sniper bots and dump in slot +1.
                                        // Only snipe when a target whale itself bought at least MIN_WHALE_BUY_SOL in the launch.
                                        if whale_bought_sol < self.config.min_whale_buy_sol {
                                            info!(
                                                "🛡️ Skipping launch [{}] {}: whale buy {:.4} SOL < {:.2} SOL floor (dev buy {:.4} SOL)",
                                                view.symbol, mint, whale_bought_sol, self.config.min_whale_buy_sol, dev_bought_sol
                                            );
                                            continue;
                                        }
                                        if dev_bought_sol < self.config.min_creator_buy_sol {
                                            info!(
                                                "🛡️ Skipping launch [{}] {}: dev buy {:.4} SOL < {:.2} SOL floor",
                                                view.symbol, mint, dev_bought_sol, self.config.min_creator_buy_sol
                                            );
                                            continue;
                                        }

                                        let my_sol_lamports = (self.config.copy_trade_amount_sol * 1e9) as u64;
                                        match self.position_manager.try_reserve(&mint, my_sol_lamports, max_position_lamports).await {
                                            Reservation::Reserved => {}
                                            Reservation::AlreadyActive => continue,
                                            Reservation::CapitalLimit { committed_sol } => {
                                                info!(
                                                    "⏸️ Max open capital reached ({:.2}/{:.2} SOL). Skipping snipe on [{}]",
                                                    committed_sol, self.config.max_position_sol, view.symbol
                                                );
                                                continue;
                                            }
                                        }

                                        let snipe_slippage = std::cmp::max(self.config.slippage_bps, 2500);
                                        info!(
                                            "⚡ TRIGGERING FIRST-BLOCK SNIPE: {} SOL on [{}] Mint: {} (Whale Buy: {:.4} SOL | Dev Buy: {:.4} SOL | slippage: {} bps)",
                                            self.config.copy_trade_amount_sol, view.symbol, mint, whale_bought_sol, dev_bought_sol, snipe_slippage
                                        );

                                        let token_prog = if account_keys.iter().any(|k| *k == crate::constants::spl_token_2022_program_id().to_bytes()) {
                                            crate::constants::spl_token_2022_program_id()
                                        } else {
                                            crate::constants::spl_token_program_id()
                                        };

                                        if self
                                            .trade_sender
                                            .send(TradeAction::Buy {
                                                signal: Some(trigger),
                                                mint,
                                                sol_amount_lamports: my_sol_lamports,
                                                slippage_bps: snipe_slippage,
                                                curve_state: initial_curve,
                                                dev_wallet: Some(creator),
                                                dev_initial_balance: dev_tokens,
                                                token_program: Some(token_prog),
                                                leaders: launch_whales,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            self.position_manager.release(&mint).await;
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
