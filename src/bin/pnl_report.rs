//! Realized P&L report for the trader wallet, rebuilt from on-chain history.
//!
//! Usage: pnl_report [WALLET] [--limit N] [--rpc URL] [--json PATH]
//!   WALLET   defaults to the configured trader keypair's pubkey
//!   --limit  max signatures to scan, newest first (default 2000)
//!   --rpc    RPC endpoint (default SOLANA_RPC_URL from config)
//!   --json   also write the per-token breakdown as JSON

use anyhow::{Context, Result};
use futures::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use solana_sdk::signer::Signer;
use std::collections::{BTreeMap, HashMap, HashSet};

use clockit::{
    config::BotConfig,
    constants::{
        wsol_mint, JITO_TIP_WALLETS, PUMPFUN_PROGRAM, PUMPSWAP_PROGRAM, RAYDIUM_AMM_V4_PROGRAM,
        RAYDIUM_CPMM_PROGRAM,
    },
};

const LAMPORTS: f64 = 1e9;

#[derive(Default, Serialize)]
struct MintStats {
    mint: String,
    venue: String,
    buys: u32,
    sells: u32,
    /// SOL paid for buys, including fees and tips of those txs
    sol_spent: f64,
    /// SOL received from sells, net of fees and tips of those txs
    sol_received: f64,
    tokens_bought: i128,
    tokens_sold: i128,
    first_slot: u64,
    last_slot: u64,
}

impl MintStats {
    fn pnl(&self) -> f64 {
        self.sol_received - self.sol_spent
    }
    fn is_open(&self) -> bool {
        // Holding more than 1% of what was bought counts as still open
        self.tokens_bought - self.tokens_sold > self.tokens_bought / 100
    }
}

