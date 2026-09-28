use anyhow::Result;
use futures::StreamExt;
use std::{collections::HashMap, time::{Duration, Instant}};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};
use clockit::constants::{CREATE_DISCRIMINATOR_BYTES, PUMPFUN_PROGRAM};

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    let endpoint = std::env::var("YELLOWSTONE_GRPC_URL")?;
    let x_token = std::env::var("YELLOWSTONE_X_TOKEN").ok();

    let mut builder = GeyserGrpcClient::build_from_shared(endpoint.clone())?;
    if endpoint.starts_with("https://") {
        builder = builder.tls_config(yellowstone_grpc_client::ClientTlsConfig::new().with_enabled_roots())?;
    }
    if let Some(token) = &x_token {
        builder = builder.x_token(Some(token.clone()))?;
    }
    let mut client = builder.connect().await?;

    let mut transactions = HashMap::new();
    transactions.insert(
        "pumpfun_creates".to_string(),
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
        accounts: HashMap::new(),
        slots: HashMap::new(),
        transactions,
        transactions_status: HashMap::new(),
        entry: HashMap::new(),
        blocks: HashMap::new(),
        blocks_meta: HashMap::new(),
        commitment: Some(CommitmentLevel::Processed as i32),
        accounts_data_slice: vec![],
        ping: None,
        from_slot: None,
    };

    let (_sink, mut stream) = client.subscribe_with_request(Some(request)).await?;
    println!("Listening for Pump.fun creates for 30 seconds...");

    let start = Instant::now();
    let mut top_creates = 0;
    let mut inner_creates = 0;
    let mut total_tx = 0;

    while start.elapsed() < Duration::from_secs(30) {
        if let Some(Ok(msg)) = stream.next().await {
            if let Some(UpdateOneof::Transaction(tx_info)) = msg.update_oneof {
                total_tx += 1;
                let sig = tx_info.transaction.as_ref().map(|t| bs58::encode(&t.signature).into_string()).unwrap_or_default();

                if let Some(tx) = tx_info.transaction.as_ref().and_then(|t| t.transaction.as_ref()) {
                    if let Some(msg_data) = tx.message.as_ref() {
                        for (ix_idx, ix) in msg_data.instructions.iter().enumerate() {
                            if ix.data.starts_with(&CREATE_DISCRIMINATOR_BYTES) {
                                top_creates += 1;
                                println!("⚡ TOP-LEVEL CREATE found! Sig: {} | Ix: {} | Data len: {} | Accounts: {}",
                                    sig, ix_idx, ix.data.len(), ix.accounts.len());
                            }
                        }
                    }
                }

                if let Some(meta) = tx_info.transaction.as_ref().and_then(|t| t.meta.as_ref()) {
                    for inner_set in &meta.inner_instructions {
                        for (inner_idx, inner_ix) in inner_set.instructions.iter().enumerate() {
                            if inner_ix.data.starts_with(&CREATE_DISCRIMINATOR_BYTES) {
                                inner_creates += 1;
                                println!("⚡ INNER CREATE found! Sig: {} | InnerSet: {} | InnerIx: {} | Data len: {} | Accounts: {}",
                                    sig, inner_set.index, inner_idx, inner_ix.data.len(), inner_ix.accounts.len());
                            }
                        }
                    }
                }
            }
        }
    }

    println!("\nSummary across {} transactions in 30s:", total_tx);
    println!("  Top-level creates: {}", top_creates);
    println!("  Inner creates:     {}", inner_creates);

    Ok(())
}
