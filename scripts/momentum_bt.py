"""Momentum entry backtest on cached Pump.fun token tapes (data/backtest/mints/*.json).

Entry: in the last WIN seconds, net SOL inflow >= INFLOW and distinct buyers >= BUYERS,
with real SOL in the curve between RSOL_MIN and RSOL_MAX. Fills LAT slots after the signal
at the curve state then (no racing). Exits: take profit, hard stop, trailing stop, max hold,
creator sell; each fills LAT slots later. A token that graduates while held is sold on
PumpSwap with the same rules, at real pool prices (data/backtest/postgrad, fetched by
scripts/fetch_postgrad.py). Fees: Pump.fun 1.25%/side, PumpSwap 0.3%, tip + network fee per side.
Walk-forward: parameters picked on the first half of the period, reported on the second.

Usage: python3 scripts/momentum_bt.py [LATENCY_SLOTS] [skip|zero]
  skip|zero: graduated tokens without post-graduation data are skipped, or counted as worthless
"""
import glob, itertools, json, sys
from collections import Counter, deque

ROOT = "/home/rootkit/pump/clockit/data/backtest"
FEE = 0.0125
POOL_FEE = 0.003
SIDE_COST = 0.000125 + 0.000025  # tip (~1% of 0.01 SOL) + network fee, SOL
SIZE = 0.01
LAT = int(sys.argv[1]) if len(sys.argv) > 1 else 2
UNKNOWN_GRAD = sys.argv[2] if len(sys.argv) > 2 else "skip"

ENTRY_CACHE = {}
POSTGRAD = {}
GRAD_STATS = Counter()


def load(root):
    tapes = []
    for f in glob.glob(f"{root}/*.json"):
        mint = f.rsplit("/", 1)[-1][:-5]
        h = json.load(open(f))
        txs = h.get("txs", {})
        seen, seq = set(), []
        for s in h.get("order") or []:
            if s in txs and s not in seen:
                seen.add(s)
                seq.extend(txs[s])
        rest = [t for s, v in txs.items() if s not in seen for t in v]
        # A transaction can trade several tokens; keep this token's trades only
        trades = [t for t in seq + rest if t["mint"] == mint]
        trades.sort(key=lambda t: t["slot"])  # stable: keeps on-chain order within a slot
        if len(trades) >= 20:
            tapes.append(trades)
    return tapes


def load_postgrad(root):
    for f in glob.glob(f"{root}/*.json"):
        POSTGRAD[f.rsplit("/", 1)[-1][:-5]] = json.load(open(f))


def buy_tokens(t, sol):
    k = t["vsol"] * t["vtok"]
    return t["vtok"] - k / (t["vsol"] + sol * 1e9)


def sell_sol(t, tokens):
    k = t["vsol"] * t["vtok"]
    return (t["vsol"] - k / (t["vtok"] + tokens)) / 1e9


def price(t):
    return t["vsol"] / t["vtok"]


def find_entry(ti, tape, WIN, INFLOW, BUYERS, RMIN, RMAX):
    """Index of the first trade we can fill before (LAT slots after the signal), or None"""
    key = (ti, WIN, INFLOW, BUYERS, RMIN, RMAX)
    if key in ENTRY_CACHE:
        return ENTRY_CACHE[key]
    win, inflow, buyers = deque(), 0, Counter()
    n, out = len(tape), None
    for i, t in enumerate(tape):
        win.append(t)
        inflow += t["sol"] if t["is_buy"] else -t["sol"]
        if t["is_buy"]:
            buyers[t["user"]] += 1
        while t["ts"] - win[0]["ts"] > WIN:
            o = win.popleft()
            inflow -= o["sol"] if o["is_buy"] else -o["sol"]
            if o["is_buy"]:
                buyers[o["user"]] -= 1
                if buyers[o["user"]] == 0:
                    del buyers[o["user"]]
        if RMIN <= t["rsol"] / 1e9 <= RMAX and t["rtok"] > 0 and inflow / 1e9 >= INFLOW and len(buyers) >= BUYERS:
            j = i + 1
            while j < n and tape[j]["slot"] < t["slot"] + LAT:
                j += 1
            out = j if j < n else None
            break
    ENTRY_CACHE[key] = out
    return out


def pool_exit(mint, grad_slot, entry, hi, entry_ts, tokens, TP, HS, TR, HOLD):
    """SOL from selling on PumpSwap after graduation with the same exit rules, or None without data"""
    pts = [p for p in (POSTGRAD.get(mint) or {}).get("points") or [] if p[0] >= grad_slot + LAT]
    if not pts:
        return None
    for i, (slot, ts, q, b) in enumerate(pts):
        px = q / b
        hi = max(hi, px)
        if px >= entry * TP or px <= entry * (1 - HS) or (hi > entry and px <= hi * (1 - TR)) \
                or ts - entry_ts >= HOLD or i == len(pts) - 1:
            j = i
            while j + 1 < len(pts) and pts[j + 1][0] < slot + LAT:
                j += 1
            q2, b2 = pts[j][2], pts[j][3]
            return (q2 - q2 * b2 / (b2 + tokens)) / 1e9 * (1 - POOL_FEE)
    return None