#[derive(Default)]
struct Totals {
    txs: u32,
    failed_txs: u32,
    failed_fees: f64,
    network_fees: f64,
    jito_tips: f64,
    deposits: f64,
    withdrawals: f64,
    other_net: f64,
    other_txs: u32,
    first_time: Option<i64>,
    last_time: Option<i64>,
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

async fn rpc(client: &reqwest::Client, url: &str, method: &str, params: Value) -> Result<Value> {
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

fn fmt_time(ts: Option<i64>) -> String {
    ts.map(|t| {
        let days = t / 86_400;
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
        format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", y, m, d, (t % 86_400) / 3600, (t % 3600) / 60)
    })
    .unwrap_or_else(|| "?".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = BotConfig::load_from_env().context("Failed to load config")?;

    let wallet = match args.first().filter(|a| !a.starts_with("--")) {
        Some(w) => w.clone(),
        None => config.load_keypair().context("No WALLET given and keypair failed to load")?.pubkey().to_string(),
    };
    let limit: usize = arg_value(&args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(2000);
    let rpc_url = arg_value(&args, "--rpc").unwrap_or_else(|| config.solana_rpc_url.clone());
    let json_out = arg_value(&args, "--json");

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
    let tip_wallets: HashSet<&str> = JITO_TIP_WALLETS.iter().copied().collect();
    let wsol = wsol_mint().to_string();

    println!("📊 P&L report for wallet {}", wallet);

    // 1. Signatures, newest first
    let mut signatures: Vec<String> = Vec::new();
    let mut before: Option<String> = None;
    while signatures.len() < limit {
        let mut opts = json!({"limit": 1000.min(limit - signatures.len())});
        if let Some(b) = &before {
            opts["before"] = json!(b);
        }
        let page = rpc(&client, &rpc_url, "getSignaturesForAddress", json!([wallet, opts])).await?;
        let page = page.as_array().cloned().unwrap_or_default();
        if page.is_empty() {
            break;
        }
        before = page.last().and_then(|s| s["signature"].as_str()).map(String::from);
        signatures.extend(page.iter().filter_map(|s| s["signature"].as_str().map(String::from)));
    }
    println!("   Found {} transactions, fetching details...", signatures.len());

    // 2. Transactions (8 in flight)
    let txs: Vec<(String, Result<Value>)> = stream::iter(signatures)
        .map(|sig| {
            let client = client.clone();
            let rpc_url = rpc_url.clone();
            async move {
                let res = rpc(
                    &client,
                    &rpc_url,
                    "getTransaction",
                    json!([sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0, "commitment": "confirmed"}]),
                )
                .await;
                (sig, res)
            }
        })
        .buffer_unordered(8)
        .collect()
        .await;

    // 3. Classify
    let mut totals = Totals::default();
    let mut mints: BTreeMap<String, MintStats> = BTreeMap::new();
    let mut fetch_errors = 0;

    for (sig, res) in txs {
        let tx = match res {
            Ok(tx) if !tx.is_null() => tx,
            Ok(_) => continue,
            Err(e) => {
                fetch_errors += 1;
                log::warn!("skipping {}: {:#}", sig, e);
                continue;
            }
        };
        let meta = &tx["meta"];
        let keys = tx["transaction"]["message"]["accountKeys"].as_array().cloned().unwrap_or_default();
        let Some(idx) = keys.iter().position(|k| k["pubkey"] == wallet.as_str()) else { continue };
        let pre = meta["preBalances"][idx].as_i64().unwrap_or(0);
        let post = meta["postBalances"][idx].as_i64().unwrap_or(0);
        let delta = (post - pre) as f64 / LAMPORTS;
        let is_fee_payer = idx == 0;
        let fee = if is_fee_payer { meta["fee"].as_u64().unwrap_or(0) as f64 / LAMPORTS } else { 0.0 };
        let block_time = tx["blockTime"].as_i64();
        let slot = tx["slot"].as_u64().unwrap_or(0);

        totals.txs += 1;
        totals.first_time = Some(totals.first_time.map_or(block_time.unwrap_or(0), |t| t.min(block_time.unwrap_or(t))));
        totals.last_time = Some(totals.last_time.map_or(block_time.unwrap_or(0), |t| t.max(block_time.unwrap_or(t))));

        if !meta["err"].is_null() {
            totals.failed_txs += 1;
            totals.failed_fees += -delta;
            continue;
        }

        totals.network_fees += fee;
        totals.jito_tips += jito_tips(&tx, &wallet, &tip_wallets) as f64 / LAMPORTS;

        let deltas = token_deltas(meta, &wallet, &wsol);
        let programs = invoked_programs(&tx);

        if deltas.len() == 1 {
            let (mint, token_delta) = deltas.into_iter().next().unwrap();
            let stats = mints.entry(mint.clone()).or_insert_with(|| MintStats {
                mint,
                first_slot: slot,
                ..Default::default()
            });
            if stats.venue.is_empty() || stats.venue == "other" {
                stats.venue = venue(&programs).to_string();
            }
            stats.first_slot = stats.first_slot.min(slot);
            stats.last_slot = stats.last_slot.max(slot);
            if token_delta > 0 {
                stats.buys += 1;
                stats.tokens_bought += token_delta;
                stats.sol_spent += -delta;
            } else {
                stats.sells += 1;
                stats.tokens_sold += -token_delta;
                stats.sol_received += delta;
            }
        } else if deltas.is_empty() && venue(&programs) == "other" {
            // Plain SOL movement: funding in or out of the trading wallet
            if delta > 0.0 {
                totals.deposits += delta;
            } else {
                totals.withdrawals += -delta;
            }
        } else {
            // Multi-token swaps, ATA closes, arb legs that net out, etc.
            totals.other_txs += 1;
            totals.other_net += delta;
        }
    }

    // 4. Report
    let mut rows: Vec<&MintStats> = mints.values().collect();
    rows.sort_by(|a, b| b.pnl().partial_cmp(&a.pnl()).unwrap_or(std::cmp::Ordering::Equal));

    let closed: Vec<&&MintStats> = rows.iter().filter(|m| !m.is_open() && m.buys > 0).collect();
    let open: Vec<&&MintStats> = rows.iter().filter(|m| m.is_open()).collect();
    let wins = closed.iter().filter(|m| m.pnl() > 0.0).count();
    let realized: f64 = closed.iter().map(|m| m.pnl()).sum();
    let open_cost: f64 = open.iter().map(|m| m.sol_spent - m.sol_received).sum();
    let gross_win: f64 = closed.iter().filter(|m| m.pnl() > 0.0).map(|m| m.pnl()).sum();
    let gross_loss: f64 = closed.iter().filter(|m| m.pnl() < 0.0).map(|m| -m.pnl()).sum();

    println!("\n================ PER TOKEN (closed + open) ================");
    println!("{:<46} {:>9} {:>4} {:>4} {:>10} {:>10} {:>10}  {}", "MINT", "VENUE", "BUY", "SELL", "SPENT", "RECEIVED", "PNL", "STATUS");
    for m in &rows {
        println!(
            "{:<46} {:>9} {:>4} {:>4} {:>10.4} {:>10.4} {:>+10.4}  {}",
            m.mint, m.venue, m.buys, m.sells, m.sol_spent, m.sol_received, m.pnl(),
            if m.is_open() { "OPEN" } else { "closed" }
        );
    }

    println!("\n========================= SUMMARY =========================");
    println!("Period:                {} → {}", fmt_time(totals.first_time), fmt_time(totals.last_time));
    println!("Transactions scanned:  {} ({} failed, {} could not be fetched)", totals.txs, totals.failed_txs, fetch_errors);
    println!("Tokens traded:         {} closed, {} still open", closed.len(), open.len());
    if !closed.is_empty() {
        println!(
            "Win rate (closed):     {}/{} = {:.1}%",
            wins, closed.len(), wins as f64 / closed.len() as f64 * 100.0
        );
        println!(
            "Avg win / avg loss:    {:+.4} / {:+.4} SOL",
            if wins > 0 { gross_win / wins as f64 } else { 0.0 },
            if closed.len() > wins { -gross_loss / (closed.len() - wins) as f64 } else { 0.0 }
        );
        if gross_loss > 0.0 {
            println!("Profit factor:         {:.2} (gross wins / gross losses; >1 = profitable)", gross_win / gross_loss);
        }
    }
    println!("REALIZED TRADING P&L:  {:+.4} SOL (closed tokens, fees + tips included)", realized);
    println!("Capital in open bags:  {:.4} SOL net cost (value them with check_position)", open_cost);
    println!("Network fees paid:     {:.4} SOL", totals.network_fees);
    println!("Jito tips paid:        {:.4} SOL", totals.jito_tips);
    println!("Failed tx cost:        {:.4} SOL over {} txs", totals.failed_fees, totals.failed_txs);
    println!("Other txs (net):       {:+.4} SOL over {} txs (multi-token / arb / account closes)", totals.other_net, totals.other_txs);
    println!("Deposits / withdrawals:{:>+8.4} / {:.4} SOL (funding, not P&L)", totals.deposits, totals.withdrawals);
    println!(
        "NET RESULT:            {:+.4} SOL (realized + other − failed; open bags excluded)",
        realized + totals.other_net - totals.failed_fees
    );

    if let Some(path) = json_out {
        std::fs::write(&path, serde_json::to_vec_pretty(&rows)?).with_context(|| format!("writing {}", path))?;
        println!("\nPer-token breakdown written to {}", path);
    }
    Ok(())
}
