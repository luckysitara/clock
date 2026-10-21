//! Backtest copy trading against real Pump.fun history.
//!
//! 1. Pulls every trade the TARGET_WALLETS made in the last N days.
//! 2. For each token they bought, pulls the token's own trade history around those buys.
//!    Every Pump.fun trade emits a TradeEvent with the curve reserves right after it, so
//!    this gives the exact price at every trade.
//! 3. Replays it all in slot order through the bot's real PositionManager: copy the leader's
//!    buy `--latency-slots` later at the curve price then, exit on take profit / stops / dev
//!    dump / leader sell, with Pump.fun fees, Jito tips and network fees.
//!
//! Fetched history is cached under data/backtest/, so reruns and sweeps are offline.
//!
//! Usage:
//!   backtest_copy [--days 7] [--latency-slots 2] [--hold-hours 6] [--parallel 8]
//!                 [--wallets a,b,...] [--max-wallet-txs 5000] [--max-mint-txs 4000]
//!                 [--max-tokens-per-wallet N] [--sweep] [--offline]
//!                 [--copy-sol X] [--max-position-sol X] [--tip-pct F] [--panic-tip-pct F]
//!                 [--take-profit X] [--hard-stop F] [--trailing F]
//!                 [--end UNIX_TS] [--no-leader-exit] [--max-hold-mins M] [--select-wallets [--min-select-trades N]]
//!                 [--leader-stats [--min-win-rate 0.7] [--min-closed 10]]
//!
//! --leader-stats only reports each copied wallet's own realized results over the period
//! (win rate, P&L, profit factor on tokens they bought and sold) and exits.

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
};

use clockit::{
    analytics::{fmt_time, rpc},
    config::BotConfig,
    constants::{derive_bonding_curve, PUMPFUN_PROGRAM},
    decoders::BondingCurveAccountPod,
    engine::{
        position_manager::{ExitReason, Position, PositionManager, Reservation},
        tip_engine::TipEngine,
    },
};

const CACHE_DIR: &str = "data/backtest";
/// getTransaction calls per JSON-RPC batch request
const BATCH: usize = 50;
/// Anchor self-CPI event tag, then sha256("event:TradeEvent")[..8]
const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];
const TRADE_EVENT_DISC: [u8; 8] = [0xbd, 0xdb, 0x7f, 0xd3, 0x4e, 0xe6, 0x61, 0xee];
/// Pump.fun protocol + creator fee per side (matches 5 real bot buys: cost = curve inflow x 1.0125)
const PUMP_FEE_BPS: u64 = 125;
/// Base fee + priority fee the executor sets (CU limit x price)
const BUY_NETWORK_FEE: u64 = 5_000 + 200_000 * 100_000 / 1_000_000;
const SELL_NETWORK_FEE: u64 = 5_000 + 150_000 * 100_000 / 1_000_000;
const PANIC_SELL_NETWORK_FEE: u64 = 5_000 + 150_000 * 1_000_000 / 1_000_000;

// ───────────────────────────── history ─────────────────────────────

/// One Pump.fun trade, decoded from its TradeEvent
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Trade {
    slot: u64,
    ts: i64,
    mint: String,
    sol: u64,
    tokens: u64,
    is_buy: bool,
    user: String,
    fee_payer: String,
    vsol: u64,
    vtok: u64,
    rsol: u64,
    rtok: u64,
    creator: String,
}

impl Trade {
    fn curve(&self) -> BondingCurveAccountPod {
        BondingCurveAccountPod {
            virtual_token_reserves: self.vtok,
            virtual_sol_reserves: self.vsol,
            real_token_reserves: self.rtok,
            real_sol_reserves: self.rsol,
            token_total_supply: 1_000_000_000_000_000,
            complete: self.rtok == 0,
            creator: Pubkey::from_str(&self.creator).unwrap_or_default(),
        }
    }
}

/// signature -> trades in that transaction (empty if none or it failed), so reruns only fetch new ones
#[derive(Default, Serialize, Deserialize)]
struct TxCache {
    txs: HashMap<String, Vec<Trade>>,
}

#[derive(Default, Serialize, Deserialize)]
struct MintCache {
    #[serde(flatten)]
    txs: TxCache,
    /// Tokens the creator bought in the create transaction (0 = unknown)
    dev_initial: u64,
    creation_sig: Option<String>,
    /// Window signatures, oldest first, as of the last online run: offline replays keep the exact
    /// on-chain order of trades within a slot
    #[serde(default)]
    order: Vec<String>,
}

fn cache_path(kind: &str, key: &str) -> PathBuf {
    Path::new(CACHE_DIR).join(kind).join(format!("{key}.json"))
}

fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save<T: Serialize>(path: &Path, value: &T) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string(value) {
        let _ = std::fs::write(path, s);
    }
}

fn read_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn read_key(b: &[u8], at: usize) -> Option<String> {
    Some(Pubkey::new_from_array(b.get(at..at + 32)?.try_into().ok()?).to_string())
}

/// All Pump.fun TradeEvents emitted by a transaction (`encoding: json`)
fn parse_trades(tx: &Value) -> Vec<Trade> {
    let meta = &tx["meta"];
    if !meta["err"].is_null() {
        return Vec::new();
    }
    let keys: Vec<&str> = tx["transaction"]["message"]["accountKeys"].as_array().into_iter().flatten()
        .chain(meta["loadedAddresses"]["writable"].as_array().into_iter().flatten())
        .chain(meta["loadedAddresses"]["readonly"].as_array().into_iter().flatten())
        .filter_map(|k| k.as_str())
        .collect();
    let Some(&fee_payer) = keys.first() else { return Vec::new() };
    let slot = tx["slot"].as_u64().unwrap_or(0);
    let mut out = Vec::new();
    for group in meta["innerInstructions"].as_array().into_iter().flatten() {
        for ix in group["instructions"].as_array().into_iter().flatten() {
            let program = ix["programIdIndex"].as_u64().and_then(|i| keys.get(i as usize));
            if program != Some(&PUMPFUN_PROGRAM) {
                continue;
            }
            let Some(data) = ix["data"].as_str().and_then(|d| bs58::decode(d).into_vec().ok()) else { continue };
            if data.len() < 16 || data[..8] != EVENT_IX_TAG || data[8..16] != TRADE_EVENT_DISC {
                continue;
            }
            let e = &data[16..];
            // mint, sol, tokens, is_buy, user, ts, vsol, vtok, rsol, rtok, fee_recipient, fee_bps, fee, creator
            let parsed = (|| {
                Some(Trade {
                    slot,
                    mint: read_key(e, 0)?,
                    sol: read_u64(e, 32)?,
                    tokens: read_u64(e, 40)?,
                    is_buy: *e.get(48)? != 0,
                    user: read_key(e, 49)?,
                    ts: read_u64(e, 81)? as i64,
                    vsol: read_u64(e, 89)?,
                    vtok: read_u64(e, 97)?,
                    rsol: read_u64(e, 105)?,
                    rtok: read_u64(e, 113)?,
                    creator: read_key(e, 169)?,
                    fee_payer: fee_payer.to_string(),
                })
            })();
            out.extend(parsed);
        }
    }
    out
}

