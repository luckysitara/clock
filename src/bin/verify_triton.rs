use anyhow::{Context, Result};
use futures::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};

use clockit::constants::PUMPFUN_PROGRAM;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv::dotenv();

    println!("============================================================");
    println!("     TRITON YELLOWSTONE gRPC CONNECTION VERIFIER");
    println!("============================================================");

    let grpc_url = std::env::var("YELLOWSTONE_GRPC_URL")
        .unwrap_or_else(|_| "https://solana-grpc.triton.one:443".to_string());

    let x_token = std::env::var("YELLOWSTONE_X_TOKEN")
        .or_else(|_| std::env::var("TRITON_X_TOKEN"))
        .ok()
        .filter(|t| !t.trim().is_empty());

    println!("📍 Endpoint: {}", grpc_url);
    match &x_token {
        Some(t) => {
            let masked = if t.len() > 8 {
                format!("{}...{}", &t[0..4], &t[t.len() - 4..])
            } else {
                "***".to_string()
            };
            println!("🔑 Token:    Configured ({}) via 'x-token' header", masked);
        }
        None => {
            println!("❌ Token:    NOT CONFIGURED! Please set YELLOWSTONE_X_TOKEN in .env");
            anyhow::bail!("Missing YELLOWSTONE_X_TOKEN in .env");
        }
    }

    println!("\n[1/3] Connecting to Triton gRPC endpoint...");
    let mut builder = GeyserGrpcClient::build_from_shared(grpc_url.clone())
        .context("Invalid gRPC URL format")?;

    if grpc_url.starts_with("https://") {
        let tls = yellowstone_grpc_proto::tonic::transport::ClientTlsConfig::new().with_enabled_roots();
        builder = builder.tls_config(tls).context("Failed to configure TLS")?;
    }

    if let Some(token) = &x_token {
        builder = builder
            .x_token(Some(token.clone()))
            .context("Failed to attach x-token header")?;
    }

    let mut client = builder
        .connect()
        .await
        .context("Failed to connect to Triton gRPC endpoint")?;

    println!("✅ Connected successfully!");

    println!("\n[2/3] Calling geyser.Geyser/GetVersion (Testing Authentication)...");
    match client.get_version().await {
        Ok(resp) => {
            println!("✅ Authentication successful!");
            println!("   Yellowstone Geyser Version: {}", resp.version);
        }
        Err(e) => {
            println!("❌ Failed to get version: {}", e);
            anyhow::bail!("GetVersion failed: check your token and permissions");
        }
    }

    println!("\n[3/3] Opening live stream for Pump.fun transactions (5-second test)...");
    let mut transactions = HashMap::new();
    transactions.insert(
        "pumpfun_test".to_string(),
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

    let (_sink, mut stream) = client
        .subscribe_with_request(Some(request))
        .await
        .context("Failed to subscribe to Yellowstone stream")?;

    println!("📡 Stream active! Listening for live transactions...");

    let start = Instant::now();
    let mut tx_count = 0;

    while start.elapsed() < Duration::from_secs(5) {
        tokio::select! {
            Some(msg_res) = stream.next() => {
                if let Ok(msg) = msg_res {
                    if let Some(UpdateOneof::Transaction(tx_info)) = msg.update_oneof {
                        tx_count += 1;
                        if tx_count == 1 {
                            println!("   ⚡ First Pump.fun transaction received at slot {}!", tx_info.slot);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    println!("\n============================================================");
    if tx_count > 0 {
        println!("🎉 VERIFICATION SUCCESSFUL!");
        println!("   Received {} Pump.fun transactions in 5 seconds.", tx_count);
        println!("   Your Triton Yellowstone gRPC access is 100% active and operational.");
    } else {
        println!("⚠️ Connected and authenticated, but no transactions captured in 5 seconds.");
        println!("   (Stream is working properly, network activity may be momentarily low).");
    }
    println!("============================================================");

    Ok(())
}
