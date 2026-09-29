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

    /// Send bundle to Jito Block Engine in parallel across all regional relayers
    pub async fn send_bundle(&self, tx: &Transaction) -> Result<String> {
        let serialized = bincode::serialize(tx).context("Failed to serialize transaction")?;
        let base58_tx = bs58::encode(serialized).into_string();

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[base58_tx]]
        });

        // Collect all target endpoints including primary block_engine_url
        let mut endpoints = vec![self.block_engine_url.as_str()];
        for &ep in JITO_REGIONAL_ENDPOINTS {
            if ep != self.block_engine_url.as_str() {
                endpoints.push(ep);
            }
        }

        debug!("🚀 Dispatching bundle to {} Jito Block Engines in parallel...", endpoints.len());

        let mut futures = Vec::new();
        for &ep in &endpoints {
            let client = self.client.clone();
            let payload = payload.clone();
            let ep_str = ep.to_string();
            futures.push(tokio::spawn(async move {
                let res = client.post(&ep_str).json(&payload).send().await;
                (ep_str, res)
            }));
        }

        let mut successful_body: Option<String> = None;
        let mut last_err = String::new();

        for fut in futures {
            if let Ok((ep, Ok(resp))) = fut.await {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    debug!("✅ Accepted by {}", ep);
                    if successful_body.is_none() {
                        successful_body = Some(body);
                    }
                } else {
                    last_err = format!("HTTP {} from {}: {}", status, ep, body);
                }
            }
        }

        if let Some(body) = successful_body {
            info!("✅ Jito bundle broadcast confirmed: {}", body);
            Ok(body)
        } else {
            error!("❌ All Jito block engines rejected bundle. Last err: {}", last_err);
            anyhow::bail!("All Jito block engines rejected bundle: {}", last_err)
        }
    }
}
