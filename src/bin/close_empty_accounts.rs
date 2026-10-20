//! Close the trader wallet's empty token accounts to recover their rent (~0.002 SOL each).
//!
//! Usage: close_empty_accounts            list what would be closed (no transactions)
//!        close_empty_accounts --execute  close them

use anyhow::{Context, Result};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{commitment_config::CommitmentConfig, message::Message, signer::Signer, transaction::Transaction};

use clockit::{
    config::BotConfig,
    engine::{instruction_builder::InstructionBuilder, wallet::fetch_empty_token_accounts},
};

/// Close instructions per transaction (well under the 1232-byte limit)
const CLOSES_PER_TX: usize = 10;

#[tokio::main]
async fn main() -> Result<()> {
    let execute = std::env::args().any(|a| a == "--execute");
    let config = BotConfig::load_from_env().context("Failed to load config")?;
    let keypair = config.load_keypair().context("Failed to load keypair")?;
    let wallet = keypair.pubkey();
    let http = reqwest::Client::new();

    println!("🔑 Wallet: {}", wallet);
    let empty = fetch_empty_token_accounts(&http, &config.solana_rpc_url, &wallet).await?;
    if empty.is_empty() {
        println!("✅ No empty token accounts, nothing to recover.");
        return Ok(());
    }

    let total_rent: u64 = empty.iter().map(|a| a.rent_lamports).sum();
    for a in &empty {
        println!("   {}  mint {}  rent {:.5} SOL", a.account, a.mint, a.rent_lamports as f64 / 1e9);
    }
    println!("Found {} empty token accounts holding {:.5} SOL of rent", empty.len(), total_rent as f64 / 1e9);

    if !execute {
        println!("Dry run only. Re-run with --execute to close them and recover the SOL.");
        return Ok(());
    }

    let rpc = RpcClient::new_with_commitment(config.solana_rpc_url.clone(), CommitmentConfig::confirmed());
    let mut recovered = 0u64;
    for chunk in empty.chunks(CLOSES_PER_TX) {
        let mut ixs = vec![
            InstructionBuilder::set_compute_unit_limit(5_000 * chunk.len() as u32 + 1_000),
            InstructionBuilder::set_compute_unit_price(10_000),
        ];
        ixs.extend(chunk.iter().map(|a| InstructionBuilder::close_ata(&a.account, &wallet, &wallet, &a.token_program)));
        let blockhash = rpc.get_latest_blockhash().await.context("Failed to get blockhash")?;
        let tx = Transaction::new(&[keypair.as_ref()], Message::new(&ixs, Some(&wallet)), blockhash);
        match rpc.send_and_confirm_transaction(&tx).await {
            Ok(sig) => {
                let rent: u64 = chunk.iter().map(|a| a.rent_lamports).sum();
                recovered += rent;
                println!("🧹 Closed {} accounts (+{:.5} SOL): https://solscan.io/tx/{}", chunk.len(), rent as f64 / 1e9, sig);
            }
            Err(e) => println!("❌ Failed to close batch of {}: {}", chunk.len(), e),
        }
    }
    println!("💰 Recovered {:.5} SOL of {:.5} SOL", recovered as f64 / 1e9, total_rent as f64 / 1e9);
    Ok(())
}
