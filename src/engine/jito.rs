use anyhow::{Context, Result};
use log::{debug, error, info};
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

pub struct JitoClient {
    client: Client,
    block_engine_url: String,
    tip_wallet_index: AtomicUsize,
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
        }
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

    /// Send bundle to Jito Block Engine
    pub async fn send_bundle(&self, tx: &Transaction) -> Result<String> {
        let serialized = bincode::serialize(tx).context("Failed to serialize transaction")?;
        let base58_tx = bs58::encode(serialized).into_string();

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[base58_tx]]
        });

        debug!("🚀 Dispatching bundle to Jito Block Engine: {}", self.block_engine_url);

        let response = self
            .client
            .post(&self.block_engine_url)
            .json(&payload)
            .send()
            .await
            .context("Failed to send bundle HTTP request to Jito")?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status.is_success() {
            info!("✅ Jito bundle accepted: {}", body);
            Ok(body)
        } else {
            error!("❌ Jito bundle rejected with status {}: {}", status, body);
            anyhow::bail!("Jito bundle rejected (HTTP {}): {}", status, body)
        }
    }
}
