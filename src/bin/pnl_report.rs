//! Realized P&L report for a wallet, rebuilt from on-chain history.
//!
//! Usage: pnl_report [WALLET] [--limit N] [--rpc URL] [--json PATH]
//!        pnl_report --paper [PATH]     summarize dry-run paper trades
//!        pnl_report --latency [PATH]   how many blocks behind the copied trade our buys land
//!   WALLET   defaults to the configured trader keypair's pubkey
//!   --limit  max signatures to scan, newest first (default 2000)
//!   --rpc    RPC endpoint (default SOLANA_RPC_URL from config)
//!   --json   also write the per-token breakdown as JSON

use anyhow::{Context, Result};
use solana_sdk::signer::Signer;

use clockit::{
    analytics::{analyze_wallet, fmt_time, paper_trades, summarize, MintStats, Summary},
    config::BotConfig,
};

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn print_rows(rows: &[&MintStats]) {
    println!("\n================ PER TOKEN (closed + open) ================");
    println!("{:<46} {:>9} {:>4} {:>4} {:>10} {:>10} {:>10}  {}", "MINT", "VENUE", "BUY", "SELL", "SPENT", "RECEIVED", "PNL", "STATUS");
    for m in rows {
        println!(
            "{:<46} {:>9} {:>4} {:>4} {:>10.4} {:>10.4} {:>+10.4}  {}",
            m.mint, m.venue, m.buys, m.sells, m.sol_spent, m.sol_received, m.pnl(),
            if m.is_open() { "OPEN" } else { "closed" }
        );
    }
}

