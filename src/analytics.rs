//! Wallet P&L reconstruction from on-chain history (used by pnl_report and score_wallets)

use anyhow::{Context, Result};
use futures::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::constants::{
    wsol_mint, JITO_TIP_WALLETS, PUMPFUN_PROGRAM, PUMPSWAP_PROGRAM, RAYDIUM_AMM_V4_PROGRAM,
    RAYDIUM_CPMM_PROGRAM,
};

const LAMPORTS: f64 = 1e9;

#[derive(Default, Serialize, Clone)]
pub struct MintStats {
    pub mint: String,
    pub venue: String,
    pub buys: u32,
    pub sells: u32,
    /// SOL paid for buys, including fees and tips of those txs
    pub sol_spent: f64,
    /// SOL received from sells, net of fees and tips of those txs
    pub sol_received: f64,
    pub tokens_bought: i128,
    pub tokens_sold: i128,
    pub first_slot: u64,
    pub last_slot: u64,
    /// Unix time of the first and last trade in this token
    pub first_time: i64,
    pub last_time: i64,
    /// Bought in the same transaction that created the token (dev / bundled launch)
    pub bought_in_create_tx: bool,
}

impl MintStats {
    /// Seconds between the first and last trade in this token
    pub fn hold_secs(&self) -> i64 {
        (self.last_time - self.first_time).max(0)
    }
    pub fn pnl(&self) -> f64 {
        self.sol_received - self.sol_spent
    }
    pub fn is_open(&self) -> bool {
        // Holding more than 1% of what was bought counts as still open
        self.tokens_bought - self.tokens_sold > self.tokens_bought / 100
    }
    pub fn is_closed_trade(&self) -> bool {
        !self.is_open() && self.buys > 0
    }
}

#[derive(Default)]
pub struct Totals {
    pub txs: u32,
    pub failed_txs: u32,
    pub failed_fees: f64,
    pub network_fees: f64,
    pub jito_tips: f64,
    pub deposits: f64,
    pub withdrawals: f64,
    pub other_net: f64,
    pub other_txs: u32,
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
}

#[derive(Default)]
pub struct WalletReport {
    pub mints: BTreeMap<String, MintStats>,
    pub totals: Totals,
    pub fetch_errors: u32,
    /// On-chain error of failed txs → count
    pub errors: HashMap<String, u32>,
}

/// Headline numbers over closed trades
pub struct Summary {
    pub closed: usize,
    pub open: usize,
    pub wins: usize,
    pub realized: f64,
    pub gross_win: f64,
    pub gross_loss: f64,
    pub open_cost: f64,
    pub volume: f64,
}

impl Summary {
    pub fn win_rate(&self) -> f64 {
        if self.closed == 0 { 0.0 } else { self.wins as f64 / self.closed as f64 }
    }
    /// Gross wins / gross losses (>1 = profitable); infinite with no losses
    pub fn profit_factor(&self) -> f64 {
        if self.gross_loss > 0.0 { self.gross_win / self.gross_loss } else if self.gross_win > 0.0 { f64::INFINITY } else { 0.0 }
    }
    /// Realized P&L as a share of SOL deployed into closed trades
    pub fn return_pct(&self) -> f64 {
        if self.volume > 0.0 { self.realized / self.volume * 100.0 } else { 0.0 }
    }
}

impl WalletReport {
    /// Per-token rows sorted best to worst
    pub fn rows(&self) -> Vec<&MintStats> {
        let mut rows: Vec<&MintStats> = self.mints.values().collect();
        rows.sort_by(|a, b| b.pnl().partial_cmp(&a.pnl()).unwrap_or(std::cmp::Ordering::Equal));
        rows
    }

    pub fn summary(&self) -> Summary {
        summarize(self.mints.values())
    }
}

