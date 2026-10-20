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

    while start.elapsed() < Duration::from_secs(20) {
        if let Some(Ok(msg)) = stream.next().await {
            if let Some(UpdateOneof::Transaction(tx_info)) = msg.update_oneof {
                total_tx += 1;
                if let Some(tx) = tx_info.transaction.as_ref().and_then(|t| t.transaction.as_ref()) {
                    if let Some(msg_data) = tx.message.as_ref() {
                        let mut all_ixs: Vec<(&Vec<u8>, &Vec<u8>)> = msg_data.instructions.iter().map(|i| (&i.data, &i.accounts)).collect();
                        if let Some(meta) = tx_info.transaction.as_ref().and_then(|t| t.meta.as_ref()) {
                            for inner_set in &meta.inner_instructions {
                                for inner in &inner_set.instructions {
                                    all_ixs.push((&inner.data, &inner.accounts));
                                }
                            }
                        }
                        for (data, accounts) in all_ixs {
                            if data.len() >= 8 {
                                let disc = u64::from_le_bytes(data[0..8].try_into().unwrap());
                                if disc == BUY_DISCRIMINATOR_U64 {
                                    println!("\n🎯 LIVE PUMP.FUN BUY INSTRUCTION FOUND!");
                                    println!("Data len: {} bytes: {:?}", data.len(), data);
                                    println!("Accounts count: {}", accounts.len());
                                    let mut all_keys: Vec<Vec<u8>> = msg_data.account_keys.clone();
                                    if let Some(meta) = tx_info.transaction.as_ref().and_then(|t| t.meta.as_ref()) {
                                        all_keys.extend(meta.loaded_writable_addresses.iter().cloned());
                                        all_keys.extend(meta.loaded_readonly_addresses.iter().cloned());
                                    }
                                    for (idx, acc_idx) in accounts.iter().enumerate() {
                                        if (*acc_idx as usize) < all_keys.len() {
                                            let pubkey_bytes = &all_keys[*acc_idx as usize];
                                            let pk = solana_sdk::pubkey::Pubkey::new_from_array(pubkey_bytes.as_slice().try_into().unwrap());
                                            println!("  [{}] Account idx {}: {}", idx, acc_idx, pk);
                                        } else {
                                            println!("  [{}] Account idx {}: <out of bounds>", idx, acc_idx);
                                        }
                                    }
                                    return Ok(());
                                }
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
