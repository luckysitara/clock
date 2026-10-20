use anyhow::{Context, Result};
use base64::Engine;
use log::{info, warn};
use reqwest::Client;
use serde_json::json;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::constants::{create_transfer_instruction, JITO_TIP_WALLETS};

pub const JITO_REGIONAL_ENDPOINTS: &[&str] = &[
    "https://mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://amsterdam.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://tokyo.mainnet.block-engine.jito.wtf/api/v1/bundles",
    "https://slc.mainnet.block-engine.jito.wtf/api/v1/bundles",
];

pub struct JitoClient {
    client: Client,
    block_engine_url: String,
    tip_wallet_index: AtomicUsize,
    last_send: std::sync::Arc<tokio::sync::Mutex<std::time::Instant>>,
}

impl JitoClient {
    pub fn new(block_engine_url: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_millis(3000))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            block_engine_url,
            tip_wallet_index: AtomicUsize::new(0),
            last_send: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(5),
            )),
        }
    }

    /// Keeps HTTP connections hot and open to primary block engine
    pub fn spawn_prewarmer(&self) -> tokio::task::JoinHandle<()> {
        let client = self.client.clone();
        let ep = self.block_engine_url.clone();
        tokio::spawn(async move {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getTipAccounts",
                "params": []
            });
            loop {
                let _ = client.post(&ep).json(&payload).send().await;
                tokio::time::sleep(std::time::Duration::from_secs(45)).await;
            }
        })
    }

    /// Selects the next Jito tip account in round-robin fashion
    pub fn get_tip_wallet(&self) -> Pubkey {
        let idx = self.tip_wallet_index.fetch_add(1, Ordering::Relaxed) % JITO_TIP_WALLETS.len();
        Pubkey::from_str(JITO_TIP_WALLETS[idx]).unwrap()
    }

    /// Creates an atomic transaction containing custom instructions + Jito tip transfer
    pub fn build_tip_bundle(
        &self,
        payer: &Keypair,
        mut instructions: Vec<Instruction>,
        tip_lamports: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let tip_wallet = self.get_tip_wallet();
        let tip_ix = create_transfer_instruction(&payer.pubkey(), &tip_wallet, tip_lamports);
        instructions.push(tip_ix);

        let message = Message::new(&instructions, Some(&payer.pubkey()));
        Transaction::new(&[payer], message, recent_blockhash)
    }

    /// Send bundle to Jito Block Engine adhering to strict rate limits
    pub async fn send_bundle(&self, tx: &Transaction) -> Result<String> {
        // Enforce strict 1,200ms spacing between bundle dispatches (Jito limit: 1 req/sec)
        {
            let mut last = self.last_send.lock().await;
            let elapsed = last.elapsed();
            if elapsed < std::time::Duration::from_millis(1200) {
                let wait = std::time::Duration::from_millis(1200) - elapsed;
                tokio::time::sleep(wait).await;
            }
            *last = std::time::Instant::now();
        }

        let serialized = bincode::serialize(tx).context("Failed to serialize transaction")?;
        info!("📦 Jito bundle transaction size: {} bytes (MTU limit: 1232 bytes)", serialized.len());
        let base64_tx = base64::prelude::BASE64_STANDARD.encode(&serialized);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[base64_tx], { "encoding": "base64" }]
        });

        // 1. Dispatch to primary block engine endpoint
        let res = self.client.post(&self.block_engine_url).json(&payload).send().await;
        match res {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    info!("✅ Jito bundle broadcast confirmed: {}", body);
                    return Ok(body);
                } else if status.as_u16() == 429 {
                    warn!("⚠️ Jito 429 Rate Limit from {}. Backing off...", self.block_engine_url);
                    anyhow::bail!("Jito rate limit 429 from primary: {}", body);
                } else {
                    warn!("Primary Jito error ({}): {}. Trying regional backup...", status, body);
                }
            }
            Err(e) => {
                warn!("Primary Jito network error: {}. Trying regional backup...", e);
            }
        }

        // 2. Single backup endpoint (Frankfurt or Amsterdam)
        let backup_url = if self.block_engine_url.contains("amsterdam") {
            "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/bundles"
        } else {
            "https://amsterdam.mainnet.block-engine.jito.wtf/api/v1/bundles"
        };

        let backup_res = self.client.post(backup_url).json(&payload).send().await;
        match backup_res {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    info!("✅ Jito bundle confirmed via backup: {}", body);
                    return Ok(body);
                } else {
                    anyhow::bail!("Backup Jito error ({}): {}", status, body);
                }
            }
            Err(e) => {
                anyhow::bail!("Backup Jito network error: {}", e);
            }
        }
    }
}