struct Fetcher {
    client: reqwest::Client,
    url: String,
    parallel: usize,
    failed: AtomicUsize,
    /// Why fetches failed, for the end-of-phase report
    failures: std::sync::Mutex<BTreeMap<String, usize>>,
}

impl Fetcher {
    fn redact(&self, s: impl ToString) -> String {
        s.to_string().replace(&self.url, "<rpc-url>")
    }

    fn note_failure<T>(&self, reason: &str) -> Option<T> {
        let reason: String = self.redact(reason).chars().take(120).collect();
        *self.failures.lock().unwrap().entry(reason).or_default() += 1;
        None
    }

    /// Signatures for `address`, newest first, stopping once older than `since_ts` (plus
    /// `extra_pages` more pages when we also want the oldest one) or after `max` entries.
    /// Returns (sig, block_time, ok) and whether the address's very first signature was reached.
    async fn signatures(&self, address: &str, since_ts: i64, max: usize, extra_pages: usize) -> (Vec<(String, i64, bool)>, bool) {
        let mut out = Vec::new();
        let mut before: Option<String> = None;
        let mut extra = 0;
        loop {
            let mut opts = json!({"limit": 1000});
            if let Some(b) = &before {
                opts["before"] = json!(b);
            }
            let page = match rpc(&self.client, &self.url, "getSignaturesForAddress", json!([address, opts])).await {
                Ok(Value::Array(p)) => p,
                Ok(_) => break,
                Err(e) => {
                    eprintln!("   ⚠️ signatures for {}: {}", address, self.redact(e));
                    self.failed.fetch_add(1, Ordering::Relaxed);
                    return (out, false);
                }
            };
            let full = page.len() == 1000;
            let mut older = false;
            for s in &page {
                let (Some(sig), ts) = (s["signature"].as_str(), s["blockTime"].as_i64().unwrap_or(0)) else { continue };
                older |= ts < since_ts;
                out.push((sig.to_string(), ts, s["err"].is_null()));
            }
            before = out.last().map(|s| s.0.clone());
            if !full {
                return (out, true);
            }
            if out.len() >= max {
                return (out, false);
            }
            if older {
                extra += 1;
                if extra > extra_pages {
                    return (out, false);
                }
            }
        }
        (out, false)
    }

    /// getTransaction for each signature in one batched request, retried on transport errors
    async fn fetch_batch(&self, sigs: &[&String]) -> Vec<(String, Option<Vec<Trade>>)> {
        let body: Vec<Value> = sigs.iter().enumerate()
            .map(|(i, sig)| json!({"jsonrpc": "2.0", "id": i, "method": "getTransaction",
                "params": [sig, {"encoding": "json", "maxSupportedTransactionVersion": 1}]}))
            .collect();
        let mut last_err = String::new();
        for attempt in 0..5u64 {
            let resp = match self.client.post(&self.url).json(&body).send().await {
                Ok(r) if r.status().as_u16() == 429 => {
                    last_err = "HTTP 429".into();
                    tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
                    continue;
                }
                Ok(r) => r.json::<Vec<Value>>().await,
                Err(e) => {
                    last_err = format!("{e}");
                    tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
                    continue;
                }
            };
            let Ok(replies) = resp else {
                last_err = "invalid batch reply".into();
                continue;
            };
            let mut by_id: HashMap<u64, Value> = replies.into_iter().filter_map(|r| Some((r["id"].as_u64()?, r))).collect();
            return sigs.iter().enumerate().map(|(i, sig)| {
                let reply = by_id.remove(&(i as u64)).unwrap_or_default();
                let trades = match (&reply["result"], &reply["error"]) {
                    (tx, _) if tx.is_object() => Some(parse_trades(tx)),
                    (_, err) if !err.is_null() => self.note_failure(&format!("getTransaction error: {err}")),
                    _ => self.note_failure("transaction not found"),
                };
                (sig.to_string(), trades)
            }).collect();
        }
        sigs.iter().map(|sig| (sig.to_string(), self.note_failure(&last_err))).collect()
    }

