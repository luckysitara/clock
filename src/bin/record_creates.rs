//! Record every Pump.fun token creation from the live gRPC stream, to study creator ('dev')
//! skill over time. One JSON line per launch in data/creates.jsonl:
//!   {mint, creator, bonding_curve, slot, ts, symbol, name, dev_buy_sol}
//!
//! This is the forward data-collection half of the creator-sniper idea: let it run for days,
//! then scripts/creator_outcomes.py reads each bonding curve's `complete` flag on-chain and
//! tests whether creators who graduated tokens before keep graduating them (persistence).
//!
//! It only observes — it sends nothing on-chain. Reads YELLOWSTONE_GRPC_URL / YELLOWSTONE_X_TOKEN.
//!
//! Usage: record_creates [--secs N]   (default: run until stopped)

use anyhow::{Context, Result};
use futures::StreamExt;
use std::{
    collections::HashMap,
    io::Write,
    time::{Duration, Instant},
};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterTransactions,
};

use clockit::{
    constants::{BUY_DISCRIMINATOR_U64, CREATE_DISCRIMINATOR_U64, CREATE_V2_DISCRIMINATOR_U64},
    decoders::{fast_is_discriminator, metadata::CreateInstructionView},
};

const LOG: &str = "data/creates.jsonl";
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

fn arg_secs() -> Option<u64> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == "--secs").and_then(|i| a.get(i + 1)).and_then(|v| v.parse().ok())
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let endpoint = std::env::var("YELLOWSTONE_GRPC_URL").context("YELLOWSTONE_GRPC_URL not set")?;
    let x_token = std::env::var("YELLOWSTONE_X_TOKEN").ok().filter(|s| !s.is_empty());
    let deadline = arg_secs().map(|s| Instant::now() + Duration::from_secs(s));
    let pumpfun_bytes = clockit::constants::pumpfun_program_id().to_bytes();

    std::fs::create_dir_all("data").ok();
    let mut total = 0u64;
    let already = std::fs::read_to_string(LOG).map(|s| s.lines().count()).unwrap_or(0);
    log::info!("📝 Recording Pump.fun creates to {} ({} already logged). Observe-only.", LOG, already);

    loop {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break;
            }
        }
        match session(&endpoint, &x_token, &pumpfun_bytes, deadline, &mut total).await {
            Ok(_) => log::warn!("stream closed, reconnecting in 2s (logged {total} this run)"),
            Err(e) => log::error!("stream error: {e:#}. reconnecting in 2s"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    log::info!("done: logged {total} creates this run");
    Ok(())
}

async fn session(
    endpoint: &str,
    x_token: &Option<String>,
    pumpfun_bytes: &[u8; 32],
    deadline: Option<Instant>,
    total: &mut u64,
) -> Result<()> {
    let mut builder = GeyserGrpcClient::build_from_shared(endpoint.to_string())?;
    if endpoint.starts_with("https://") {
        builder = builder.tls_config(yellowstone_grpc_client::ClientTlsConfig::new().with_enabled_roots())?;
    }
    if let Some(token) = x_token {
        builder = builder.x_token(Some(token.clone()))?;
    }
    let mut client = builder.connect().await.context("connect")?;

    let mut transactions = HashMap::new();
    transactions.insert(
        "pf_creates".to_string(),
        SubscribeRequestFilterTransactions {
            vote: Some(false),
            failed: Some(false),
            signature: None,
            account_include: vec![clockit::constants::PUMPFUN_PROGRAM.to_string()],
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
        commitment: Some(CommitmentLevel::Confirmed as i32),
        accounts_data_slice: vec![],
        ping: None,
        from_slot: None,
    };

    let (_sink, mut stream) = client.subscribe_with_request(Some(request)).await.context("subscribe")?;
    log::info!("✅ stream active, watching for creates");

    loop {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                return Ok(());
            }
        }
        let msg = match tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => anyhow::bail!("recv: {e}"),
            Ok(None) => return Ok(()),
            Err(_) => anyhow::bail!("no data for {}s", IDLE_TIMEOUT.as_secs()),
        };
        let Some(UpdateOneof::Transaction(tx_update)) = msg.update_oneof else { continue };
        let slot = tx_update.slot;
        let Some(tx_info) = tx_update.transaction.as_ref() else { continue };
        let Some(msg_data) = tx_info.transaction.as_ref().and_then(|t| t.message.as_ref()) else { continue };

        let mut keys: Vec<&[u8]> = msg_data.account_keys.iter().map(|k| k.as_slice()).collect();
        if let Some(meta) = tx_info.meta.as_ref() {
            keys.extend(meta.loaded_writable_addresses.iter().map(|k| k.as_slice()));
            keys.extend(meta.loaded_readonly_addresses.iter().map(|k| k.as_slice()));
        }
        let is_pf = |idx: u32| keys.get(idx as usize) == Some(&&pumpfun_bytes[..]);

        // Gather this tx's instructions (top-level + inner CPI), so we catch creates either way
        let mut dev_buy_lamports = 0u64;
        let mut create: Option<(String, usize, Vec<u8>)> = None; // (view-data b58? no: symbol/name/mint/bc)

        // fee-payer (first account) is the token's creator for both Create and CreateV2
        let fee_payer = keys.first().map(|k| bs58::encode(k).into_string()).unwrap_or_default();
        let mut handle = |data: &[u8], accts: &[u8]| {
            let is_create = fast_is_discriminator(data, CREATE_DISCRIMINATOR_U64)
                || fast_is_discriminator(data, CREATE_V2_DISCRIMINATOR_U64);
            if is_create {
                // Both Create and CreateV2 carry name/symbol/uri args and put the mint at account 0.
                let view = CreateInstructionView::parse(data);
                let (symbol, name) = view.map(|v| (v.symbol.to_string(), v.name.to_string())).unwrap_or_default();
                let mint = accts.first().and_then(|&i| keys.get(i as usize)).map(|k| bs58::encode(k).into_string());
                if let Some(mint) = mint {
                    create = Some((
                        serde_json::json!({
                            "mint": mint, "creator": fee_payer,
                            "symbol": symbol, "name": name,
                        })
                        .to_string(),
                        0,
                        Vec::new(),
                    ));
                }
            } else if fast_is_discriminator(data, BUY_DISCRIMINATOR_U64) && data.len() >= 24 {
                // the dev's first buy rides in the same tx; sol amount is the 2nd u64 arg (max_sol_cost)
                let sol = u64::from_le_bytes(data[16..24].try_into().unwrap());
                dev_buy_lamports = dev_buy_lamports.max(sol);
            }
        };

        for ix in &msg_data.instructions {
            if is_pf(ix.program_id_index) {
                handle(&ix.data, &ix.accounts);
            }
        }
        if let Some(meta) = tx_info.meta.as_ref() {
            for inner in meta.inner_instructions.iter().flat_map(|i| &i.instructions) {
                if is_pf(inner.program_id_index) {
                    handle(&inner.data, &inner.accounts);
                }
            }
        }

        if let Some((base, _, _)) = create {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            // splice slot/ts/dev_buy into the json object
            let line = format!(
                "{}\n",
                base.trim_end_matches('}').to_string()
                    + &format!(",\"slot\":{slot},\"ts\":{ts},\"dev_buy_sol\":{:.4}}}", dev_buy_lamports as f64 / 1e9)
            );
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(LOG) {
                let _ = f.write_all(line.as_bytes());
            }
            *total += 1;
            if *total % 50 == 0 {
                log::info!("… {total} creates recorded this run");
            }
        }
    }
}
