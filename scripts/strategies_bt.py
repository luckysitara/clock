"""More no-racing strategies on the cached tapes, with the same exits, fees and walk-forward
split as scripts/momentum_bt.py:

  consensus  buy when K good wallets bought the token within WINDOW s, entering DELAY s late.
             "Good" wallets are picked on the FIRST half only (their own realized results),
             so the second half is out of sample.
  dip        on tokens with >= BUYERS distinct buyers so far, buy after the price falls DROP
             from its high (curve still has >= RSOL SOL).
  postgrad   buy on PumpSwap DELAY s after graduation if the pool price rose RISE since the
             first pool trade; exit with take profit / stop / trailing / max hold on pool prices.

Usage: python3 scripts/strategies_bt.py [consensus|dip|postgrad|all] [LATENCY_SLOTS]
"""
import glob, itertools, json, os, sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
ARGS = sys.argv[1:]
WHICH = ARGS[0] if ARGS else "all"
sys.argv = sys.argv[:1]  # momentum_bt reads its own arguments on import
import momentum_bt as m  # noqa: E402

m.LAT = int(ARGS[1]) if len(ARGS) > 1 else 2
ROOT = m.ROOT


def exit_from(tape, j, entry_cost_state, ex):
    """Buy SIZE before tape[j] (state tape[j-1]) and exit with ex = (TP, HS, TR, HOLD)"""
    TP, HS, TR, HOLD = ex
    n = len(tape)
    creator = tape[0]["creator"]
    state = tape[j - 1]
    cost = m.SIZE * (1 + m.FEE)
    tokens = m.buy_tokens(state, m.SIZE)
    entry = m.SIZE * 1e9 / tokens
    hi, entry_ts = entry, state["ts"]
    exit_at = None
    for k in range(j, n):
        u = tape[k]
        px = m.price(u)
        hi = max(hi, px)
        if u["rtok"] == 0:
            got = m.pool_exit(u["mint"], u["slot"], entry, hi, entry_ts, tokens, TP, HS, TR, HOLD)
            if got is None:
                return None
            return (entry_ts, got - cost - 2 * m.SIDE_COST, "graduated")
        if (px >= entry * TP or px <= entry * (1 - HS) or (hi > entry and px <= hi * (1 - TR))
                or u["ts"] - entry_ts >= HOLD or (not u["is_buy"] and u["user"] == creator)):
            exit_at = k
            break
    if exit_at is None:
        exit_at = n - 1
    q = exit_at
    while q + 1 < n and tape[q + 1]["slot"] < tape[exit_at]["slot"] + m.LAT:
        q += 1
    got = m.sell_sol(tape[q], tokens) * (1 - m.FEE)
    return (entry_ts, got - cost - 2 * m.SIDE_COST, "curve")


def fill_index(tape, i, delay_s):
    """First trade at least LAT slots and delay_s seconds after tape[i]"""
    t = tape[i]
    j = i + 1
    while j < len(tape) and (tape[j]["slot"] < t["slot"] + m.LAT or tape[j]["ts"] < t["ts"] + delay_s):
        j += 1
    return j if j < len(tape) else None


EXITS = list(itertools.product([1.3, 1.5, 2.5], [0.15, 0.25], [0.15, 0.30], [120, 600]))