fn print_summary(s: &Summary) {
    println!("Tokens traded:         {} closed, {} still open", s.closed, s.open);
    if s.closed > 0 {
        let losses = s.closed - s.wins;
        println!("Win rate (closed):     {}/{} = {:.1}%", s.wins, s.closed, s.win_rate() * 100.0);
        println!(
            "Avg win / avg loss:    {:+.4} / {:+.4} SOL",
            if s.wins > 0 { s.gross_win / s.wins as f64 } else { 0.0 },
            if losses > 0 { -s.gross_loss / losses as f64 } else { 0.0 }
        );
        println!("Profit factor:         {:.2} (gross wins / gross losses; >1 = profitable)", s.profit_factor());
        println!("Return on deployed:    {:+.1}% of {:.4} SOL", s.return_pct(), s.volume);
    }
    println!("REALIZED TRADING P&L:  {:+.4} SOL (closed tokens, fees + tips included)", s.realized);
    println!("Capital in open bags:  {:.4} SOL net cost (value them with check_position)", s.open_cost);
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = BotConfig::load_from_env().context("Failed to load config")?;

    if args.first().map(String::as_str) == Some("--latency") {
        let path = args.get(1).cloned().unwrap_or_else(|| clockit::engine::executor::LATENCY_LOG.to_string());
        return latency_report(&path);
    }

    if args.first().map(String::as_str) == Some("--paper") {
        let path = args.get(1).cloned().unwrap_or_else(|| config.paper_trades_file.clone());
        let mints = paper_trades(std::path::Path::new(&path))?;
        println!("📝 Paper trading report from {}", path);
        let mut rows: Vec<&MintStats> = mints.values().collect();
        rows.sort_by(|a, b| b.pnl().partial_cmp(&a.pnl()).unwrap_or(std::cmp::Ordering::Equal));
        print_rows(&rows);
        println!("\n========================= SUMMARY =========================");
        print_summary(&summarize(mints.values()));
        println!("(Estimates: Pump.fun fees modelled at 1.25%/side; real fills will be worse when slow)");
        return Ok(());
    }

    let wallet = match args.first().filter(|a| !a.starts_with("--")) {
        Some(w) => w.clone(),
        None => config.load_keypair().context("No WALLET given and keypair failed to load")?.pubkey().to_string(),
    };
    let limit: usize = arg_value(&args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(2000);
    let rpc_url = arg_value(&args, "--rpc").unwrap_or_else(|| config.solana_rpc_url.clone());
    let json_out = arg_value(&args, "--json");

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
    println!("📊 P&L report for wallet {}", wallet);
    let report = analyze_wallet(&client, &rpc_url, &wallet, limit, 8).await?;
    let rows = report.rows();
    print_rows(&rows);

    let t = &report.totals;
    println!("\n========================= SUMMARY =========================");
    println!("Period:                {} → {}", fmt_time(t.first_time), fmt_time(t.last_time));
    println!("Transactions scanned:  {} ({} failed, {} could not be fetched)", t.txs, t.failed_txs, report.fetch_errors);
    let summary = report.summary();
    print_summary(&summary);
    println!("Network fees paid:     {:.4} SOL", t.network_fees);
    println!("Jito tips paid:        {:.4} SOL", t.jito_tips);
    println!("Failed tx cost:        {:.4} SOL over {} txs", t.failed_fees, t.failed_txs);
    println!("Other txs (net):       {:+.4} SOL over {} txs (multi-token / arb / account closes)", t.other_net, t.other_txs);
    println!("Deposits / withdrawals:{:>+8.4} / {:.4} SOL (funding, not P&L)", t.deposits, t.withdrawals);
    println!(
        "NET RESULT:            {:+.4} SOL (realized + other − failed; open bags excluded)",
        summary.realized + t.other_net - t.failed_fees
    );

    if !report.errors.is_empty() {
        println!("\nFailed tx reasons (Pump.fun: 6003 = too little SOL out, 6002 = too much SOL required, i.e. slippage):");
        let mut errors: Vec<_> = report.errors.iter().collect();
        errors.sort_by(|a, b| b.1.cmp(a.1));
        for (err, count) in errors.iter().take(10) {
            println!("  {:>4} × {}", count, err);
        }
    }

    if let Some(path) = json_out {
        std::fs::write(&path, serde_json::to_vec_pretty(&rows)?).with_context(|| format!("writing {}", path))?;
        println!("\nPer-token breakdown written to {}", path);
    }
    Ok(())
}

/// Distribution of how many blocks after the copied trade our buys land (live) or could earliest
/// land (paper). The backtest only found an edge when buys land in the same block.
fn latency_report(path: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path} (written by the bot on each copy buy)"))?;
    let recs: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    println!("⏱️ Copy-buy latency from {path}");
    for (label, live) in [("PAPER (earliest slot we could land in)", false), ("LIVE (slot we landed in)", true)] {
        let rows: Vec<&serde_json::Value> = recs.iter().filter(|r| r["landed"].as_bool() == Some(live)).collect();
        if rows.is_empty() {
            continue;
        }
        let mut buckets = [0usize; 5];
        for r in &rows {
            let b = r["blocks"].as_i64().unwrap_or(99).clamp(0, 4) as usize;
            buckets[b] += 1;
        }
        let mut ms: Vec<u64> = rows.iter().filter_map(|r| r["ms"].as_u64()).collect();
        ms.sort_unstable();
        let pct = |p: usize| ms.get((ms.len() * p / 100).min(ms.len().saturating_sub(1))).copied().unwrap_or(0);
        println!("\n{label}: {} buys", rows.len());
        for (i, name) in ["same block", "+1 block", "+2 blocks", "+3 blocks", "+4 or more"].iter().enumerate() {
            let n = buckets[i];
            println!("   {:<11} {:>5}  {:>5.1}%  {}", name, n, 100.0 * n as f64 / rows.len() as f64, "█".repeat(40 * n / rows.len()));
        }
        println!("   detection → send: median {} ms, 90th percentile {} ms", pct(50), pct(90));
        let failed = rows.iter().filter(|r| r["ok"].as_bool() == Some(false)).count();
        if failed > 0 {
            println!("   {} of {} failed (price moved past our slippage before we landed)", failed, rows.len());
        }
    }
    println!("\nBacktest: copying only made money when buys landed in the same block (+1 block already lost).");
    Ok(())
}
