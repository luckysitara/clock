"""Test creator ('dev') skill persistence from recorded launches (data/creates.jsonl).

Reads every launch record, looks up each token's outcome (did it graduate?), then does the
walk-forward test: rank creators by their EARLIER launches' graduation rate, and check whether
their LATER launches graduate above the baseline rate. If they don't, dev skill doesn't persist
and the creator-sniper idea has no edge.

Outcome source: Pump.fun API `complete` flag per mint, with a DexScreener "has a DEX pool"
fallback (graduated tokens migrate to PumpSwap/Raydium). Needs only the public internet.

Run this after record_creates has collected a week or two of launches.
Usage: python3 scripts/creator_outcomes.py [--split-ts T] [--min-early 2]
"""
import json, sys, time, urllib.request

ROOT = "/home/rootkit/pump/clockit"
LOG = f"{ROOT}/data/creates.jsonl"


def arg(name, default, cast=int):
    return cast(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default


def get(url, headers=None):
    for i in range(4):
        try:
            req = urllib.request.Request(url, headers=headers or {"User-Agent": "clockit-research"})
            with urllib.request.urlopen(req, timeout=20) as r:
                return json.load(r)
        except Exception:
            time.sleep(1 + i)
    return None


def graduated(mint):
    """True/False/None — did this token graduate off the bonding curve?"""
    d = get(f"https://frontend-api-v3.pump.fun/coins/{mint}", {"Origin": "https://pump.fun", "User-Agent": "Mozilla/5.0"})
    if isinstance(d, dict) and d.get("complete") is not None:
        return bool(d["complete"])
    # fallback: graduated tokens get a PumpSwap/Raydium pool
    p = get(f"https://api.dexscreener.com/token-pairs/v1/solana/{mint}")
    if isinstance(p, list):
        return any((x.get("dexId") in ("pumpswap", "raydium", "meteora")) for x in p)
    return None


def main():
    rows = [json.loads(l) for l in open(LOG) if l.strip()]
    if not rows:
        print(f"{LOG} is empty — let record_creates run first.")
        return
    ts = sorted(r["ts"] for r in rows)
    split = arg("--split-ts", ts[len(ts) // 2])
    min_early = arg("--min-early", 2)
    print(f"{len(rows)} launches recorded, {len({r['creator'] for r in rows})} creators, "
          f"span {time.strftime('%m-%d %H:%M', time.gmtime(ts[0]))} → {time.strftime('%m-%d %H:%M', time.gmtime(ts[-1]))}")

    # outcome per mint (cached to disk so reruns don't re-fetch)
    cache_path = f"{ROOT}/data/creates_outcomes.json"
    try:
        outcome = json.load(open(cache_path))
    except Exception:
        outcome = {}
    todo = [r["mint"] for r in rows if r["mint"] not in outcome]
    print(f"looking up {len(todo)} new token outcomes…")
    for n, mint in enumerate(todo, 1):
        outcome[mint] = graduated(mint)
        if n % 50 == 0:
            print(f"   {n}/{len(todo)}")
            json.dump(outcome, open(cache_path, "w"))
        time.sleep(0.1)
    json.dump(outcome, open(cache_path, "w"))

    from collections import defaultdict
    by = defaultdict(lambda: {"early": [], "late": []})
    for r in rows:
        g = outcome.get(r["mint"])
        if g is None:
            continue
        by[r["creator"]]["early" if r["ts"] < split else "late"].append(1 if g else 0)

    all_late = [g for c in by.values() for g in c["late"]]
    base = sum(all_late) / len(all_late) if all_late else 0
    print(f"\n================ CREATOR SKILL PERSISTENCE ================")
    print(f"baseline late-period graduation rate: {sum(all_late)}/{len(all_late)} = {base*100:.2f}%")
    for thr_grad in (1, 2):
        picked = {c: v for c, v in by.items() if len(v["early"]) >= min_early and sum(v["early"]) >= thr_grad}
        late = [g for v in picked.values() for g in v["late"]]
        rate = sum(late) / len(late) if late else 0
        lift = rate / base if base else 0
        print(f"creators with >= {min_early} early launches AND >= {thr_grad} early graduation(s) "
              f"({len(picked)} creators): later grad {sum(late)}/{len(late)} = {rate*100:.2f}%  ({lift:.2f}x baseline)")
    print("\nIf picked creators' later rate isn't clearly above baseline, dev skill does not persist.")


if __name__ == "__main__":
    main()