    /// Fill `cache` with the trades of every signature it doesn't have yet
    async fn fetch_txs(&self, sigs: &[String], cache: &mut TxCache, concurrency: usize) {
        let missing: Vec<&String> = sigs.iter().filter(|s| !cache.txs.contains_key(*s)).collect();
        // JSON-RPC batches: one HTTP request carries BATCH getTransaction calls (~3x the throughput)
        let results: Vec<(String, Option<Vec<Trade>>)> = stream::iter(missing.chunks(BATCH))
            .map(|chunk| self.fetch_batch(chunk))
            .buffer_unordered(concurrency.div_ceil(4).max(1))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .flatten()
            .collect();
        for (sig, trades) in results {
            match trades {
                Some(t) => {
                    cache.txs.insert(sig, t);
                }
                // Not cached, so the next run retries it
                None => {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

/// A copied wallet's Pump.fun trades over the period (from its own transaction history)
async fn wallet_trades(f: &Fetcher, wallet: &str, since_ts: i64, until_ts: i64, max_txs: usize, offline: bool) -> (Vec<Trade>, i64) {
    let path = cache_path("wallets", wallet);
    let mut cache: TxCache = load(&path);
    let (sigs, reached_start) = if offline {
        (Vec::new(), true)
    } else {
        f.signatures(wallet, since_ts, max_txs, 0).await
    };
    let wanted: Vec<String> = sigs.iter().filter(|s| s.2 && s.1 >= since_ts && s.1 <= until_ts).map(|s| s.0.clone()).collect();
    // Only when the tx cap stopped paging before reaching the start of the period
    let covered_from = match sigs.last() {
        Some(oldest) if !reached_start && oldest.1 >= since_ts => oldest.1,
        _ => since_ts,
    };
    f.fetch_txs(&wanted, &mut cache, f.parallel).await;
    save(&path, &cache);
    let trades = cache.txs.values().flatten()
        .filter(|t| t.ts >= since_ts && t.ts <= until_ts && (t.user == wallet || t.fee_payer == wallet))
        .cloned()
        .collect();
    (trades, covered_from)
}

struct MintHistory {
    mint: String,
    trades: Vec<Trade>,
    dev_initial: u64,
    truncated: bool,
}

/// Every trade on `mint`'s bonding curve between `from_ts` and `to_ts`, oldest first
async fn mint_history(f: &Fetcher, mint: &str, from_ts: i64, to_ts: i64, max_txs: usize, offline: bool) -> MintHistory {
    let path = cache_path("mints", mint);
    let mut cache: MintCache = load(&path);
    let curve = derive_bonding_curve(&Pubkey::from_str(mint).unwrap_or_default()).0.to_string();
    let mut truncated = false;
    let mut window: Vec<(String, i64)> = Vec::new();
    if offline {
        let trades = cache.order.iter()
            .filter_map(|sig| cache.txs.txs.get(sig))
            .flatten()
            .filter(|t| t.mint == mint && t.ts >= from_ts && t.ts <= to_ts)
            .cloned()
            .collect();
        return MintHistory { mint: mint.to_string(), trades, dev_initial: cache.dev_initial, truncated };
    }
    // Newest first; a few pages past the window find the create tx for young tokens
    let (sigs, reached_start) = f.signatures(&curve, from_ts, 50_000, 3).await;
    for (sig, ts, ok) in sigs.iter().rev() {
        if *ok && *ts >= from_ts && *ts <= to_ts {
            window.push((sig.clone(), *ts));
        }
    }
    if window.len() > max_txs {
        window.truncate(max_txs);
        truncated = true;
    }
    let mut wanted: Vec<String> = window.iter().map(|w| w.0.clone()).collect();
    if reached_start && cache.creation_sig.is_none() {
        if let Some((sig, _, _)) = sigs.last() {
            cache.creation_sig = Some(sig.clone());
            wanted.push(sig.clone());
        }
    }
    f.fetch_txs(&wanted, &mut cache.txs, f.parallel).await;
    if let Some(sig) = &cache.creation_sig {
        if let Some(trades) = cache.txs.txs.get(sig) {
            cache.dev_initial = trades.iter().filter(|t| t.mint == mint && t.is_buy && t.user == t.creator).map(|t| t.tokens).sum();
        }
    }
    cache.order = window.iter().map(|w| w.0.clone()).collect();
    save(&path, &cache);
    // Signature order is chronological within a slot; keep it
    let trades = window.iter()
        .filter_map(|(sig, _)| cache.txs.txs.get(sig))
        .flatten()
        .filter(|t| t.mint == mint)
        .cloned()
        .collect();
    MintHistory { mint: mint.to_string(), trades, dev_initial: cache.dev_initial, truncated }
}

// ───────────────────────────── replay ─────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Params {
    trailing: f64,
    hard_stop: f64,
    take_profit: f64,
    dev_dump: f64,
    leader_exit: bool,
    latency_slots: u64,
    /// Sell after holding this long (0 = never; the live bot has no time exit)
    max_hold_secs: i64,
}

#[derive(Clone)]
struct Settings {
    copy_lamports: u64,
    max_position_lamports: u64,
    copy_slippage_bps: u64,
    sell_slippage_bps: u64,
    min_whale_lamports: u64,
    targets: HashSet<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SimTrade {
    mint: String,
    leader: String,
    entry_ts: i64,
    exit_ts: Option<i64>,
    exit: String,
    spent_lamports: u64,
    received_lamports: u64,
    pnl_lamports: i64,
}

#[derive(Default)]
struct SimResult {
    trades: Vec<SimTrade>,
    signals: usize,
    failed_slippage: usize,
    skipped_capital: usize,
    skipped_graduated: usize,
}

impl SimResult {
    fn closed(&self) -> impl Iterator<Item = &SimTrade> {
        self.trades.iter()
    }
    fn pnl(&self) -> i64 {
        self.closed().map(|t| t.pnl_lamports).sum()
    }
    fn wins(&self) -> usize {
        self.closed().filter(|t| t.pnl_lamports > 0).count()
    }
    fn profit_factor(&self) -> f64 {
        let won: i64 = self.closed().filter(|t| t.pnl_lamports > 0).map(|t| t.pnl_lamports).sum();
        let lost: i64 = -self.closed().filter(|t| t.pnl_lamports < 0).map(|t| t.pnl_lamports).sum::<i64>();
        if lost == 0 { if won > 0 { 99.0 } else { 0.0 } } else { won as f64 / lost as f64 }
    }
    /// Worst peak-to-trough fall of cumulative P&L, by exit time
    fn max_drawdown(&self) -> i64 {
        let mut by_exit: Vec<&SimTrade> = self.closed().collect();
        by_exit.sort_by_key(|t| t.exit_ts.unwrap_or(i64::MAX));
        let (mut cum, mut peak, mut dd) = (0i64, 0i64, 0i64);
        for t in by_exit {
            cum += t.pnl_lamports;
            peak = peak.max(cum);
            dd = dd.max(peak - cum);
        }
        dd
    }
}

enum Order {
    Buy { tokens: u64, max_cost: u64, leader: String, signal_ts: i64 },
    Sell { reason: String, panic: bool },
}

struct Pending {
    fill_slot: u64,
    mint: usize,
    order: Order,
}

struct Open {
    leader: String,
    entry_ts: i64,
    tokens: u64,
    spent: u64,
}

/// Replay one parameter set. `entries` limits which leader buys may be copied (by timestamp)
async fn simulate(hist: &[MintHistory], timeline: &[(u64, usize, usize)], s: &Settings, tips: &TipEngine, p: Params, entries: (i64, i64)) -> SimResult {
    let pm = PositionManager::new(p.trailing, p.hard_stop, p.take_profit, p.dev_dump);
    let mints: Vec<Pubkey> = hist.iter().map(|h| Pubkey::from_str(&h.mint).unwrap_or_default()).collect();
    let curves: Vec<Pubkey> = mints.iter().map(|m| derive_bonding_curve(m).0).collect();
    let mut state: Vec<Option<BondingCurveAccountPod>> = vec![None; hist.len()];
    let mut dev_balance: Vec<u64> = hist.iter().map(|h| h.dev_initial).collect();
    let mut last_ts: Vec<i64> = vec![0; hist.len()];
    // Tokens whose fetched tape has ended (nothing to price a new position with)
    let mut finished: Vec<bool> = vec![false; hist.len()];
    let mut open: HashMap<usize, Open> = HashMap::new();
    let mut exiting: HashSet<usize> = HashSet::new();
    let mut pending: Vec<Pending> = Vec::new();
    let mut r = SimResult::default();

    for &(slot, mi, ti) in timeline {
        // 1. Our orders land `latency_slots` after the trigger, ahead of that slot's other trades
        if pending.iter().any(|o| o.fill_slot <= slot) {
            let (due, rest): (Vec<Pending>, Vec<Pending>) = pending.into_iter().partition(|o| o.fill_slot <= slot);
            pending = rest;
            for o in due {
                if finished[o.mint] && matches!(o.order, Order::Buy { .. }) {
                    pm.release(&mints[o.mint]).await;
                    continue;
                }
                fill(o, &mut r, &pm, &mut open, &state, &dev_balance, &last_ts, &mints, &curves, s, tips).await;
            }
        }

        // 2. Apply the historical trade
        let t = &hist[mi].trades[ti];
        let curve = t.curve();
        state[mi] = Some(curve);
        last_ts[mi] = t.ts;
        if t.user == t.creator {
            dev_balance[mi] = if t.is_buy { dev_balance[mi] + t.tokens } else { dev_balance[mi].saturating_sub(t.tokens) };
        }

        // 3. Exits for a position we hold (stream: curve account update, then sell detection)
        exiting.retain(|m| open.contains_key(m));
        if open.contains_key(&mi) {
            let mint = &mints[mi];
            let mut exit = if curve.complete {
                None
            } else {
                pm.on_price_update(&curves[mi], curve.current_spot_price_sol()).await.map(|(_, reason)| {
                    let panic = matches!(reason, ExitReason::DevDumpPanic | ExitReason::HardStopLoss | ExitReason::TrailingStop);
                    (format!("{reason:?}"), panic)
                })
            };
            if exit.is_none() && !t.is_buy {
                for who in [&t.user, &t.fee_payer] {
                    let Ok(seller) = Pubkey::from_str(who) else { continue };
                    if pm.check_dev_dump(mint, &seller, t.tokens).await.is_some() {
                        exit = Some(("DevDump".into(), true));
                        break;
                    }
                    if p.leader_exit && pm.check_leader_exit(mint, &seller).await.is_some() {
                        exit = Some(("LeaderSold".into(), true));
                        break;
                    }
                }
            }
            if exit.is_none() && p.max_hold_secs > 0 && !exiting.contains(&mi) && open.get(&mi).map_or(false, |o| t.ts - o.entry_ts >= p.max_hold_secs) {
                exit = Some(("MaxHold".into(), false));
            }
            if let Some((reason, panic)) = exit.filter(|_| exiting.insert(mi)) {
                pending.push(Pending { fill_slot: slot + p.latency_slots, mint: mi, order: Order::Sell { reason, panic } });
            }
        }

        // The fetched tape for this token ends here; value what we hold instead of letting it
        // sit "open" (and block capital) for the rest of the replay
        finished[mi] = ti + 1 == hist[mi].trades.len();
        if finished[mi] && open.contains_key(&mi) {
            close(mi, "Window end (marked)".into(), false, &mut r, &pm, &mut open, &state, &last_ts, &mints, s, tips).await;
        }

        // 4. A copied wallet buying is a signal
        if t.is_buy && t.sol >= s.min_whale_lamports && t.ts >= entries.0 && t.ts < entries.1 && !curve.complete {
            let leader = if s.targets.contains(&t.user) { Some(&t.user) } else if s.targets.contains(&t.fee_payer) { Some(&t.fee_payer) } else { None };
            // Like the bot: never copy a creator buying its own token
            let leader = leader.filter(|l| **l != t.creator);
            if let Some(leader) = leader {
                r.signals += 1;
                match pm.try_reserve(&mints[mi], s.copy_lamports, s.max_position_lamports).await {
                    Reservation::Reserved => {
                        if let Some(calc) = curve.calculate_buy_output(s.copy_lamports, s.copy_slippage_bps) {
                            pending.push(Pending {
                                fill_slot: slot + p.latency_slots,
                                mint: mi,
                                order: Order::Buy { tokens: calc.tokens_out, max_cost: calc.max_sol_cost, leader: leader.clone(), signal_ts: t.ts },
                            });
                        } else {
                            pm.release(&mints[mi]).await;
                        }
                    }
                    Reservation::AlreadyActive => {}
                    Reservation::CapitalLimit { .. } => r.skipped_capital += 1,
                }
            }
        }
    }

    // Orders still in flight at the end never landed; whatever is open is marked to market
    for o in pending {
        if let Order::Sell { reason, panic } = o.order {
            close(o.mint, reason, panic, &mut r, &pm, &mut open, &state, &last_ts, &mints, s, tips).await;
        } else {
            pm.release(&mints[o.mint]).await;
        }
    }
    let still_open: Vec<usize> = open.keys().copied().collect();
    for mi in still_open {
        let graduated = state[mi].map_or(false, |c| c.complete);
        let label = if graduated { "Graduated (marked)" } else { "Open at end (marked)" };
        close(mi, label.into(), false, &mut r, &pm, &mut open, &state, &last_ts, &mints, s, tips).await;
    }
    r
}

#[allow(clippy::too_many_arguments)]
async fn fill(
    o: Pending, r: &mut SimResult, pm: &PositionManager, open: &mut HashMap<usize, Open>,
    state: &[Option<BondingCurveAccountPod>], dev_balance: &[u64], last_ts: &[i64],
    mints: &[Pubkey], curves: &[Pubkey], s: &Settings, tips: &TipEngine,
) {
    let mi = o.mint;
    match o.order {
        Order::Buy { tokens, max_cost, leader, signal_ts } => {
            let Some(curve) = state[mi].filter(|c| !c.complete) else {
                r.skipped_graduated += 1;
                pm.release(&mints[mi]).await;
                return;
            };
            // Pump.fun buys exactly `tokens`; it fails if they now cost more than max_sol_cost
            let mut after = curve;
            let Some(curve_cost) = after.apply_buy(tokens) else {
                pm.release(&mints[mi]).await;
                return;
            };
            let cost = curve_cost + curve_cost * PUMP_FEE_BPS / 10_000;
            if cost > max_cost {
                r.failed_slippage += 1;
                pm.release(&mints[mi]).await;
                return;
            }
            let tip = tips.trade_tip(s.copy_lamports);
            let entry_price_sol = (cost as f64 / 1e9) / (tokens as f64 / 1e6);
            pm.add_position(Position {
                mint: mints[mi],
                bonding_curve: curves[mi],
                entry_price_sol,
                highest_price_sol: entry_price_sol,
                token_balance: tokens,
                sol_invested_lamports: cost,
                dev_wallet: Some(curve.creator),
                dev_initial_balance: dev_balance[mi],
                token_program: Pubkey::default(),
                is_selling: false,
                leaders: Pubkey::from_str(&leader).into_iter().collect(),
            })
            .await;
            open.insert(mi, Open { leader, entry_ts: signal_ts.max(last_ts[mi]), tokens, spent: cost + tip + BUY_NETWORK_FEE });
        }
        Order::Sell { reason, panic } => {
            close(mi, reason, panic, r, pm, open, state, last_ts, mints, s, tips).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn close(
    mi: usize, reason: String, panic: bool, r: &mut SimResult, pm: &PositionManager, open: &mut HashMap<usize, Open>,
    state: &[Option<BondingCurveAccountPod>], last_ts: &[i64], mints: &[Pubkey], s: &Settings, tips: &TipEngine,
) {
    let Some(pos) = open.remove(&mi) else { return };
    pm.remove_position(&mints[mi]).await;
    let sol_out = state[mi]
        .and_then(|c| c.calculate_sell_output(pos.tokens, s.sell_slippage_bps))
        .map(|c| c.sol_out)
        .unwrap_or(0);
    // A graduated curve can't be sold into; the bag moves to PumpSwap at roughly this value
    let reason = if state[mi].map_or(false, |c| c.complete) && !reason.ends_with("(marked)") { "Graduated (marked)".to_string() } else { reason };
    let marked = reason.ends_with("(marked)");
    let (tip, net_fee) = match (marked, panic) {
        (true, _) => (0, 0),
        (false, true) => (tips.panic_tip(sol_out), PANIC_SELL_NETWORK_FEE),
        (false, false) => (tips.trade_tip(sol_out), SELL_NETWORK_FEE),
    };
    let received = sol_out.saturating_sub(sol_out * PUMP_FEE_BPS / 10_000 + tip + net_fee);
    r.trades.push(SimTrade {
        mint: mints[mi].to_string(),
        leader: pos.leader,
        entry_ts: pos.entry_ts,
        exit_ts: Some(last_ts[mi]),
        exit: reason,
        spent_lamports: pos.spent,
        received_lamports: received,
        pnl_lamports: received as i64 - pos.spent as i64,
    });
}

// ───────────────────────────── main ─────────────────────────────

fn arg<T: FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn sol(l: i64) -> f64 {
    l as f64 / 1e9
}

fn print_summary(label: &str, r: &SimResult) {
    let n = r.trades.len();
    let spent: u64 = r.trades.iter().map(|t| t.spent_lamports).sum();
    println!("{label}");
    println!("   Copy signals:          {} ({} filled, {} failed on slippage, {} skipped: capital limit, {} skipped: graduated)",
        r.signals, n, r.failed_slippage, r.skipped_capital, r.skipped_graduated);
    println!("   Win rate:              {}/{} = {:.1}%", r.wins(), n, if n > 0 { 100.0 * r.wins() as f64 / n as f64 } else { 0.0 });
    println!("   Profit factor:         {:.2}", r.profit_factor());
    println!("   P&L:                   {:+.4} SOL on {:.4} SOL deployed ({:+.1}%)",
        sol(r.pnl()), sol(spent as i64), if spent > 0 { 100.0 * r.pnl() as f64 / spent as f64 } else { 0.0 });
    println!("   Max drawdown:          {:.4} SOL", sol(r.max_drawdown()));
    let mut reasons: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
    for t in &r.trades {
        let e = reasons.entry(t.exit.as_str()).or_default();
        e.0 += 1;
        e.1 += t.pnl_lamports;
    }
    for (reason, (count, pnl)) in reasons {
        println!("     exit {:<22} {:>4} trades  {:+.4} SOL", reason, count, sol(pnl));
    }
}

/// Each wallet's own realized results: per token it bought in the period, buy cost vs sell
/// proceeds (Pump.fun fee included). A token counts as closed once 95% of what was bought is sold.
fn print_leader_stats(per_wallet: &[(String, Vec<Trade>, i64)], since: i64, min_win_rate: f64, min_closed: usize) {
    #[derive(Serialize)]
    struct Stats { wallet: String, closed: usize, wins: usize, win_rate: f64, pnl_sol: f64, roi: f64, profit_factor: f64, open: usize, history_from: i64, passes: bool }
    let fee = |l: u64| l * PUMP_FEE_BPS / 10_000;
    let mut rows: Vec<Stats> = per_wallet.iter().map(|(w, trades, covered_from)| {
        let mut by_mint: HashMap<&str, (u64, u64, u64, u64)> = HashMap::new(); // cost, proceeds, bought, sold
        let mut trades: Vec<&Trade> = trades.iter().collect();
        trades.sort_by_key(|t| (t.slot, t.ts));
        for t in trades {
            if t.is_buy {
                let e = by_mint.entry(t.mint.as_str()).or_default();
                e.0 += t.sol + fee(t.sol);
                e.2 += t.tokens;
            } else if let Some(e) = by_mint.get_mut(t.mint.as_str()) {
                // Sells of tokens bought before the period are ignored
                e.1 += t.sol - fee(t.sol);
                e.3 += t.tokens;
            }
        }
        let (mut closed, mut wins, mut open, mut pnl, mut gain, mut loss, mut cost) = (0, 0, 0, 0i64, 0i64, 0i64, 0i64);
        for (c, p, bought, sold) in by_mint.values() {
            if *sold * 100 < *bought * 95 {
                open += 1;
                continue;
            }
            closed += 1;
            let r = *p as i64 - *c as i64;
            pnl += r;
            cost += *c as i64;
            if r > 0 { wins += 1; gain += r } else { loss -= r }
        }
        let win_rate = if closed > 0 { wins as f64 / closed as f64 } else { 0.0 };
        let profit_factor = if loss > 0 { gain as f64 / loss as f64 } else if gain > 0 { f64::INFINITY } else { 0.0 };
        let full_history = *covered_from <= since + 3600;
        Stats {
            wallet: w.clone(), closed, wins, win_rate, pnl_sol: sol(pnl), roi: if cost > 0 { pnl as f64 / cost as f64 } else { 0.0 },
            profit_factor, open, history_from: *covered_from,
            passes: win_rate > min_win_rate && closed >= min_closed && pnl > 0 && full_history,
        }
    }).collect();
    rows.sort_by(|a, b| b.passes.cmp(&a.passes).then(b.win_rate.total_cmp(&a.win_rate)));
    println!("\n📊 Copied wallets' own results since {} (pass = win rate > {:.0}%, ≥ {} closed tokens, in profit, full history)",
        fmt_time(Some(since)), min_win_rate * 100.0, min_closed);
    println!("   {:<44} {:>6} {:>6} {:>9} {:>8} {:>6} {:>5}  {}", "wallet", "closed", "win%", "P&L SOL", "ROI", "PF", "open", "");
    for r in &rows {
        let note = if r.history_from > since + 3600 { format!("history only from {}", fmt_time(Some(r.history_from))) } else { String::new() };
        println!("   {:<44} {:>6} {:>5.1}% {:>+9.2} {:>+7.1}% {:>6.2} {:>5}  {} {}", r.wallet, r.closed, r.win_rate * 100.0, r.pnl_sol,
            r.roi * 100.0, r.profit_factor.min(99.0), r.open, if r.passes { "✅ PASS" } else { "❌" }, note);
    }
    let passing: Vec<&str> = rows.iter().filter(|r| r.passes).map(|r| r.wallet.as_str()).collect();
    println!("\n   {} of {} wallets pass", passing.len(), rows.len());
    save(Path::new("data/leader_stats.json"), &rows);
    let _ = std::fs::write("data/leader_stats_passing.txt", passing.join(","));
    println!("   Wrote data/leader_stats.json and data/leader_stats_passing.txt (comma-separated, for TARGET_WALLETS)");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let config = BotConfig::load_from_env()?;
    let days: f64 = arg(&args, "--days", 7.0);
    let hold_hours: f64 = arg(&args, "--hold-hours", 6.0);
    let parallel: usize = arg(&args, "--parallel", 8);
    let max_wallet_txs: usize = arg(&args, "--max-wallet-txs", 5000);
    let max_mint_txs: usize = arg(&args, "--max-mint-txs", 4000);
    let max_tokens_per_wallet: usize = arg(&args, "--max-tokens-per-wallet", usize::MAX);
    let latency: u64 = arg(&args, "--latency-slots", 2);
    let offline = args.iter().any(|a| a == "--offline");
    let wallets: Vec<String> = match args.iter().position(|a| a == "--wallets").and_then(|i| args.get(i + 1)) {
        Some(list) => list.split(',').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect(),
        None => config.target_wallets.clone(),
    };
    anyhow::ensure!(!wallets.is_empty(), "no wallets: set TARGET_WALLETS or pass --wallets");

    // --end (unix seconds) replays a fixed past period, e.g. one already in the cache with --offline
    let now: i64 = arg(&args, "--end", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64);
    let since = now - (days * 86_400.0) as i64;
    let hold = (hold_hours * 3600.0) as i64;
    let f = Fetcher {
        client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?,
        url: config.solana_rpc_url.clone(),
        parallel,
        failed: AtomicUsize::new(0),
        failures: Default::default(),
    };

    println!("🧪 Copy-trading backtest: {} wallets, last {:.1} days ({} → now){}", wallets.len(), days, fmt_time(Some(since)), if offline { ", cache only" } else { "" });

    // Phase 1: what the copied wallets bought
    println!("📥 Phase 1: copied wallets' trades...");
    let min_whale_lamports = (config.min_whale_buy_sol * 1e9) as u64;
    let mut all_buys: HashMap<String, (i64, i64)> = HashMap::new();
    let mut sampled: HashSet<String> = HashSet::new();
    let leader_stats_mode = args.iter().any(|a| a == "--leader-stats");
    let mut per_wallet: Vec<(String, Vec<Trade>, i64)> = Vec::new();
    for (i, w) in wallets.iter().enumerate() {
        let (trades, covered_from) = wallet_trades(&f, w, since, now, max_wallet_txs, offline).await;
        if leader_stats_mode {
            per_wallet.push((w.clone(), trades.clone(), covered_from));
        }
        let buys: Vec<&Trade> = trades.iter().filter(|t| t.is_buy).collect();
        for b in &buys {
            let e = all_buys.entry(b.mint.clone()).or_insert((b.ts, b.ts));
            e.0 = e.0.min(b.ts);
            e.1 = e.1.max(b.ts);
        }
        // Busy wallets buy thousands of tokens; a fixed pseudo-random sample of them keeps the
        // run short without favouring any kind of token (same sample on every rerun)
        let mut copyable: Vec<&str> = buys.iter().filter(|b| b.sol >= min_whale_lamports).map(|b| b.mint.as_str()).collect();
        copyable.sort_by_key(|m| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (m, w.as_str()).hash(&mut h);
            h.finish()
        });
        copyable.dedup();
        let take = copyable.len().min(max_tokens_per_wallet);
        sampled.extend(copyable[..take].iter().map(|m| m.to_string()));
        let partial = if covered_from > since + 3600 { format!(" (history only back to {})", fmt_time(Some(covered_from))) } else { String::new() };
        let sample = if take < copyable.len() { format!(", sampling {take} of {} tokens", copyable.len()) } else { String::new() };
        println!("   [{:>2}/{}] {}…  {:>4} buys, {:>4} sells{}{}", i + 1, wallets.len(), &w[..8.min(w.len())],
            buys.len(), trades.len() - buys.len(), partial, sample);
    }
    if leader_stats_mode {
        print_leader_stats(&per_wallet, since, arg(&args, "--min-win-rate", 0.7), arg(&args, "--min-closed", 10usize));
        return Ok(());
    }
    let buys_by_mint: HashMap<String, (i64, i64)> = all_buys.into_iter().filter(|(m, _)| sampled.contains(m)).collect();
    println!("   {} tokens to replay", buys_by_mint.len());

    // Phase 2: each token's full trade tape around those buys
    println!("📥 Phase 2: price history of those tokens (window: first buy → last buy + {:.0}h)...", hold_hours);
    let done = AtomicUsize::new(0);
    let total = buys_by_mint.len();
    let mut hist: Vec<MintHistory> = stream::iter(buys_by_mint.iter())
        .map(|(mint, (first, last))| {
            let (f, done) = (&f, &done);
            async move {
                let h = mint_history(f, mint, first - 5, (last + hold).min(now), max_mint_txs, offline).await;
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 25 == 0 || n == total {
                    println!("   {n}/{total} tokens");
                }
                h
            }
        })
        .buffer_unordered(3)
        .collect()
        .await;
    hist.retain(|h| !h.trades.is_empty());
    hist.sort_by(|a, b| a.mint.cmp(&b.mint));
    let events: usize = hist.iter().map(|h| h.trades.len()).sum();
    let truncated = hist.iter().filter(|h| h.truncated).count();
    let failed = f.failed.load(Ordering::Relaxed);
    println!("   {} tokens, {} trades{}{}", hist.len(), events,
        if truncated > 0 { format!(", {truncated} tokens capped at {max_mint_txs} trades") } else { String::new() },
        if failed > 0 { format!(", {failed} RPC fetches failed (rerun to retry)") } else { String::new() });

    for (reason, n) in f.failures.lock().unwrap().iter() {
        println!("      {n:>6} × {reason}");
    }

    let mut timeline: Vec<(u64, usize, usize)> = hist.iter().enumerate()
        .flat_map(|(mi, h)| h.trades.iter().enumerate().map(move |(ti, t)| (t.slot, mi, ti)))
        .collect();
    timeline.sort();

    let copy_sol: f64 = arg(&args, "--copy-sol", config.copy_trade_amount_sol);
    let max_position_sol: f64 = arg(&args, "--max-position-sol", config.max_position_sol);
    let settings = Settings {
        copy_lamports: (copy_sol * 1e9) as u64,
        max_position_lamports: (max_position_sol * 1e9) as u64,
        copy_slippage_bps: config.slippage_bps.max(1500),
        sell_slippage_bps: config.slippage_bps,
        min_whale_lamports: (config.min_whale_buy_sol * 1e9) as u64,
        targets: wallets.iter().cloned().collect(),
    };
    let tips = TipEngine::new(config.min_tip_lamports, config.max_tip_lamports, config.tip_aggression_factor)
        .with_trade_caps(arg(&args, "--tip-pct", config.tip_max_pct_of_trade), arg(&args, "--panic-tip-pct", config.tip_max_panic_pct_of_trade));
    let base = Params {
        trailing: arg(&args, "--trailing", config.trailing_stop_pct),
        hard_stop: arg(&args, "--hard-stop", config.hard_stop_loss_pct),
        take_profit: arg(&args, "--take-profit", config.take_profit_pct),
        dev_dump: config.dev_dump_threshold_pct,
        leader_exit: !args.iter().any(|a| a == "--no-leader-exit"),
        latency_slots: latency,
        max_hold_secs: (arg(&args, "--max-hold-mins", 0.0f64) * 60.0) as i64,
    };

    println!("\n🔁 Replaying: {:.4} SOL per copy (max {:.2} SOL open), trailing {:.0}%, hard stop {:.0}%, take profit {:.2}x, leader exit {}, max hold {}, latency {} slots",
        settings.copy_lamports as f64 / 1e9, max_position_sol, base.trailing * 100.0, base.hard_stop * 100.0, base.take_profit,
        if base.leader_exit { "on" } else { "off" },
        if base.max_hold_secs > 0 { format!("{}m", base.max_hold_secs / 60) } else { "off".into() }, latency);
    let r = simulate(&hist, &timeline, &settings, &tips, base, (since, now)).await;
    println!();
    print_summary("==================== RESULT (current settings) ====================", &r);

    // Per copied wallet
    let mut per: BTreeMap<&str, Vec<&SimTrade>> = BTreeMap::new();
    for t in &r.trades {
        per.entry(t.leader.as_str()).or_default().push(t);
    }
    let mut rows: Vec<(&str, usize, usize, i64)> = per.iter()
        .map(|(w, ts)| (*w, ts.len(), ts.iter().filter(|t| t.pnl_lamports > 0).count(), ts.iter().map(|t| t.pnl_lamports).sum()))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.3));
    println!("\n================ PER COPIED WALLET (our copies, not their trades) ================");
    println!("{:<46} {:>6} {:>6} {:>11}", "WALLET", "TRADES", "WIN%", "OUR PNL SOL");
    for (w, n, wins, pnl) in &rows {
        println!("{:<46} {:>6} {:>5.0}% {:>+11.4}", w, n, 100.0 * *wins as f64 / *n as f64, sol(*pnl));
    }

    // Robustness: latency and leader exit
    println!("\n================ SENSITIVITY ================");
    for (label, p) in [
        ("latency 1 slot", Params { latency_slots: 1, ..base }),
        ("latency 4 slots", Params { latency_slots: 4, ..base }),
        ("latency 8 slots", Params { latency_slots: 8, ..base }),
        ("leader exit off", Params { leader_exit: false, ..base }),
        ("max hold 10m", Params { max_hold_secs: 600, ..base }),
        ("max hold 30m", Params { max_hold_secs: 1800, ..base }),
    ] {
        let x = simulate(&hist, &timeline, &settings, &tips, p, (since, now)).await;
        println!("   {:<18} {:>4} trades  win {:>5.1}%  PF {:>5.2}  P&L {:+.4} SOL", label, x.trades.len(),
            if x.trades.is_empty() { 0.0 } else { 100.0 * x.wins() as f64 / x.trades.len() as f64 }, x.profit_factor(), sol(x.pnl()));
    }

    // Wallet selection, walk-forward: keep the wallets whose copies made money in the first
    // half, then copy only those in the second half (and compare with copying everyone)
    if args.iter().any(|a| a == "--select-wallets") {
        let min_trades: usize = arg(&args, "--min-select-trades", 5);
        let mid = since + (now - since) / 2;
        let first = simulate(&hist, &timeline, &settings, &tips, base, (since, mid)).await;
        let mut per: BTreeMap<&str, (usize, i64)> = BTreeMap::new();
        for t in &first.trades {
            let e = per.entry(t.leader.as_str()).or_default();
            e.0 += 1;
            e.1 += t.pnl_lamports;
        }
        let chosen: HashSet<String> = per.iter().filter(|(_, (n, pnl))| *n >= min_trades && *pnl > 0).map(|(w, _)| w.to_string()).collect();
        println!("\n================ WALLET SELECTION (picked on 1st half: ≥{min_trades} copies and profitable; tested on 2nd half) ================");
        println!("   1st half ({} → {}): {} of {} wallets qualified", fmt_time(Some(since)), fmt_time(Some(mid)), chosen.len(), wallets.len());
        for w in &chosen {
            let (n, pnl) = per[w.as_str()];
            println!("     {}  {:>3} copies  {:+.4} SOL", w, n, sol(pnl));
        }
        let picked = Settings { targets: chosen.clone(), ..settings.clone() };
        let all = simulate(&hist, &timeline, &settings, &tips, base, (mid, now)).await;
        print_summary("   2nd half, copying ALL wallets:", &all);
        if chosen.is_empty() {
            println!("   2nd half, copying the picked wallets: none qualified");
        } else {
            let sel = simulate(&hist, &timeline, &picked, &tips, base, (mid, now)).await;
            print_summary("   2nd half, copying only the PICKED wallets:", &sel);
        }
    }

    // Parameter sweep with a walk-forward check: pick on the first half, judge on the second
    if args.iter().any(|a| a == "--sweep") {
        let mid = since + (now - since) / 2;
        let mut grid = Vec::new();
        for trailing in [0.10, 0.15, 0.20, 0.30] {
            for hard_stop in [0.10, 0.20, 0.30, 0.50] {
                for take_profit in [1.5, 2.0, 2.5, 4.0, 1000.0] {
                    for leader_exit in [true, false] {
                        grid.push(Params { trailing, hard_stop, take_profit, leader_exit, ..base });
                    }
                }
            }
        }
        println!("\n================ SWEEP ({} combinations; chosen on first half, tested on second half) ================", grid.len());
        let mut scored = Vec::new();
        for p in grid {
            let first = simulate(&hist, &timeline, &settings, &tips, p, (since, mid)).await;
            let second = simulate(&hist, &timeline, &settings, &tips, p, (mid, now)).await;
            scored.push((p, first.pnl(), first.trades.len(), second.pnl(), second.trades.len()));
        }
        scored.sort_by_key(|s| std::cmp::Reverse(s.1));
        println!("{:>8} {:>8} {:>8} {:>6}   {:>16}   {:>16}", "TRAIL", "STOP", "TP", "LEADER", "1ST HALF P&L", "2ND HALF P&L");
        let show = |p: &Params, a: i64, an: usize, b: i64, bn: usize| {
            let tp = if p.take_profit > 100.0 { "off".to_string() } else { format!("{:.1}x", p.take_profit) };
            println!("{:>7.0}% {:>7.0}% {:>8} {:>6}   {:>+9.4} ({:>3})   {:>+9.4} ({:>3})",
                p.trailing * 100.0, p.hard_stop * 100.0, tp, if p.leader_exit { "on" } else { "off" }, sol(a), an, sol(b), bn);
        };
        for (p, a, an, b, bn) in scored.iter().take(12) {
            show(p, *a, *an, *b, *bn);
        }
        if let Some((p, a, an, b, bn)) = scored.iter().find(|s| {
            s.0.trailing == base.trailing && s.0.hard_stop == base.hard_stop && s.0.take_profit == base.take_profit && s.0.leader_exit
        }) {
            println!("   current settings:");
            show(p, *a, *an, *b, *bn);
        }
    }

    let out = Path::new(CACHE_DIR).join("last_run_trades.json");
    save(&out, &r.trades);
    println!("\nWrote {} (every simulated trade)", out.display());
    println!("Assumptions: our order lands {latency} slots after the trigger at that slot's opening price; Pump.fun fee {:.2}%/side; \
tips from TIP_* settings; positions open at the end are marked to market; signal clusters (SIGNAL_WALLETS) are not simulated.",
        PUMP_FEE_BPS as f64 / 100.0);
    Ok(())
}
