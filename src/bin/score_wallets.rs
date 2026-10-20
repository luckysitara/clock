//! Rank the copy-trade target wallets by their own realized Pump.fun results.
//!
//! Usage: score_wallets [--limit N] [--parallel N] [--min-trades N] [--min-pf X]
//!   --limit       recent transactions scanned per wallet (default 300)
//!   --parallel    wallets analysed at once (default 4)
//!   --min-trades  closed Pump.fun trades needed to judge a wallet (default 8)
//!   --min-pf      profit factor needed to keep a wallet (default 1.2)
//!
//! Writes data/wallet_scores.json and data/smart_money_wallets_scored.txt (the keepers,
//! comma-separated, ready for TARGET_WALLETS). Nothing is changed in .env.

use anyhow::{Context, Result};
use futures::{stream, StreamExt};
use serde::Serialize;

use clockit::{
    analytics::{analyze_wallet, summarize},
    config::BotConfig,
};

#[derive(Serialize)]
struct Score {
    wallet: String,
    closed_trades: usize,
    wins: usize,
    win_rate: f64,
    profit_factor: f64,
    realized_sol: f64,
    return_pct: f64,
    keep: bool,
    note: String,
}

fn arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let limit: usize = arg(&args, "--limit", 300);
    let parallel: usize = arg(&args, "--parallel", 4);
    let min_trades: usize = arg(&args, "--min-trades", 8);
    let min_pf: f64 = arg(&args, "--min-pf", 1.2);

    let config = BotConfig::load_from_env().context("Failed to load config")?;
    let wallets = config.target_wallets.clone();
    anyhow::ensure!(!wallets.is_empty(), "No target wallets configured (TARGET_WALLETS or data/smart_money_wallets.txt)");
    println!("🐋 Scoring {} target wallets on their last {} transactions each...", wallets.len(), limit);

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
    let rpc_url = config.solana_rpc_url.clone();
    let total = wallets.len();
    let done = std::sync::atomic::AtomicUsize::new(0);

    let mut scores: Vec<Score> = stream::iter(wallets)
        .map(|wallet| {
            let (client, rpc_url, done) = (&client, &rpc_url, &done);
            async move {
                let result = analyze_wallet(client, rpc_url, &wallet, limit, 4).await;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                eprint!("\r   {}/{} analysed", n, total);
                match result {
                    Ok(report) => {
                        // Only bonding-curve trades are what the bot copies
                        let s = summarize(report.mints.values().filter(|m| m.venue == "pump.fun"));
                        let enough = s.closed >= min_trades;
                        let keep = enough && s.profit_factor() >= min_pf && s.realized > 0.0;
                        Score {
                            wallet,
                            closed_trades: s.closed,
                            wins: s.wins,
                            win_rate: s.win_rate(),
                            profit_factor: if s.profit_factor().is_finite() { s.profit_factor() } else { 99.0 },
                            realized_sol: s.realized,
                            return_pct: s.return_pct(),
                            keep,
                            note: if enough { String::new() } else { format!("only {} closed pump.fun trades in sample", s.closed) },
                        }
                    }
                    Err(e) => Score {
                        wallet,
                        closed_trades: 0,
                        wins: 0,
                        win_rate: 0.0,
                        profit_factor: 0.0,
                        realized_sol: 0.0,
                        return_pct: 0.0,
                        keep: false,
                        note: format!("analysis failed: {:#}", e),
                    },
                }
            }
        })
        .buffer_unordered(parallel.max(1))
        .collect()
        .await;
    eprintln!();

    scores.sort_by(|a, b| {
        b.keep
            .cmp(&a.keep)
            .then(b.profit_factor.partial_cmp(&a.profit_factor).unwrap_or(std::cmp::Ordering::Equal))
            .then(b.realized_sol.partial_cmp(&a.realized_sol).unwrap_or(std::cmp::Ordering::Equal))
    });

    println!("\n{:<46} {:>6} {:>6} {:>7} {:>10} {:>8}  {}", "WALLET", "TRADES", "WIN%", "PF", "PNL SOL", "RET%", "VERDICT");
    for s in &scores {
        println!(
            "{:<46} {:>6} {:>5.0}% {:>7.2} {:>+10.3} {:>+7.1}%  {}{}",
            s.wallet, s.closed_trades, s.win_rate * 100.0, s.profit_factor, s.realized_sol, s.return_pct,
            if s.keep { "KEEP" } else { "drop" },
            if s.note.is_empty() { String::new() } else { format!(" ({})", s.note) }
        );
    }

    let keepers: Vec<&str> = scores.iter().filter(|s| s.keep).map(|s| s.wallet.as_str()).collect();
    std::fs::create_dir_all("data")?;
    std::fs::write("data/wallet_scores.json", serde_json::to_vec_pretty(&scores)?)?;
    std::fs::write("data/smart_money_wallets_scored.txt", keepers.join(","))?;
    println!(
        "\n✅ {} of {} wallets pass (≥{} closed trades, profit factor ≥{}, positive P&L).",
        keepers.len(), scores.len(), min_trades, min_pf
    );
    println!("   Wrote data/wallet_scores.json and data/smart_money_wallets_scored.txt");
    println!("   To use them: set TARGET_WALLETS to the contents of data/smart_money_wallets_scored.txt");
    println!("   Note: a wallet's own P&L is an upper bound; copying it later, at a worse price, earns less.");
    Ok(())
}