/// Summarize any set of per-token stats (closed trades drive the headline numbers)
pub fn summarize<'a>(mints: impl Iterator<Item = &'a MintStats>) -> Summary {
    let mut s = Summary { closed: 0, open: 0, wins: 0, realized: 0.0, gross_win: 0.0, gross_loss: 0.0, open_cost: 0.0, volume: 0.0 };
    for m in mints {
        if m.is_open() {
            s.open += 1;
            s.open_cost += m.sol_spent - m.sol_received;
        } else if m.is_closed_trade() {
            s.closed += 1;
            s.volume += m.sol_spent;
            let p = m.pnl();
            s.realized += p;
            if p > 0.0 {
                s.wins += 1;
                s.gross_win += p;
            } else {
                s.gross_loss += -p;
            }
        }
    }
    s
}

pub async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    for attempt in 0..5u64 {
        let resp = client.post(url).json(&body).send().await;
        match resp {
            Ok(r) if r.status().as_u16() == 429 => {
                tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
            }
            Ok(r) => {
                let v: Value = r.json().await.context("invalid JSON from RPC")?;
                if let Some(err) = v.get("error") {
                    anyhow::bail!("{} error: {}", method, err);
                }
                return Ok(v["result"].clone());
            }
            Err(e) if attempt < 4 => {
                log::debug!("{} retry after error: {}", method, e);
                tokio::time::sleep(std::time::Duration::from_millis(500 * (attempt + 1))).await;
            }
            Err(e) => return Err(e).with_context(|| format!("{} request failed", method)),
        }
    }
    anyhow::bail!("{} rate-limited after retries", method)
}

/// All program ids invoked by the tx (top-level and inner)
fn invoked_programs(tx: &Value) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut collect = |ixs: &Value| {
        for ix in ixs.as_array().into_iter().flatten() {
            if let Some(p) = ix["programId"].as_str() {
                ids.insert(p.to_string());
            }
        }
    };
    collect(&tx["transaction"]["message"]["instructions"]);
    for inner in tx["meta"]["innerInstructions"].as_array().into_iter().flatten() {
        collect(&inner["instructions"]);
    }
    ids
}

/// Lamports the wallet sent to Jito tip accounts in this tx
fn jito_tips(tx: &Value, wallet: &str, tip_wallets: &HashSet<&str>) -> u64 {
    let mut total = 0;
    let mut scan = |ixs: &Value| {
        for ix in ixs.as_array().into_iter().flatten() {
            let info = &ix["parsed"]["info"];
            if ix["program"] == "system"
                && ix["parsed"]["type"] == "transfer"
                && info["source"] == wallet
                && info["destination"].as_str().is_some_and(|d| tip_wallets.contains(d))
            {
                total += info["lamports"].as_u64().unwrap_or(0);
            }
        }
    };
    scan(&tx["transaction"]["message"]["instructions"]);
    for inner in tx["meta"]["innerInstructions"].as_array().into_iter().flatten() {
        scan(&inner["instructions"]);
    }
    total
}

/// Per-mint change in the wallet's token balance (raw units), WSOL excluded
fn token_deltas(meta: &Value, wallet: &str, wsol: &str) -> HashMap<String, i128> {
    let mut deltas: HashMap<String, i128> = HashMap::new();
    let mut apply = |list: &Value, sign: i128| {
        for b in list.as_array().into_iter().flatten() {
            if b["owner"] != wallet || b["mint"] == wsol {
                continue;
            }
            let amount: i128 = b["uiTokenAmount"]["amount"].as_str().unwrap_or("0").parse().unwrap_or(0);
            if let Some(mint) = b["mint"].as_str() {
                *deltas.entry(mint.to_string()).or_default() += sign * amount;
            }
        }
    };
    apply(&meta["preTokenBalances"], -1);
    apply(&meta["postTokenBalances"], 1);
    deltas.retain(|_, d| *d != 0);
    deltas
}

fn venue(programs: &HashSet<String>) -> &'static str {
    if programs.contains(PUMPFUN_PROGRAM) {
        "pump.fun"
    } else if programs.contains(PUMPSWAP_PROGRAM) {
        "pumpswap"
    } else if programs.contains(RAYDIUM_AMM_V4_PROGRAM) || programs.contains(RAYDIUM_CPMM_PROGRAM) {
        "raydium"
    } else {
        "other"
    }
}

