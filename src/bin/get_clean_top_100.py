import urllib.request
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from collections import defaultdict, Counter

RPC_URL = "https://graceful-oasis-a398.mainnet.rpcpool.com/95794a05-4057-49cb-87e4-42d3d9a46774"

KNOWN_EXCLUSIONS = {
    "11111111111111111111111111111111",
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
    "ComputeBudget111111111111111111111111111111",
    "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P", # Pump.fun
    "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1",
    "6CpipySaXTAiUKDyyhWeCfYSMQLCiHrbCN1WvRv5s8kF", # Fee vault
    "GijFWw4oNyh9ko3FaZforNsi3jk6wDovARpkKahPD4o5", # Fee vault
    "CyaE1VxvBrahnPWkqm5VsdCvyS2QmNht2UFrKJHga54o", # Fee vault
    "3bzaJd5yZG73EVDz8xosQb7gfZm2LN5auFGh6wnP1n1f", # Fee vault
    "4UwK5AE6Djdf3MfwtPGE8pFYD47MhU9fiStDVmSHJVMB", # Fee vault
    "DXbZugG3hkX6o2cbNuKnUtDTQdjRRUx9uvtoE9FQCc3i", # Fee vault
    "6i2aHtxfqkC2biTo98FSkP59FVHPKFRLZWDbdghN6WKK", # Fee vault
    "CAPn1yH4oSywsxGU456jfgTrSSUidf9jgeAnHceNUJdw", # Fee vault
    "J23qr98GjGJJqKq9CBEnyRhHbmkaVxtTJNNxKu597wsA", # Fee vault
    "8k1BPp8pCxq7RJxxBz3BUxvBjsfjhkHKnhr2WSQABGM9", # Fee router
    "82m59BvGrbCSKUXhuqdNXP7pSnYQEasLhWCek7zsbXpT", # Fee router
    "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ", # Pump fee program
}

def rpc_call(method, params, timeout=10):
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(RPC_URL, data=payload, headers={"Content-Type": "application/json"})
    try:
        resp = urllib.request.urlopen(req, timeout=timeout)
        return json.loads(resp.read().decode()).get("result")
    except Exception:
        return None

def main():
    if not os.path.exists("/tmp/graduated_curves.txt"):
        print("Missing /tmp/graduated_curves.txt")
        return

    with open("/tmp/graduated_curves.txt") as f:
        curves = [line.strip() for line in f if line.strip()]

    # Take all available graduated curves from today (up to 300)
    selected_curves = curves[:250]
    print(f"[*] Scanning {len(selected_curves)} graduated curves from today...")

    # Step 1: Fetch signatures for each curve in parallel
    curve_sigs = []
    def fetch_sigs(c):
        res = rpc_call("getSignaturesForAddress", [c, {"limit": 30}])
        if res:
            return [(c, item["signature"]) for item in res]
        return []

    with ThreadPoolExecutor(max_workers=35) as executor:
        futures = [executor.submit(fetch_sigs, c) for c in selected_curves]
        for f in as_completed(futures):
            curve_sigs.extend(f.result())

    print(f"[*] Collected {len(curve_sigs)} curve transactions. Filtering true smart traders...")

    # Step 2: Parse transactions in parallel
    wallet_volumes = defaultdict(float)
    wallet_wins = defaultdict(set)
    wallet_trades_count = Counter()
    wallet_max_buy = defaultdict(float)

    def parse_tx(item):
        curve, sig = item
        tx_data = rpc_call("getTransaction", [sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 1}], timeout=8)
        if not tx_data:
            return None
        logs = tx_data.get("meta", {}).get("logMessages", [])
        
        # Must be a Buy instruction
        if not any("Instruction: Buy" in l for l in logs):
            return None
            
        # EXCLUDE Dev single-bundle creations (CreateV2 + Buy) which are 1-time dev burners
        if any("Instruction: CreateV2" in l or "Instruction: Create" in l for l in logs):
            return None

        accs = tx_data.get("transaction", {}).get("message", {}).get("accountKeys", [])
        signer = accs[0]["pubkey"] if accs else None
        if not signer or signer in KNOWN_EXCLUSIONS:
            return None
            
        pre = tx_data.get("meta", {}).get("preBalances", [0])[0]
        post = tx_data.get("meta", {}).get("postBalances", [0])[0]
        sol_spent = (pre - post) / 1e9
        
        # Only include real trader buys: between 0.1 SOL and 75.0 SOL
        if 0.10 <= sol_spent <= 75.0:
            return (signer, curve, sol_spent)
        return None

    with ThreadPoolExecutor(max_workers=45) as executor:
        futures = [executor.submit(parse_tx, item) for item in curve_sigs]
        for f in as_completed(futures):
            res = f.result()
            if res:
                signer, curve, sol_spent = res
                wallet_volumes[signer] += sol_spent
                wallet_wins[signer].add(curve)
                wallet_trades_count[signer] += 1
                if sol_spent > wallet_max_buy[signer]:
                    wallet_max_buy[signer] = sol_spent

    # Step 3: Rank genuine traders
    # Ranking Formula: (Wins * 20.0) + (Total Volume * 1.5) + (Trades Count * 2.0)
    # This prioritizes traders who win repeatedly across different graduated tokens, while rewarding volume.
    ranked = []
    for w in wallet_volumes:
        wins = len(wallet_wins[w])
        vol = wallet_volumes[w]
        trades = wallet_trades_count[w]
        max_b = wallet_max_buy[w]
        score = (wins * 25.0) + (vol * 2.0) + (trades * 3.0)
        ranked.append({
            "score": score,
            "wins": wins,
            "volume": vol,
            "trades": trades,
            "max_buy": max_b,
            "address": w
        })

    ranked.sort(key=lambda x: (x["wins"], x["score"]), reverse=True)

    top_100 = ranked[:100]

    output_path = "/home/rootkit/pump/clockit/top_100_clean_traders.json"
    with open(output_path, "w") as out:
        json.dump({
            "generated_at_utc": "2026-09-29T14:50:00Z",
            "total_ranked": len(ranked),
            "top_100": [t["address"] for t in top_100],
            "details": [
                {
                    "rank": i + 1,
                    "address": t["address"],
                    "graduated_tokens_won": t["wins"],
                    "total_sol_deployed": round(t["volume"], 4),
                    "max_single_buy_sol": round(t["max_buy"], 4),
                    "total_trades": t["trades"]
                }
                for i, t in enumerate(top_100)
            ]
        }, out, indent=2)

    print(f"[*] Processed {len(ranked)} unique traders. Top 100 saved to {output_path}")

if __name__ == "__main__":
    main()
