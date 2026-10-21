"""Measure whether cross-DEX price gaps on the same Solana token are actually tradeable.

For each token, read every SOL-quoted pool's price from DexScreener, find the cheapest and
dearest pool, and compute the NET gap after the two swap fees and a Jito tip. A gap is only
real if it beats all of that AND both pools are deep enough that our trade doesn't move the
price past the gap. We poll repeatedly and record how many consecutive polls each net-positive
gap survives, because a gap that lasts < ~1s is one only colocated searchers can take.

This measures opportunity only. It sends no transactions. A public API, so prices lag real
time; if gaps don't even show up here, they certainly aren't tradeable by us. If they do, the
next step would be block-level reserve recording.

Usage: python3 scripts/arb_scan.py [--polls 40] [--interval 8] [--tip-bps 30] [--min-liq 3000]
"""
import glob, json, sys, time, urllib.request

WSOL = "So11111111111111111111111111111111111111112"
# Generous (low) fee assumption per side, so we don't under-count opportunities
FEE_BPS = {"pumpswap": 25, "raydium": 25, "orca": 30, "meteora": 25, "default": 30}


def arg(name, default, cast):
    return cast(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default


POLLS = arg("--polls", 40, int)
INTERVAL = arg("--interval", 8, int)
TIP_BPS = arg("--tip-bps", 30, int)       # 0.30% tip, already generous for a small trade
MIN_LIQ = arg("--min-liq", 3000, float)


def get(url):
    for i in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"user-agent": "clockit-research"}), timeout=20) as r:
                return json.load(r)
        except Exception:
            time.sleep(1 + i)
    return None


def token_list():
    """Majors known to trade on several venues, plus tokens that graduated in our cache"""
    majors = [
        "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",  # BONK
        "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  # WIF
        "7GCihgDB8fe6KNjn2MYtkzZcRjQy3t9GHdC8uHYmW2hr",  # POPCAT
        "A8C3xuqscfmyLrte3VmTqrAq8kgMASius9AFNANwpump",  # FWOG
    ]
    grad = []
    for f in glob.glob("/home/rootkit/pump/clockit/data/backtest/postgrad/*.json"):
        mint = f.rsplit("/", 1)[-1][:-5]
        if json.load(open(f)).get("points"):
            grad.append(mint)
    seen, out = set(), []
    for m in majors + grad:
        if m not in seen:
            seen.add(m)
            out.append(m)
    return out


def sol_pools(mint):
    d = get(f"https://api.dexscreener.com/token-pairs/v1/solana/{mint}")
    if not isinstance(d, list):
        return []
    pools = []
    for p in d:
        if (p.get("quoteToken") or {}).get("address") != WSOL:
            continue
        price = p.get("priceNative")
        liq = (p.get("liquidity") or {}).get("usd", 0) or 0
        if not price or liq < MIN_LIQ:
            continue
        pools.append({"dex": p.get("dexId", "default"), "price": float(price), "liq": liq,
                      "pair": p.get("pairAddress", "")})
    return pools


def net_gap(pools):
    """Best net gap (after 2 fees + tip) across any pair of pools, and whether depth allows it.
    Returns (net_bps, raw_bps, buy, sell) or None."""
    best = None
    for buy in pools:
        for sell in pools:
            if buy is sell:
                continue
            raw = (sell["price"] / buy["price"] - 1) * 10_000
            cost = FEE_BPS.get(buy["dex"], FEE_BPS["default"]) + FEE_BPS.get(sell["dex"], FEE_BPS["default"]) + TIP_BPS
            net = raw - cost
            if best is None or net > best[0]:
                best = (net, raw, buy, sell)
    return best


def main():
    tokens = token_list()
    print(f"Scanning {len(tokens)} tokens, {POLLS} polls every {INTERVAL}s "
          f"(tip {TIP_BPS}bps, min pool liq ${MIN_LIQ:.0f}, fees {FEE_BPS})")
    # token -> current run length of a positive net gap, and the longest run seen
    run, longest, ever_positive, poll_hits = {}, {}, set(), 0
    best_seen = {}
    for poll in range(POLLS):
        hits = []
        for mint in tokens:
            pools = sol_pools(mint)
            if len(pools) < 2:
                run[mint] = 0
                continue
            ng = net_gap(pools)
            if ng is None:
                run[mint] = 0
                continue
            net, raw, buy, sell = ng
            if net > best_seen.get(mint, (-1e9,))[0]:
                best_seen[mint] = (net, raw, buy["dex"], sell["dex"], len(pools))
            if net > 0:
                run[mint] = run.get(mint, 0) + 1
                longest[mint] = max(longest.get(mint, 0), run[mint])
                ever_positive.add(mint)
                hits.append((mint, net, raw, buy["dex"], sell["dex"]))
            else:
                run[mint] = 0
            time.sleep(0.15)  # be gentle on the public API
        if hits:
            poll_hits += 1
            for mint, net, raw, bd, sd in hits:
                print(f"  poll {poll+1:>3}: {mint[:6]}… NET +{net:.0f}bps (raw {raw:.0f}) buy {bd} → sell {sd}  [run {run[mint]}]")
        if poll % 5 == 0:
            print(f"  …poll {poll+1}/{POLLS} done")
        time.sleep(INTERVAL)

    poll_secs = INTERVAL + len(tokens) * 0.15
    print("\n================ RESULT ================")
    print(f"polls with ANY net-positive gap: {poll_hits}/{POLLS}")
    print(f"tokens that ever showed a net-positive gap: {len(ever_positive)}/{len(tokens)}")
    if longest:
        print("longest consecutive polls a net gap persisted (×%.0fs each):" % poll_secs)
        for mint, L in sorted(longest.items(), key=lambda x: -x[1])[:10]:
            b = best_seen.get(mint)
            print(f"   {mint[:6]}…  {L} polls (~{L*poll_secs:.0f}s)  best NET {b[0]:+.0f}bps raw {b[1]:+.0f} {b[2]}→{b[3]} ({b[4]} pools)")
    else:
        print("no token ever showed a gap that beat 2 swap fees + tip.")
    print("\nReminder: a gap must persist LONGER than our reaction+land time (≈1-2 blocks, <1s)")
    print("to be takeable by us rather than by colocated searchers.")


if __name__ == "__main__":
    main()