/// Whether the tx contains a Pump.fun create instruction (top-level or inner)
fn has_pump_create(tx: &Value) -> bool {
    let is_create = |ix: &Value| {
        ix["programId"] == PUMPFUN_PROGRAM
            && ix["data"]
                .as_str()
                .and_then(|d| bs58::decode(d).into_vec().ok())
                .is_some_and(|bytes| bytes.len() >= 8 && bytes[..8] == crate::constants::CREATE_DISCRIMINATOR_BYTES)
    };
    tx["transaction"]["message"]["instructions"].as_array().into_iter().flatten().any(is_create)
        || tx["meta"]["innerInstructions"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|inner| inner["instructions"].as_array().into_iter().flatten())
            .any(is_create)
}

/// Readable form of a failed tx's error, e.g. "ix 3: Custom 6003"
fn describe_error(err: &Value) -> String {
    if let Some(ie) = err.get("InstructionError").and_then(|v| v.as_array()) {
        let idx = ie.first().and_then(|v| v.as_u64()).unwrap_or(0);
        let detail = match ie.get(1) {
            Some(Value::Object(o)) => o
                .iter()
                .map(|(k, v)| format!("{} {}", k, v))
                .collect::<Vec<_>>()
                .join(","),
            Some(v) => v.as_str().map(String::from).unwrap_or_else(|| v.to_string()),
            None => "?".into(),
        };
        return format!("ix {}: {}", idx, detail);
    }
    err.as_str().map(String::from).unwrap_or_else(|| err.to_string())
}

