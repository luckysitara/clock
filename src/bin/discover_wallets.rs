//! Find the richest active Pump.fun traders and profile their strategies.
//!
//! 1. Watch live Pump.fun trades on Yellowstone for --minutes and collect every trader
//! 2. Look up SOL balances of the most active --candidates, keep those worth > --min-usd
//! 3. Take the --top richest and rebuild each one's P&L and trading style from on-chain history
//!
//! Usage: discover_wallets [--minutes 15] [--candidates 6000] [--min-usd 1000] [--top 1000]
//!                         [--limit 150] [--parallel 8] [--min-win-rate 0.7] [--min-trades 10]
//!                         [--sol-price USD]
//! Writes data/top_wallets.json, data/top_wallets_winrate.json and data/top_wallets_winrate.txt

use anyhow::{Context, Result};
use futures::{stream, StreamExt};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest, SubscribeRequestFilterTransactions,
};

use clockit::{
    analytics::{analyze_wallet, rpc, summarize},
    config::BotConfig,
    constants::{
        BUY_DISCRIMINATOR_U64, PUMPFUN_PROGRAM, SELL_DISCRIMINATOR_ALT_U64, SELL_DISCRIMINATOR_U64,
    },
    decoders::{fast_is_discriminator, PumpFunBuyPod, PumpFunSellPod},
};

#[derive(Default, Clone, Copy)]
struct Activity {
    trades: u32,
    sol_volume: f64,
}

#[derive(Serialize, Clone)]
struct WalletProfile {
    wallet: String,
    sol_balance: f64,
    usd_value: f64,
    stream_trades: u32,
    txs_scanned: u32,
    closed_trades: usize,
    open_positions: usize,
    wins: usize,
    win_rate: f64,
    profit_factor: f64,
    realized_sol: f64,
    return_pct: f64,
    avg_trade_sol: f64,
    median_hold_secs: i64,
    txs_per_hour: f64,
    launch_tx_buys: usize,
    venues: HashMap<String, usize>,
    style: String,
    error: Option<String>,
}

fn arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

async fn sol_price_usd(client: &reqwest::Client) -> Option<f64> {
    let jup = client
        .get("https://lite-api.jup.ag/price/v3?ids=So11111111111111111111111111111111111111112")
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v["So11111111111111111111111111111111111111112"]["usdPrice"].as_f64());
    if jup.is_some() {
        return jup;
    }
    client
        .get("https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd")
        .send()
        .await
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?["solana"]["usd"]
        .as_f64()
}

