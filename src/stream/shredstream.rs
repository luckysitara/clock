use anyhow::Result;
use log::info;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{
    config::BotConfig,
    engine::executor::TradeAction,
};

pub struct ShredStreamReceiver {
    _config: Arc<BotConfig>,
    _trade_sender: mpsc::Sender<TradeAction>,
}

impl ShredStreamReceiver {
    pub fn new(config: Arc<BotConfig>, trade_sender: mpsc::Sender<TradeAction>) -> Self {
        Self {
            _config: config,
            _trade_sender: trade_sender,
        }
    }

    /// Connect to local ShredStream Proxy on port 7777 (or configured proxy port)
    pub async fn run_loop(&self, proxy_addr: &str) -> Result<()> {
        info!(
            "📡 ShredStream Receiver configured for local proxy at: {}",
            proxy_addr
        );
        info!("ℹ️ Run 'jito-shredstream-proxy' on localhost to feed raw UDP shreds into this receiver");
        Ok(())
    }
}
