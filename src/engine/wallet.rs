use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::{
    constants::{derive_associated_user_token, derive_bonding_curve, spl_token_2022_program_id, spl_token_program_id},
    decoders::BondingCurveAccountPod,
};

/// A non-zero token balance held by a wallet
#[derive(Debug, Clone, Copy)]
pub struct TokenHolding {
    pub mint: Pubkey,
    pub amount: u64,
    pub token_program: Pubkey,
}

async fn rpc_call(client: &Client, rpc_url: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let json: serde_json::Value = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("{} request failed", method))?
        .json()
        .await
        .with_context(|| format!("{} returned invalid JSON", method))?;
    if let Some(err) = json.get("error") {
        anyhow::bail!("{} RPC error: {}", method, err);
    }
    Ok(json)
}

/// All non-zero SPL Token and Token-2022 balances owned by `wallet`
pub async fn fetch_token_holdings(client: &Client, rpc_url: &str, wallet: &Pubkey) -> Result<Vec<TokenHolding>> {
    let mut holdings = Vec::new();
    for token_program in [spl_token_program_id(), spl_token_2022_program_id()] {
        let json = rpc_call(
            client,
            rpc_url,
            "getTokenAccountsByOwner",
            serde_json::json!([
                wallet.to_string(),
                {"programId": token_program.to_string()},
                {"encoding": "jsonParsed"}
            ]),
        )
        .await?;

        let accounts = json["result"]["value"].as_array().cloned().unwrap_or_default();
        for acc in accounts {
            let info = &acc["account"]["data"]["parsed"]["info"];
            let amount: u64 = info["tokenAmount"]["amount"].as_str().unwrap_or("0").parse().unwrap_or(0);
            if amount == 0 {
                continue;
            }
            if let Ok(mint) = Pubkey::from_str(info["mint"].as_str().unwrap_or_default()) {
                holdings.push(TokenHolding { mint, amount, token_program });
            }
        }
    }
    Ok(holdings)
}

/// Pump.fun bonding curve state for `mint` (None if the account doesn't exist or isn't a curve)
pub async fn fetch_bonding_curve(client: &Client, rpc_url: &str, mint: &Pubkey) -> Result<Option<BondingCurveAccountPod>> {
    let (bonding_curve, _) = derive_bonding_curve(mint);
    let json = rpc_call(
        client,
        rpc_url,
        "getAccountInfo",
        serde_json::json!([bonding_curve.to_string(), {"encoding": "base64"}]),
    )
    .await?;
    let Some(b64) = json["result"]["value"]["data"][0].as_str() else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("Invalid base64 curve data")?;
    Ok(BondingCurveAccountPod::read_from_account(&bytes))
}

/// Token balance of `owner`'s associated token account for `mint`.
/// Some(0) if the account doesn't exist, None if the RPC call itself failed.
pub async fn fetch_token_balance(
    client: &Client,
    rpc_url: &str,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Option<u64> {
    let (ata, _) = derive_associated_user_token(owner, mint, token_program);
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountBalance",
        "params": [ata.to_string(), {"commitment": "processed"}],
    });
    let json: serde_json::Value = client.post(rpc_url).json(&payload).send().await.ok()?.json().await.ok()?;
    if json.get("error").is_some() {
        // RPC reports a missing token account as an error
        return Some(0);
    }
    json["result"]["value"]["amount"].as_str()?.parse().ok()
}

/// A token account owned by the wallet whose balance is zero (closing it refunds its rent)
#[derive(Debug, Clone, Copy)]
pub struct EmptyTokenAccount {
    pub account: Pubkey,
    pub mint: Pubkey,
    pub token_program: Pubkey,
    pub rent_lamports: u64,
}

/// All zero-balance SPL Token and Token-2022 accounts owned by `wallet`
pub async fn fetch_empty_token_accounts(client: &Client, rpc_url: &str, wallet: &Pubkey) -> Result<Vec<EmptyTokenAccount>> {
    let mut empty = Vec::new();
    for token_program in [spl_token_program_id(), spl_token_2022_program_id()] {
        let json = rpc_call(
            client,
            rpc_url,
            "getTokenAccountsByOwner",
            serde_json::json!([
                wallet.to_string(),
                {"programId": token_program.to_string()},
                {"encoding": "jsonParsed"}
            ]),
        )
        .await?;

        for acc in json["result"]["value"].as_array().cloned().unwrap_or_default() {
            let info = &acc["account"]["data"]["parsed"]["info"];
            let amount = info["tokenAmount"]["amount"].as_str().unwrap_or("1");
            // Token-2022 accounts holding withheld transfer fees can't be closed
            let withheld = info["extensions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|e| e["state"]["withheldAmount"].as_u64().unwrap_or(0) > 0);
            if amount != "0" || withheld {
                continue;
            }
            let (Ok(account), Ok(mint)) = (
                Pubkey::from_str(acc["pubkey"].as_str().unwrap_or_default()),
                Pubkey::from_str(info["mint"].as_str().unwrap_or_default()),
            ) else {
                continue;
            };
            empty.push(EmptyTokenAccount {
                account,
                mint,
                token_program,
                rent_lamports: acc["account"]["lamports"].as_u64().unwrap_or(0),
            });
        }
    }
    Ok(empty)
}

/// Whether `owner`'s associated token account for `mint` exists and is empty.
/// None if the RPC call failed.
pub async fn ata_is_empty(
    client: &Client,
    rpc_url: &str,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Option<bool> {
    let (ata, _) = derive_associated_user_token(owner, mint, token_program);
    let json = rpc_call(
        client,
        rpc_url,
        "getAccountInfo",
        serde_json::json!([ata.to_string(), {"encoding": "jsonParsed", "commitment": "confirmed"}]),
    )
    .await
    .ok()?;
    let value = &json["result"]["value"];
    if value.is_null() {
        return Some(false);
    }
    Some(value["data"]["parsed"]["info"]["tokenAmount"]["amount"].as_str() == Some("0"))
}