/// Phase 1: every signer that traded on Pump.fun during the window
async fn collect_traders(config: &BotConfig, minutes: u64) -> Result<HashMap<String, Activity>> {
    let mut builder = GeyserGrpcClient::build_from_shared(config.yellowstone_grpc_url.clone())?;
    if config.yellowstone_grpc_url.starts_with("https://") {
        builder = builder.tls_config(yellowstone_grpc_client::ClientTlsConfig::new().with_enabled_roots())?;
    }
    if let Some(token) = &config.yellowstone_x_token {
        builder = builder.x_token(Some(token.clone()))?;
    }
    let mut client = builder.connect().await.context("Yellowstone connect failed")?;

    let mut transactions = HashMap::new();
    transactions.insert(
        "pumpfun_traders".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: Some(false),
            signature: None,
            account_include: vec![PUMPFUN_PROGRAM.to_string()],
            account_exclude: vec![],
            account_required: vec![],
        },
    );
    let request = SubscribeRequest {
        transactions,
        commitment: Some(CommitmentLevel::Confirmed as i32),
        ..Default::default()
    };
    let (_sink, mut stream) = client.subscribe_with_request(Some(request)).await?;

    let pumpfun = clockit::constants::pumpfun_program_id().to_bytes();
    let window = Duration::from_secs(minutes * 60);
    let start = Instant::now();
    let mut traders: HashMap<String, Activity> = HashMap::new();
    let mut txs = 0u64;
    let mut last_print = Instant::now();

    while start.elapsed() < window {
        let msg = match tokio::time::timeout(window.saturating_sub(start.elapsed()), stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                eprintln!("\nstream error: {} (keeping {} traders collected so far)", e, traders.len());
                break;
            }
            Ok(None) | Err(_) => break,
        };
        let Some(UpdateOneof::Transaction(update)) = msg.update_oneof else { continue };
        let Some(info) = update.transaction.as_ref() else { continue };
        let Some(message) = info.transaction.as_ref().and_then(|t| t.message.as_ref()) else { continue };
        txs += 1;

        let signers = message.header.as_ref().map_or(1, |h| h.num_required_signatures as usize);
        let mut keys: Vec<&[u8]> = message.account_keys.iter().map(|k| k.as_slice()).collect();
        if let Some(meta) = info.meta.as_ref() {
            keys.extend(meta.loaded_writable_addresses.iter().map(|k| k.as_slice()));
            keys.extend(meta.loaded_readonly_addresses.iter().map(|k| k.as_slice()));
        }

        let inner = info.meta.iter().flat_map(|m| m.inner_instructions.iter()).flat_map(|i| i.instructions.iter());
        let all = message
            .instructions
            .iter()
            .map(|ix| (ix.program_id_index, ix.accounts.as_slice(), ix.data.as_slice()))
            .chain(inner.map(|ix| (ix.program_id_index, ix.accounts.as_slice(), ix.data.as_slice())));

        for (prog, accounts, data) in all {
            if keys.get(prog as usize) != Some(&&pumpfun[..]) {
                continue;
            }
            let sol = if fast_is_discriminator(data, BUY_DISCRIMINATOR_U64) {
                PumpFunBuyPod::read_from_raw(data).map(|p| p.max_sol_cost)
            } else if fast_is_discriminator(data, SELL_DISCRIMINATOR_U64) || fast_is_discriminator(data, SELL_DISCRIMINATOR_ALT_U64) {
                PumpFunSellPod::read_from_raw(data).map(|p| p.min_sol_output)
            } else {
                None
            };
            let Some(sol) = sol else { continue };
            // The trading user (account #6) must be a signer, i.e. a wallet rather than a program PDA
            let Some(&user_idx) = accounts.get(6) else { continue };
            if (user_idx as usize) >= signers {
                continue;
            }
            let Some(key) = keys.get(user_idx as usize).and_then(|k| <[u8; 32]>::try_from(*k).ok()) else { continue };
            let entry = traders.entry(bs58::encode(key).into_string()).or_default();
            entry.trades += 1;
            entry.sol_volume += sol as f64 / 1e9;
        }

        if last_print.elapsed() > Duration::from_secs(10) {
            last_print = Instant::now();
            eprint!(
                "\r   phase 1: {:>4}s / {}s | {} txs | {} unique traders",
                start.elapsed().as_secs(), window.as_secs(), txs, traders.len()
            );
        }
    }
    eprintln!();
    Ok(traders)
}