def walk_forward(name, entries, first, second, min_trades=20):
    """entries: list of (label, fn(ti, tape) -> fill index or None)"""
    rows = []
    for label, fn in entries:
        cache = {}
        for ex in EXITS:
            def res(sample):
                out = []
                for ti, t in sample:
                    if ti not in cache:
                        cache[ti] = fn(ti, t)
                    j = cache[ti]
                    if j is not None:
                        r = exit_from(t, j, None, ex)
                        if r:
                            out.append(r)
                return out
            a = m.stats(res(first))
            if a[0] >= min_trades:
                rows.append((a, label, ex, res))
    rows.sort(key=lambda r: -r[0][2])
    print(f"\n=== {name}: {len(rows)} settings with >= {min_trades} trades in the 1st half; top 8 (picked on 1st half) ===")
    print("   entry                               TP   HS   TR HOLD |  1st: n  win    P&L    PF |  2nd: n  win    P&L    PF")
    for a, label, ex, res in rows[:8]:
        b = m.stats(res(second))
        print(f"   {label:<34} {ex[0]:>4} {ex[1]:>4} {ex[2]:>4} {ex[3]:>4} | {a[0]:>5} {a[1]*100:3.0f}% {a[2]:+.3f} {a[3]:5.2f}"
              f" | {b[0]:>5} {b[1]*100:3.0f}% {b[2]:+.3f} {b[3]:5.2f}")
    both = sum(1 for a, *_ , res in rows if a[2] > 0 and m.stats(res(second))[2] > 0)
    print(f"   settings profitable in BOTH halves: {both} of {len(rows)}")
    if rows:
        b = m.stats(rows[0][3](second))
        return rows[0][1], rows[0][2], rows[0][0], b
    return None


# ───────────── consensus ─────────────

def good_wallets(until_ts, since_ts):
    """Target wallets whose own realized results in [since, until) were good"""
    out = []
    for f in glob.glob(f"{ROOT}/wallets/*.json"):
        w = f.rsplit("/", 1)[-1][:-5]
        c = json.load(open(f))
        by = defaultdict(lambda: [0, 0, 0, 0])
        trades = sorted((t for v in c["txs"].values() for t in v
                         if (t["user"] == w or t["fee_payer"] == w) and since_ts <= t["ts"] < until_ts), key=lambda t: t["slot"])
        own_created = 0
        for t in trades:
            if t["is_buy"]:
                e = by[t["mint"]]; e[0] += t["sol"] * (1 + m.FEE); e[2] += t["tokens"]
                own_created += t["creator"] == w
            elif t["mint"] in by:
                e = by[t["mint"]]; e[1] += t["sol"] * (1 - m.FEE); e[3] += t["tokens"]
        closed = [(p - c_) for c_, p, b, s in by.values() if b and s * 100 >= b * 95]
        gain = sum(x for x in closed if x > 0); loss = -sum(x for x in closed if x < 0)
        pf = gain / loss if loss else (99 if gain else 0)
        if len(closed) >= 10 and sum(closed) > 0 and pf >= 1.5 and own_created == 0:
            out.append(w)
    return set(out)


def consensus_entries(good):
    entries = []
    for K, WINDOW, DELAY in itertools.product([1, 2, 3], [60, 300], [0, 10, 30]):
        def fn(ti, tape, K=K, WINDOW=WINDOW, DELAY=DELAY):
            seen = {}
            for i, t in enumerate(tape):
                if t["is_buy"] and t["sol"] >= 1e8 and t["user"] in good and t["user"] != t["creator"]:
                    seen[t["user"]] = t["ts"]
                    if sum(1 for ts in seen.values() if t["ts"] - ts <= WINDOW) >= K and t["rtok"] > 0:
                        return fill_index(tape, i, DELAY)
            return None
        entries.append((f"K={K} window={WINDOW}s delay={DELAY}s", fn))
    return entries


# ───────────── dip ─────────────

def dip_entries():
    entries = []
    for BUYERS, DROP, RSOL in itertools.product([20, 50], [0.3, 0.5], [5, 20]):
        def fn(ti, tape, BUYERS=BUYERS, DROP=DROP, RSOL=RSOL):
            hi, buyers = 0, set()
            for i, t in enumerate(tape):
                px = m.price(t)
                hi = max(hi, px)
                if t["is_buy"]:
                    buyers.add(t["user"])
                if len(buyers) >= BUYERS and px <= hi * (1 - DROP) and t["rsol"] / 1e9 >= RSOL and t["rtok"] > 0:
                    return fill_index(tape, i, 0)
            return None
        entries.append((f"buyers>={BUYERS} drop {DROP:.0%} rsol>={RSOL}", fn))
    return entries


# ───────────── postgrad ─────────────