/// Rebuild trading results for `wallet` from its most recent `limit` transactions
pub async fn analyze_wallet(
    client: &reqwest::Client,
    rpc_url: &str,
    wallet: &str,
    limit: usize,
    concurrency: usize,
) -> Result<WalletReport> {
    let tip_wallets: HashSet<&str> = JITO_TIP_WALLETS.iter().copied().collect();
    let wsol = wsol_mint().to_string();

    // 1. Signatures, newest first
    let mut signatures: Vec<String> = Vec::new();
    let mut before: Option<String> = None;
    while signatures.len() < limit {
        let mut opts = json!({"limit": 1000.min(limit - signatures.len())});
        if let Some(b) = &before {
            opts["before"] = json!(b);
        }
        let page = rpc(client, rpc_url, "getSignaturesForAddress", json!([wallet, opts])).await?;
        let page = page.as_array().cloned().unwrap_or_default();
        if page.is_empty() {
            break;
        }
        before = page.last().and_then(|s| s["signature"].as_str()).map(String::from);
        let short_page = page.len() < 1000;
        signatures.extend(page.iter().filter_map(|s| s["signature"].as_str().map(String::from)));
        if short_page {
            break;
        }
    }

    // 2. Transactions
    let txs: Vec<(String, Result<Value>)> = stream::iter(signatures)
        .map(|sig| async move {
            let res = rpc(
                client,
                rpc_url,
                "getTransaction",
                json!([sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0, "commitment": "confirmed"}]),
            )
            .await;
            (sig, res)
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // 3. Classify
    let mut report = WalletReport::default();
    for (sig, res) in txs {
        let tx = match res {
            Ok(tx) if !tx.is_null() => tx,
            Ok(_) => continue,
            Err(e) => {
                report.fetch_errors += 1;
                log::warn!("skipping {}: {:#}", sig, e);
                continue;
            }
        };
        let meta = &tx["meta"];
        let keys = tx["transaction"]["message"]["accountKeys"].as_array().cloned().unwrap_or_default();
        let Some(idx) = keys.iter().position(|k| k["pubkey"] == wallet) else { continue };
        let pre = meta["preBalances"][idx].as_i64().unwrap_or(0);
        let post = meta["postBalances"][idx].as_i64().unwrap_or(0);
        let delta = (post - pre) as f64 / LAMPORTS;
        let fee = if idx == 0 { meta["fee"].as_u64().unwrap_or(0) as f64 / LAMPORTS } else { 0.0 };
        let slot = tx["slot"].as_u64().unwrap_or(0);

        let totals = &mut report.totals;
        totals.txs += 1;
        if let Some(t) = tx["blockTime"].as_i64() {
            totals.first_time = Some(totals.first_time.map_or(t, |f| f.min(t)));
            totals.last_time = Some(totals.last_time.map_or(t, |l| l.max(t)));
        }

        if !meta["err"].is_null() {
            totals.failed_txs += 1;
            totals.failed_fees += -delta;
            *report.errors.entry(describe_error(&meta["err"])).or_default() += 1;
            continue;
        }

        totals.network_fees += fee;
        totals.jito_tips += jito_tips(&tx, wallet, &tip_wallets) as f64 / LAMPORTS;

        let deltas = token_deltas(meta, wallet, &wsol);
        let programs = invoked_programs(&tx);

        if deltas.len() == 1 {
            let (mint, token_delta) = deltas.into_iter().next().unwrap();
            let block_time = tx["blockTime"].as_i64().unwrap_or(0);
            let stats = report.mints.entry(mint.clone()).or_insert_with(|| MintStats {
                mint,
                first_slot: slot,
                first_time: block_time,
                last_time: block_time,
                ..Default::default()
            });
            if block_time > 0 {
                stats.first_time = if stats.first_time == 0 { block_time } else { stats.first_time.min(block_time) };
                stats.last_time = stats.last_time.max(block_time);
            }
            if stats.venue.is_empty() || stats.venue == "other" {
                stats.venue = venue(&programs).to_string();
            }
            stats.first_slot = stats.first_slot.min(slot);
            stats.last_slot = stats.last_slot.max(slot);
            if token_delta > 0 {
                if has_pump_create(&tx) {
                    stats.bought_in_create_tx = true;
                }
                stats.buys += 1;
                stats.tokens_bought += token_delta;
                stats.sol_spent += -delta;
            } else {
                stats.sells += 1;
                stats.tokens_sold += -token_delta;
                stats.sol_received += delta;
            }
        } else if deltas.is_empty() && venue(&programs) == "other" {
            // Plain SOL movement: funding in or out of the wallet
            if delta > 0.0 {
                totals.deposits += delta;
            } else {
                totals.withdrawals += -delta;
            }
        } else {
            // Multi-token swaps, account closes, arb legs that net out, etc.
            totals.other_txs += 1;
            totals.other_net += delta;
        }
    }
    Ok(report)
}

/// Rebuild per-token stats from a paper trading log (JSON lines written in dry-run mode)
pub fn paper_trades(path: &std::path::Path) -> Result<BTreeMap<String, MintStats>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut mints: BTreeMap<String, MintStats> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        let Some(mint) = rec["mint"].as_str() else { continue };
        let sol = rec["sol_lamports"].as_u64().unwrap_or(0) as f64 / LAMPORTS;
        let stats = mints.entry(mint.to_string()).or_insert_with(|| MintStats {
            mint: mint.to_string(),
            venue: "paper".into(),
            ..Default::default()
        });
        match rec["kind"].as_str() {
            Some("buy") => {
                stats.buys += 1;
                stats.sol_spent += sol;
                stats.tokens_bought += rec["tokens"].as_u64().unwrap_or(0) as i128;
            }
            Some("sell") => {
                stats.sells += 1;
                stats.sol_received += sol;
                stats.tokens_sold = stats.tokens_bought;
            }
            _ => {}
        }
    }
    Ok(mints)
}

pub fn fmt_time(ts: Option<i64>) -> String {
    ts.map(|t| {
        let days = t.div_euclid(86_400);
        // Civil date from days since epoch (Howard Hinnant's algorithm)
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        let secs = t.rem_euclid(86_400);
        format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, m, d, secs / 3600, (secs % 3600) / 60)
    })
    .unwrap_or_else(|| "?".into())
}