fn classify(p: &WalletProfile) -> String {
    let pumpfun = *p.venues.get("pump.fun").unwrap_or(&0);
    let post_grad = p.venues.get("pumpswap").unwrap_or(&0) + p.venues.get("raydium").unwrap_or(&0);
    if p.closed_trades + p.open_positions == 0 {
        "no token trades in sample".into()
    } else if p.launch_tx_buys * 3 >= p.closed_trades + p.open_positions {
        "dev / launch bundler (buys inside the token's create tx)".into()
    } else if p.txs_per_hour > 120.0 {
        "high-frequency bot".into()
    } else if post_grad > pumpfun {
        "post-graduation trader (PumpSwap/Raydium)".into()
    } else if p.median_hold_secs < 60 {
        "sniper / scalper (holds < 1 min)".into()
    } else if p.median_hold_secs < 30 * 60 {
        "momentum trader (holds minutes)".into()
    } else {
        "swing holder (holds 30 min+)".into()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let minutes: u64 = arg(&args, "--minutes", 15);
    let candidates: usize = arg(&args, "--candidates", 6000);
    let min_usd: f64 = arg(&args, "--min-usd", 1000.0);
    let top: usize = arg(&args, "--top", 1000);
    let limit: usize = arg(&args, "--limit", 150);
    let parallel: usize = arg(&args, "--parallel", 8);
    let min_win_rate: f64 = arg(&args, "--min-win-rate", 0.7);
    let min_trades: usize = arg(&args, "--min-trades", 10);

    let config = BotConfig::load_from_env().context("Failed to load config")?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
    let rpc_url = config.solana_rpc_url.clone();

    let sol_usd = match arg(&args, "--sol-price", 0.0f64) {
        p if p > 0.0 => p,
        _ => sol_price_usd(&client).await.context("Could not fetch SOL price; pass --sol-price USD")?,
    };
    println!("💵 SOL price: ${:.2} → ${} = {:.2} SOL", sol_usd, min_usd, min_usd / sol_usd);

    // Phase 1
    println!("📡 Phase 1: collecting Pump.fun traders from the live stream for {} min...", minutes);
    let traders = collect_traders(&config, minutes).await?;
    let mut active: Vec<(String, Activity)> = traders.into_iter().collect();
    active.sort_by(|a, b| b.1.sol_volume.partial_cmp(&a.1.sol_volume).unwrap_or(std::cmp::Ordering::Equal));
    active.truncate(candidates);
    println!("   {} candidates (most active by SOL volume)", active.len());

    // Phase 2
    println!("💰 Phase 2: checking balances...");
    let mut balances: HashMap<String, u64> = HashMap::new();
    for chunk in active.chunks(100) {
        let keys: Vec<&str> = chunk.iter().map(|(w, _)| w.as_str()).collect();
        let result = rpc(&client, &rpc_url, "getMultipleAccounts", json!([keys, {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}}])).await?;
        for (wallet, acc) in keys.iter().zip(result["value"].as_array().cloned().unwrap_or_default()) {
            balances.insert(wallet.to_string(), acc["lamports"].as_u64().unwrap_or(0));
        }
    }
    let activity: HashMap<String, Activity> = active.into_iter().collect();
    let mut rich: Vec<(String, f64)> = balances
        .into_iter()
        .map(|(w, l)| (w, l as f64 / 1e9))
        .filter(|(_, sol)| sol * sol_usd >= min_usd)
        .collect();
    rich.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let qualifying = rich.len();
    rich.truncate(top);
    println!("   {} wallets hold ≥ ${} in SOL; analysing the top {}", qualifying, min_usd, rich.len());

    // Phase 3
    println!("🔬 Phase 3: rebuilding P&L and style ({} recent txs per wallet)...", limit);
    let total = rich.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut profiles: Vec<WalletProfile> = stream::iter(rich)
        .map(|(wallet, sol_balance)| {
            let (client, rpc_url, done, activity) = (&client, &rpc_url, &done, &activity);
            async move {
                let result = analyze_wallet(client, rpc_url, &wallet, limit, 4).await;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n % 10 == 0 || n == total {
                    eprint!("\r   phase 3: {}/{} wallets analysed", n, total);
                }
                let mut p = WalletProfile {
                    wallet: wallet.clone(),
                    sol_balance,
                    usd_value: sol_balance * sol_usd,
                    stream_trades: activity.get(&wallet).map_or(0, |a| a.trades),
                    txs_scanned: 0, closed_trades: 0, open_positions: 0, wins: 0, win_rate: 0.0,
                    profit_factor: 0.0, realized_sol: 0.0, return_pct: 0.0, avg_trade_sol: 0.0,
                    median_hold_secs: 0, txs_per_hour: 0.0, launch_tx_buys: 0,
                    venues: HashMap::new(), style: String::new(), error: None,
                };
                match result {
                    Ok(report) => {
                        let s = summarize(report.mints.values());
                        let t = &report.totals;
                        let mut holds: Vec<i64> = report.mints.values().filter(|m| m.is_closed_trade()).map(|m| m.hold_secs()).collect();
                        holds.sort_unstable();
                        let hours = match (t.first_time, t.last_time) {
                            (Some(f), Some(l)) if l > f => (l - f) as f64 / 3600.0,
                            _ => 0.0,
                        };
                        for m in report.mints.values() {
                            *p.venues.entry(m.venue.clone()).or_default() += 1;
                        }
                        p.txs_scanned = t.txs;
                        p.closed_trades = s.closed;
                        p.open_positions = s.open;
                        p.wins = s.wins;
                        p.win_rate = s.win_rate();
                        p.profit_factor = if s.profit_factor().is_finite() { s.profit_factor() } else { 99.0 };
                        p.realized_sol = s.realized;
                        p.return_pct = s.return_pct();
                        p.avg_trade_sol = if s.closed > 0 { s.volume / s.closed as f64 } else { 0.0 };
                        p.median_hold_secs = holds.get(holds.len() / 2).copied().unwrap_or(0);
                        p.txs_per_hour = if hours > 0.0 { t.txs as f64 / hours } else { t.txs as f64 };
                        p.launch_tx_buys = report.mints.values().filter(|m| m.bought_in_create_tx).count();
                        p.style = classify(&p);
                    }
                    Err(e) => {
                        p.style = "analysis failed".into();
                        p.error = Some(format!("{:#}", e));
                    }
                }
                p
            }
        })
        .buffer_unordered(parallel.max(1))
        .collect()
        .await;
    eprintln!();

    profiles.sort_by(|a, b| b.realized_sol.partial_cmp(&a.realized_sol).unwrap_or(std::cmp::Ordering::Equal));
    let winners: Vec<&WalletProfile> = profiles
        .iter()
        .filter(|p| p.win_rate > min_win_rate && p.closed_trades >= min_trades && p.realized_sol > 0.0)
        .collect();

    std::fs::create_dir_all("data")?;
    std::fs::write("data/top_wallets.json", serde_json::to_vec_pretty(&profiles)?)?;
    std::fs::write("data/top_wallets_winrate.json", serde_json::to_vec_pretty(&winners)?)?;
    std::fs::write(
        "data/top_wallets_winrate.txt",
        winners.iter().map(|p| p.wallet.as_str()).collect::<Vec<_>>().join(","),
    )?;

    // Report
    let analysed = profiles.iter().filter(|p| p.error.is_none()).count();
    let mut styles: HashMap<&str, (usize, usize)> = HashMap::new();
    for p in profiles.iter().filter(|p| p.error.is_none()) {
        let e = styles.entry(p.style.as_str()).or_default();
        e.0 += 1;
        if winners.iter().any(|w| w.wallet == p.wallet) {
            e.1 += 1;
        }
    }
    println!("\n==================== STYLE MIX ({} analysed) ====================", analysed);
    let mut styles: Vec<_> = styles.into_iter().collect();
    styles.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (style, (n, w)) in &styles {
        println!("  {:>4} wallets  {:>3} pass  {}", n, w, style);
    }

    println!(
        "\n=========== WIN RATE > {:.0}% (≥{} closed trades, profitable): {} wallets ===========",
        min_win_rate * 100.0, min_trades, winners.len()
    );
    println!("{:<45} {:>9} {:>6} {:>5} {:>6} {:>9} {:>7} {:>7} {:>6}  {}", "WALLET", "BAL SOL", "TRADES", "WIN%", "PF", "PNL SOL", "RET%", "HOLD", "TX/H", "STYLE");
    for p in &winners {
        println!(
            "{:<45} {:>9.1} {:>6} {:>4.0}% {:>6.2} {:>+9.2} {:>+6.1}% {:>6}s {:>6.0}  {}",
            p.wallet, p.sol_balance, p.closed_trades, p.win_rate * 100.0, p.profit_factor,
            p.realized_sol, p.return_pct, p.median_hold_secs, p.txs_per_hour, p.style
        );
    }
    println!("\nWrote data/top_wallets.json ({} wallets), data/top_wallets_winrate.json and .txt ({} wallets)", profiles.len(), winners.len());
    Ok(())
}