def postgrad_test(tapes_by_mint, mid):
    """Buy on PumpSwap DELAY s after graduation if the price rose RISE from the first pool trade"""
    rows = []
    for DELAY, RISE, (TP, HS, TR, HOLD) in itertools.product([5, 30, 120], [-1.0, 0.0, 0.2], EXITS):
        res = {0: [], 1: []}
        for mint, pg in m.POSTGRAD.items():
            pts = pg.get("points")
            if not pts:
                continue
            half = 0 if pg["grad_ts"] < mid else 1
            t0 = pts[0][1]
            i = next((k for k, p in enumerate(pts) if p[1] >= t0 + DELAY), None)
            if i is None or (pts[i][2] / pts[i][3]) < (pts[0][2] / pts[0][3]) * (1 + RISE):
                continue
            j = i
            while j + 1 < len(pts) and pts[j + 1][0] < pts[i][0] + m.LAT:
                j += 1
            q, b = pts[j][2], pts[j][3]
            sol_in = m.SIZE * 1e9 * (1 - m.POOL_FEE)
            tokens = b - q * b / (q + sol_in)
            entry = m.SIZE * 1e9 / tokens
            hi, ets = entry, pts[j][1]
            got = None
            for k in range(j + 1, len(pts)):
                px = pts[k][2] / pts[k][3]
                hi = max(hi, px)
                if px >= entry * TP or px <= entry * (1 - HS) or (hi > entry and px <= hi * (1 - TR)) or pts[k][1] - ets >= HOLD or k == len(pts) - 1:
                    e = k
                    while e + 1 < len(pts) and pts[e + 1][0] < pts[k][0] + m.LAT:
                        e += 1
                    q2, b2 = pts[e][2], pts[e][3]
                    got = (q2 - q2 * b2 / (b2 + tokens)) / 1e9 * (1 - m.POOL_FEE)
                    break
            if got is not None:
                res[half].append((ets, got - m.SIZE - 2 * m.SIDE_COST, "pool"))
        a, b = m.stats(res[0]), m.stats(res[1])
        if a[0] >= 5:
            rows.append((a, b, DELAY, RISE, (TP, HS, TR, HOLD)))
    rows.sort(key=lambda r: -r[0][2])
    print(f"\n=== postgrad: {len(m.POSTGRAD)} graduated tokens with data; top 8 (picked on 1st half) ===")
    print("   delay rise   TP   HS   TR HOLD |  1st: n  win    P&L    PF |  2nd: n  win    P&L    PF")
    for a, b, d, r, ex in rows[:8]:
        print(f"   {d:>5} {r:>+4.0%} {ex[0]:>4} {ex[1]:>4} {ex[2]:>4} {ex[3]:>4} | {a[0]:>5} {a[1]*100:3.0f}% {a[2]:+.3f} {a[3]:5.2f}"
              f" | {b[0]:>5} {b[1]*100:3.0f}% {b[2]:+.3f} {b[3]:5.2f}")
    print(f"   settings profitable in BOTH halves: {sum(1 for a, b, *_ in rows if a[2] > 0 and b[2] > 0)} of {len(rows)}")


if __name__ == "__main__":
    tapes = m.load(f"{ROOT}/mints")
    m.load_postgrad(f"{ROOT}/postgrad")
    idx, first, second = m.split(tapes)
    mid = min(t[0]["ts"] for _, t in second)
    start = min(t[0]["ts"] for t in tapes)
    print(f"{len(tapes)} tapes, latency {m.LAT} slots, 1st half before ts {mid}")
    if WHICH in ("consensus", "all"):
        good = good_wallets(mid, start - 86400)
        print(f"consensus: {len(good)} good wallets picked on the 1st half only: {', '.join(sorted(w[:6] for w in good))}")
        walk_forward("consensus", consensus_entries(good), first, second)
    if WHICH in ("dip", "all"):
        walk_forward("dip", dip_entries(), first, second)
    if WHICH in ("postgrad", "all"):
        postgrad_test(None, mid)
