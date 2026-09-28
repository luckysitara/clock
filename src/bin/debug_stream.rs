use anyhow::{Context, Result};
use futures::StreamExt;
use std::{collections::HashMap, time::{Duration, Instant}};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};
use clockit::constants::{BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_U64, PUMPFUN_PROGRAM, SELL_DISCRIMINATOR_U64};
use clockit::decoders::fast_is_discriminator;

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
        "pumpfun_debug".to_string(),
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
    println!("Listening for 10 seconds...");

    let start = Instant::now();
    let mut total_tx = 0;
    let mut create_count = 0;
    let mut buy_count = 0;
    let mut sell_count = 0;
    let mut inner_creates = 0;

    let mut disc_counts: HashMap<u64, usize> = HashMap::new();

    while start.elapsed() < Duration::from_secs(5) {
        if let Some(Ok(msg)) = stream.next().await {
            if let Some(UpdateOneof::Transaction(tx_info)) = msg.update_oneof {
                total_tx += 1;
                if let Some(tx) = tx_info.transaction.as_ref().and_then(|t| t.transaction.as_ref()) {
                    if let Some(msg_data) = tx.message.as_ref() {
                        for ix in &msg_data.instructions {
                            if ix.data.len() >= 8 {
                                let disc = u64::from_le_bytes(ix.data[0..8].try_into().unwrap());
                                *disc_counts.entry(disc).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    println!("In 5 seconds captured {} transactions. Unique discriminators:", total_tx);
    for (disc, count) in disc_counts.iter() {
        let bytes = disc.to_le_bytes();
        println!("  Disc: {:?} (u64: {}) -> count: {}", bytes, disc, count);
    }

    Ok(())
}
