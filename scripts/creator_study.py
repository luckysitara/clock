"""Test whether Pump.fun creator ('dev') skill persists — the core of the creator-sniper idea.

Method (all on-chain + public feed, no special indexer):
  1. Seed a population of creator addresses from Pump.fun's recent-coins feed and our cache.
  2. For each creator, walk their transaction history and find every Pump.fun `create`
     instruction -> the full list of tokens they launched, with timestamps. The create tx
     names the token's bonding-curve account directly (no PDA derivation needed).
  3. Read each token's outcome ON-CHAIN: fetch the bonding-curve account and read `complete`
     (graduated yes/no). Graduation is the clean quality signal; ~1-2% of tokens graduate.
  4. Walk-forward persistence test: rank creators by their graduation rate in the EARLIER
     period, then measure whether their LATER tokens graduate more often than a random
     creator's. If later outcomes don't beat baseline, dev skill does NOT persist and the
     creator-sniper idea is dead.

RPC is read from HELIUS_RPC_URL then SOLANA_RPC_URL in .env, never printed.

Usage: python3 scripts/creator_study.py [--seed-coins 500] [--max-creators 300] [--split-ts T]
"""
import base64, glob, json, re, sys, time, urllib.request

ROOT = "/home/rootkit/pump/clockit"
PUMP = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
CREATE_DISC = bytes([24, 30, 200, 40, 5, 28, 7, 119])  # pump.fun create instruction
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def arg(name, default, cast=int):
    return cast(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default


def rpc_url():
    env = dict(l.split("=", 1) for l in open(f"{ROOT}/.env") if "=" in l and not l.startswith("#"))
    for k in ("HELIUS_RPC_URL", "SOLANA_RPC_URL"):
        v = env.get(k, "").strip().strip('"')
        if v:
            return v
    raise SystemExit("no RPC url in .env")


URL = rpc_url()
SEED_COINS = arg("--seed-coins", 500)
MAX_CREATORS = arg("--max-creators", 300)
SPLIT_TS = arg("--split-ts", 0)
MAX_PAGES = arg("--max-pages", 2)
SKIP_ACTIVE = arg("--skip-active", 4000)  # skip wallets with more sigs than this (hyperactive bots/traders)


def post(payload):
    for i in range(6):
        try:
            req = urllib.request.Request(URL, data=json.dumps(payload).encode(), headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=40) as r:
                return json.load(r)
        except Exception as e:
            last = re.sub(r"https?://\S+", "<url>", str(e))
            time.sleep(1 + i)
    print("   rpc failed:", last)
    return None


def feed(url):
    for i in range(4):
        try:
            req = urllib.request.Request(url, headers={"Origin": "https://pump.fun", "User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(req, timeout=20) as r:
                return json.load(r)
        except Exception:
            time.sleep(1 + i)
    return None


def b58enc(b):
    n = int.from_bytes(b, "big")
    s = ""
    while n:
        n, r = divmod(n, 58)
        s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s


def b58dec(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    return n.to_bytes(32, "big")


def seed_creators():
    """Creator addresses from the recent-coins feed plus our cached tapes"""
    creators = {}
    page = feed(f"https://frontend-api-v3.pump.fun/coins?offset=0&limit={min(SEED_COINS,500)}&sort=created_timestamp&order=DESC&includeNsfw=true")
    for c in page or []:
        if c.get("creator"):
            creators[c["creator"]] = creators.get(c["creator"], 0) + 1
    cache_creators = {}
    for f in glob.glob(f"{ROOT}/data/backtest/mints/*.json"):
        mint = f.rsplit("/", 1)[-1][:-5]
        h = json.load(open(f))
        done = False
        for v in h.get("txs", {}).values():
            for t in v:
                if t["mint"] == mint and t.get("creator"):
                    cache_creators[t["creator"]] = cache_creators.get(t["creator"], 0) + 1
                    done = True
                    break
            if done:
                break
    # cache creators first (they launched tokens real traders bought), then feed creators
    ordered = list(cache_creators) + [c for c in creators if c not in cache_creators]
    return ordered[:MAX_CREATORS]


def creator_launches(addr):
    """Every (mint, bonding_curve, ts) this address created on Pump.fun, via its tx history"""
    # peek: skip hyperactive wallets (heavy traders/bots), they aren't pure launchers
    peek = post({"jsonrpc":"2.0","id":1,"method":"getSignaturesForAddress","params":[addr,{"limit":1000}]})
    pk = (peek or {}).get("result") or []
    if len(pk) >= 1000:
        # one full page already; only proceed if within a couple pages, else skip as too active
        pass
    sigs, before = [], None
    for _ in range(MAX_PAGES):
        opts = {"limit": 1000}
        if before:
            opts["before"] = before
        r = post({"jsonrpc": "2.0", "id": 1, "method": "getSignaturesForAddress", "params": [addr, opts]})
        page = (r or {}).get("result") or []
        sigs += [(s["signature"], s.get("blockTime") or 0) for s in page if s.get("err") is None]
        if len(page) < 1000:
            break
        before = page[-1]["signature"]
    launches = []
    for i in range(0, len(sigs), 50):
        batch = [{"jsonrpc": "2.0", "id": j, "method": "getTransaction",
                  "params": [s[0], {"maxSupportedTransactionVersion": 1, "encoding": "json"}]}
                 for j, s in enumerate(sigs[i:i + 50])]
        res = post(batch) or []
        for r in res:
            tx = r.get("result")
            if not tx or (tx.get("meta") or {}).get("err"):
                continue
            msg = tx["transaction"]["message"]
            keys = msg["accountKeys"]
            try:
                pidx = keys.index(PUMP)
            except ValueError:
                continue
            for ix in msg["instructions"]:
                if ix.get("programIdIndex") != pidx:
                    continue
                raw = ix.get("data") or ""
                try:
                    data = b58dec_data(raw)
                except Exception:
                    continue
                if data[:8] == CREATE_DISC:
                    accts = ix["accounts"]
                    mint = keys[accts[0]]
                    bc = keys[accts[2]] if len(accts) > 2 else None
                    launches.append((mint, bc, tx.get("blockTime") or 0))
        time.sleep(0.1)
    return launches


def _is_b64(s):
    return bool(s) and s[-1] == "=" or (s and all(c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=" for c in s))


def b58dec_data(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return raw


def graduated(bc_accounts):
    """Map bonding-curve pubkey -> complete flag, read on-chain in batches"""
    out = {}
    uniq = [b for b in bc_accounts if b]
    for i in range(0, len(uniq), 100):
        chunk = uniq[i:i + 100]
        r = post({"jsonrpc": "2.0", "id": 1, "method": "getMultipleAccounts", "params": [chunk, {"encoding": "base64"}]})
        vals = ((r or {}).get("result") or {}).get("value") or [None] * len(chunk)
        for bc, v in zip(chunk, vals):
            if not v:
                out[bc] = None
                continue
            data = base64.b64decode(v["data"][0])
            # BondingCurveAccountPod: 8 disc + 5*u64(40) = complete bool at offset 48
            out[bc] = bool(data[48]) if len(data) > 48 else None
        time.sleep(0.1)
    return out


def main():
    creators = seed_creators()
    print(f"Seeded {len(creators)} candidate creators. Walking each one's launch history via {('HELIUS' if 'helius' in URL else 'RPC')}…")
    hist = {}
    for n, c in enumerate(creators, 1):
        launches = creator_launches(c)
        if launches:
            hist[c] = launches
        if n % 20 == 0:
            print(f"   {n}/{len(creators)} creators scanned, {sum(len(v) for v in hist.values())} launches found")
    all_bc = [bc for v in hist.values() for (_, bc, _) in v]
    grad = graduated(all_bc)
    # assemble per-creator timeline
    rows = []
    all_ts = sorted(ts for v in hist.values() for (_, _, ts) in v if ts)
    split = SPLIT_TS or (all_ts[len(all_ts) // 2] if all_ts else 0)
    for c, launches in hist.items():
        early = [(m, bc, ts) for (m, bc, ts) in launches if ts and ts < split]
        late = [(m, bc, ts) for (m, bc, ts) in launches if ts and ts >= split]
        rows.append({
            "creator": c,
            "total": len(launches),
            "early_n": len(early), "early_grad": sum(1 for _, bc, _ in early if grad.get(bc)),
            "late_n": len(late), "late_grad": sum(1 for _, bc, _ in late if grad.get(bc)),
        })
    json.dump(rows, open(f"{ROOT}/data/creator_study.json", "w"))

    base_n = sum(r["late_n"] for r in rows)
    base_g = sum(r["late_grad"] for r in rows)
    base = base_g / base_n if base_n else 0
    print("\n================ CREATOR SKILL PERSISTENCE ================")
    print(f"split at ts {split}; baseline LATE-period graduation rate: {base_g}/{base_n} = {base*100:.2f}%")
    for thr in (1, 2, 3):
        picked = [r for r in rows if r["early_n"] >= thr and r["early_grad"] >= 1]
        ln = sum(r["late_n"] for r in picked)
        lg = sum(r["late_grad"] for r in picked)
        rate = lg / ln if ln else 0
        lift = (rate / base) if base else 0
        print(f"creators with >= {thr} early launches AND >= 1 early graduation ({len(picked)} creators): "
              f"later grad {lg}/{ln} = {rate*100:.2f}%  ({lift:.2f}x baseline)")
    print("\nIf the picked rate isn't clearly above baseline, dev skill does not persist enough to snipe on.")


if __name__ == "__main__":
    main()