def run(ti, tape, p):
    WIN, INFLOW, BUYERS, RMIN, RMAX, TP, HS, TR, HOLD = p
    j = find_entry(ti, tape, WIN, INFLOW, BUYERS, RMIN, RMAX)
    if j is None:
        return None
    n = len(tape)
    creator = tape[0]["creator"]
    state = tape[j - 1]
    cost = SIZE * (1 + FEE)
    tokens = buy_tokens(state, SIZE)
    entry = SIZE * 1e9 / tokens
    hi = entry
    entry_ts = state["ts"]
    exit_at = None
    for k in range(j, n):
        u = tape[k]
        px = price(u)
        hi = max(hi, px)
        if u["rtok"] == 0:
            got = pool_exit(u["mint"], u["slot"], entry, hi, entry_ts, tokens, TP, HS, TR, HOLD)
            if got is None:
                GRAD_STATS["unknown"] += 1
                if UNKNOWN_GRAD == "skip":
                    return None
                got = 0.0
            else:
                GRAD_STATS["pool exits"] += 1
            return (entry_ts, got - cost - 2 * SIDE_COST, "graduated")
        if (px >= entry * TP or px <= entry * (1 - HS) or (hi > entry and px <= hi * (1 - TR))
                or u["ts"] - entry_ts >= HOLD or (not u["is_buy"] and u["user"] == creator)):
            exit_at = k
            break
    if exit_at is None:
        exit_at = n - 1  # tape end: mark to market
    m = exit_at
    while m + 1 < n and tape[m + 1]["slot"] < tape[exit_at]["slot"] + LAT:
        m += 1
    got = sell_sol(tape[m], tokens) * (1 - FEE)
    return (entry_ts, got - cost - 2 * SIDE_COST, "curve")


def evaluate(tapes, p):
    return [r for r in (run(ti, t, p) for ti, t in tapes) if r]


def stats(res):
    if not res:
        return (0, 0.0, 0.0, 0.0)
    pnl = [r[1] for r in res]
    wins = sum(1 for x in pnl if x > 0)
    gain = sum(x for x in pnl if x > 0)
    loss = -sum(x for x in pnl if x < 0)
    return (len(pnl), wins / len(pnl), sum(pnl), gain / loss if loss else 99)


def split(tapes):
    starts = sorted(t[0]["ts"] for t in tapes)
    mid = starts[len(starts) // 2]
    idx = list(enumerate(tapes))
    return idx, [x for x in idx if x[1][0]["ts"] < mid], [x for x in idx if x[1][0]["ts"] >= mid]


GRID = list(itertools.product(
    [10, 30],                       # WIN s
    [1.0, 3.0, 6.0],                # INFLOW SOL
    [5, 10, 20],                    # BUYERS
    [(0, 15), (15, 40), (40, 75)],  # real SOL band
    [1.5, 2.5],                     # TP x
    [0.15, 0.25],                   # HS
    [0.15, 0.30],                   # TR
    [120, 600],                     # HOLD s
))

if __name__ == "__main__":
    tapes = load(f"{ROOT}/mints")
    load_postgrad(f"{ROOT}/postgrad")
    idx, first, second = split(tapes)
    print(f"{len(tapes)} tapes, {len(POSTGRAD)} with post-graduation data, latency {LAT} slots, "
          f"graduated without data: {UNKNOWN_GRAD}; halves {len(first)} / {len(second)}")
    rows = []
    for g in GRID:
        p = (g[0], g[1], g[2], g[3][0], g[3][1], g[4], g[5], g[6], g[7])
        a = stats(evaluate(first, p))
        if a[0] >= 30:
            rows.append((a, p))
    rows.sort(key=lambda r: -r[0][2])
    print(f"{len(rows)} settings with >= 30 trades in the first half; top 10 there, then the same on the second half:")
    print("   WIN INFLOW BUY  RSOL    TP   HS   TR HOLD |  1st: n   win    P&L    PF |  2nd: n   win    P&L    PF")
    for a, p in rows[:10]:
        b = stats(evaluate(second, p))
        print(f"   {p[0]:>3} {p[1]:>5} {p[2]:>4} {p[3]:>2}-{p[4]:<3} {p[5]:>4} {p[6]:>4} {p[7]:>4} {p[8]:>4} |"
              f" {a[0]:>5} {a[1]*100:4.0f}% {a[2]:+.3f} {a[3]:5.2f} | {b[0]:>5} {b[1]*100:4.0f}% {b[2]:+.3f} {b[3]:5.2f}")
    both = sum(1 for a, p in rows if a[2] > 0 and stats(evaluate(second, p))[2] > 0)
    print(f"settings profitable in BOTH halves: {both} of {len(rows)}")
