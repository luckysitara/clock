use anyhow::Result;
use futures::StreamExt;
use std::{collections::HashMap, time::{Duration, Instant}};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};
use clockit::constants::{
    BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_U64, PUMPFUN_PROGRAM, SELL_DISCRIMINATOR_ALT_U64,
    SELL_DISCRIMINATOR_U64,
};
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
    let listen = Duration::from_secs(10);
    println!("Listening for {} seconds...", listen.as_secs());

    let pumpfun_bytes = clockit::constants::pumpfun_program_id().to_bytes();
    let start = Instant::now();
    let mut total_tx = 0;
    let mut create_count = 0;
    let mut buy_count = 0;
    let mut sell_count = 0;
    let mut inner_creates = 0;
    let mut inner_buys = 0;
    let mut inner_sells = 0;

    let mut disc_counts: HashMap<u64, usize> = HashMap::new();

    while start.elapsed() < listen {
        let msg = match tokio::time::timeout(listen.saturating_sub(start.elapsed()), stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => {
                eprintln!("Stream error: {}", e);
                break;
            }
            Ok(None) | Err(_) => break,
        };
        let Some(UpdateOneof::Transaction(tx_update)) = msg.update_oneof else { continue };
        let Some(tx_info) = tx_update.transaction.as_ref() else { continue };
        let Some(msg_data) = tx_info.transaction.as_ref().and_then(|t| t.message.as_ref()) else { continue };
        total_tx += 1;

        // Static keys followed by lookup-table addresses, matching v0 account indexing
        let mut account_keys: Vec<&[u8]> = msg_data.account_keys.iter().map(|k| k.as_slice()).collect();
        if let Some(meta) = tx_info.meta.as_ref() {
            account_keys.extend(meta.loaded_writable_addresses.iter().map(|k| k.as_slice()));
            account_keys.extend(meta.loaded_readonly_addresses.iter().map(|k| k.as_slice()));
        }
        let is_pumpfun = |idx: u32| account_keys.get(idx as usize) == Some(&&pumpfun_bytes[..]);

        for ix in &msg_data.instructions {
            if ix.data.len() >= 8 {
                let disc = u64::from_le_bytes(ix.data[0..8].try_into().unwrap());
                *disc_counts.entry(disc).or_insert(0) += 1;
            }
            if !is_pumpfun(ix.program_id_index) {
                continue;
            }
            if fast_is_discriminator(&ix.data, CREATE_DISCRIMINATOR_U64) {
                create_count += 1;
            } else if fast_is_discriminator(&ix.data, BUY_DISCRIMINATOR_U64) {
                buy_count += 1;
            } else if fast_is_discriminator(&ix.data, SELL_DISCRIMINATOR_U64)
                || fast_is_discriminator(&ix.data, SELL_DISCRIMINATOR_ALT_U64)
            {
                sell_count += 1;
            }
        }

        if let Some(meta) = tx_info.meta.as_ref() {
            for inner_ix in meta.inner_instructions.iter().flat_map(|inner| &inner.instructions) {
                if !is_pumpfun(inner_ix.program_id_index) {
                    continue;
                }
                if fast_is_discriminator(&inner_ix.data, CREATE_DISCRIMINATOR_U64) {
                    inner_creates += 1;
                } else if fast_is_discriminator(&inner_ix.data, BUY_DISCRIMINATOR_U64) {
                    inner_buys += 1;
                } else if fast_is_discriminator(&inner_ix.data, SELL_DISCRIMINATOR_U64)
                    || fast_is_discriminator(&inner_ix.data, SELL_DISCRIMINATOR_ALT_U64)
                {
                    inner_sells += 1;
                }
            }
        }
    }

    println!("In {:.1}s captured {} transactions.", start.elapsed().as_secs_f64(), total_tx);
    println!("Pump.fun top-level: {} create | {} buy | {} sell", create_count, buy_count, sell_count);
    println!("Pump.fun inner CPI: {} create | {} buy | {} sell", inner_creates, inner_buys, inner_sells);
    println!("Unique top-level discriminators:");
    for (disc, count) in disc_counts.iter() {
        let bytes = disc.to_le_bytes();
        println!("  Disc: {:?} (u64: {}) -> count: {}", bytes, disc, count);
    }

    Ok(())
}
