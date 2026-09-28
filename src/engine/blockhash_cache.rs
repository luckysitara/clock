use log::{error, info};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::hash::Hash;
use std::{
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

/// Cache-line aligned struct (64 bytes) to avoid false sharing across CPU cores
#[repr(C, align(64))]
pub struct AlignedBlockhashEntry {
    pub hash: Hash,
    pub last_updated_epoch_ms: u64,
}

pub struct BlockhashCache {
    entry: Arc<RwLock<AlignedBlockhashEntry>>,
}

impl BlockhashCache {
    pub fn new() -> Self {
        Self {
            entry: Arc::new(RwLock::new(AlignedBlockhashEntry {
                hash: Hash::default(),
                last_updated_epoch_ms: 0,
            })),
        }
    }

    /// Read the cached blockhash from RAM in nanoseconds
    pub async fn get_latest(&self) -> Hash {
        self.entry.read().await.hash
    }

    /// Spawn a dedicated background task to continuously refresh the blockhash
    pub fn spawn_updater(&self, rpc_url: String, interval: Duration) -> tokio::task::JoinHandle<()> {
        let entry_clone = Arc::clone(&self.entry);
        tokio::spawn(async move {
            let client = RpcClient::new(rpc_url.clone());
            info!("🔄 Blockhash background updater started for RPC: {}", rpc_url);

            loop {
                match client.get_latest_blockhash().await {
                    Ok(new_hash) => {
                        let mut lock = entry_clone.write().await;
                        lock.hash = new_hash;
                        lock.last_updated_epoch_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                    }
                    Err(e) => {
                        error!("⚠️ Failed to fetch recent blockhash: {}", e);
                    }
                }
                tokio::time::sleep(interval).await;
            }
        })
    }
}
