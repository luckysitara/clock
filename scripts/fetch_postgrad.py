"""For each cached token tape that graduated, fetch its first PumpSwap trades after graduation
and record the pool price (from the pool's vault balances) after each one.

Writes data/backtest/postgrad/<mint>.json: {"grad_slot", "grad_ts", "points": [[slot, ts, sol_reserve, token_reserve], ...]}
The RPC URL is read from .env (SOLANA_RPC_URL) and never printed.
"""
import glob, json, os, re, sys, time, urllib.request

ROOT = "/home/rootkit/pump/clockit"
OUT = f"{ROOT}/data/backtest/postgrad"
WSOL = "So11111111111111111111111111111111111111112"
PUMPSWAP = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
MAX_SIG_PAGES = 60
MAX_POST_TXS = 400
POST_SECS = 3600


def rpc_url():
    for line in open(f"{ROOT}/.env"):
        if line.startswith("SOLANA_RPC_URL="):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("SOLANA_RPC_URL missing")


URL = rpc_url()


def call(payload, tries=6):
    data = json.dumps(payload).encode()
    for i in range(tries):
        try:
            req = urllib.request.Request(URL, data=data, headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except Exception as e:  # never print the URL
            msg = re.sub(r"https?://\S+", "<url>", str(e))
            time.sleep(1.5 * (i + 1))
            last = msg
    print("   request failed:", last)
    return None


def signatures_after(address, until_sig):
    """All signatures newer than until_sig, oldest first (None if too many pages)"""
    out, before = [], None
    for _ in range(MAX_SIG_PAGES):
        opts = {"limit": 1000, "until": until_sig}
        if before:
            opts["before"] = before
        r = call({"jsonrpc": "2.0", "id": 1, "method": "getSignaturesForAddress", "params": [address, opts]})
        if r is None or "result" not in r:
            raise ConnectionError("getSignaturesForAddress failed")
        page = r["result"] or []
        out.extend(page)
        if len(page) < 1000:
            return list(reversed(out))
        before = page[-1]["signature"]
    return None


def pool_point(tx, mint):
    meta = tx.get("meta") or {}
    if meta.get("err") is not None:
        return None
    keys = tx["transaction"]["message"]["accountKeys"]
    keys = [k if isinstance(k, str) else k.get("pubkey") for k in keys]
    keys += (meta.get("loadedAddresses") or {}).get("writable", []) + (meta.get("loadedAddresses") or {}).get("readonly", [])
    if PUMPSWAP not in keys:
        return None
    by_owner = {}
    for b in meta.get("postTokenBalances") or []:
        amt = int(b["uiTokenAmount"]["amount"])
        by_owner.setdefault(b.get("owner"), {})[b["mint"]] = amt
    best = None
    for owner, m in by_owner.items():
        if mint in m and WSOL in m and m[WSOL] > 0 and m[mint] > 0:
            if best is None or m[mint] > best[1]:
                best = (m[WSOL], m[mint])
    return best


def main():
    os.makedirs(OUT, exist_ok=True)
    todo = []
    for f in glob.glob(f"{ROOT}/data/backtest/mints/*.json"):
        mint = f.rsplit("/", 1)[-1][:-5]
        if os.path.exists(f"{OUT}/{mint}.json"):
            continue
        h = json.load(open(f))
        grads = [(s, t) for s, v in h.get("txs", {}).items() for t in v if t["mint"] == mint and t["rtok"] == 0]
        if grads:
            grads.sort(key=lambda x: x[1]["slot"])
            todo.append((mint, grads[0][0], grads[0][1]))
    # --shard i/n: fetch every n-th token (run n copies in parallel)
    if "--shard" in sys.argv:
        i, n_ = map(int, sys.argv[sys.argv.index("--shard") + 1].split("/"))
        todo = sorted(todo)[i::n_]
    print(f"{len(todo)} graduated tokens to fetch")
    for n, (mint, gsig, gt) in enumerate(todo, 1):
        try:
            sigs = signatures_after(mint, gsig)
        except ConnectionError:
            print(f"[{n}/{len(todo)}] {mint[:8]}… RPC unreachable, will retry on the next run")
            continue
        if sigs is None:
            print(f"[{n}/{len(todo)}] {mint[:8]}… too many transactions since graduation, skipped")
            json.dump({"grad_slot": gt["slot"], "grad_ts": gt["ts"], "points": None}, open(f"{OUT}/{mint}.json", "w"))
            continue
        sigs = [s for s in sigs if s.get("err") is None and (s.get("blockTime") or 0) <= gt["ts"] + POST_SECS][:MAX_POST_TXS]
        points, failed = [], False
        for i in range(0, len(sigs), 25):
            batch = [{"jsonrpc": "2.0", "id": j, "method": "getTransaction",
                      "params": [s["signature"], {"maxSupportedTransactionVersion": 1, "encoding": "json"}]}
                     for j, s in enumerate(sigs[i:i + 25])]
            res = call(batch)
            if res is None:
                failed = True
                break
            for r in sorted(res, key=lambda r: r.get("id", 0)):
                tx = r.get("result")
                if not tx:
                    continue
                pt = pool_point(tx, mint)
                if pt:
                    points.append([tx["slot"], tx.get("blockTime") or 0, pt[0], pt[1]])
            time.sleep(0.2)
        if failed:
            print(f"[{n}/{len(todo)}] {mint[:8]}… RPC unreachable, will retry on the next run")
            continue
        points.sort(key=lambda p: (p[0]))
        json.dump({"grad_slot": gt["slot"], "grad_ts": gt["ts"], "points": points}, open(f"{OUT}/{mint}.json", "w"))
        first = f", first pool {points[0][2]/1e9:.1f} SOL" if points else ""
        print(f"[{n}/{len(todo)}] {mint[:8]}… {len(sigs)} txs, {len(points)} pool prices{first}")


main()
