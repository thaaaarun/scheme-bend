#!/usr/bin/env python3
"""Search for the fastest *shape* of each program, automatically.

Each program is one source file, benchmarks/shapes/<program>/<program>.scm,
written in the as-written (chain) form. The compiler's `--shape` pass generates
the alternative shapes; nothing here is hand-written per variant.

Every shape must be a semantics-preserving rewrite of the same computation, so
the harness refuses to rank a program whose shapes disagree -- a search can
never "win" with a wrong answer.

The objective is median wall clock at the target thread count. ITRS is recorded
and deliberately NOT used to rank: it is a serial work count, and it mis-ranks
re-associated programs (a balanced tree can do 2x the interactions of a chain
and still be 3.5x faster on 8 threads).

Thread count is a compile-time knob in HVM's generated C (`TPC_L2`), not an
environment variable, so every (shape, threads) pair is compiled separately.
"""
import os, re, shutil, statistics, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BEND = os.path.expanduser("~/.cargo/bin/bend")
THREADS = [int(t) for t in os.environ.get("THREADS", "1,2,4,8").split(",")]
SHAPES = os.environ.get("SHAPES", "asis,balanced,chunked:2,chunked:4,chunked:8").split(",")
ROUNDS = int(os.environ.get("ROUNDS", "5"))
# Either `benchmarks/shapes/<program>/<program>.scm` or a flat directory of
# `<program>.scm`, so the same harness can sweep the benchmark suite.
PROGRAMS = os.environ.get("PROGRAMS", os.path.join("benchmarks", "shapes"))


def sh(cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True)


def build(scm, outdir, tag, spec, threads):
    """Scheme --shape=spec -> Bend -> C -> binary pinned to `threads`."""
    bend = sh([f"{ROOT}/target/release/scheme-bend", f"--shape={spec}", scm]).stdout
    bp = os.path.join(outdir, f"{tag}.bend")
    with open(bp, "w") as f:
        f.write(bend)
    c = os.path.join(outdir, f"{tag}_{threads}.c")
    with open(c, "w") as f:
        f.write(sh([BEND, "gen-c", bp]).stdout)
    exe = os.path.join(outdir, f"{tag}_{threads}")
    sh(["clang", "-O3", "-pthread", f"-DTPC_L2={threads.bit_length()-1}", c, "-o", exe])
    return exe


def run(exe):
    t0 = time.perf_counter()
    out = sh([exe]).stdout
    dt = time.perf_counter() - t0
    res = re.search(r"^Result: (\S+)", out, re.M)
    itrs = re.search(r"^- ITRS: (\d+)", out, re.M)
    return (res.group(1) if res else None,
            int(itrs.group(1)) if itrs else None, dt)


def main():
    shapes_root = os.path.join(ROOT, PROGRAMS)
    nested = {
        d: os.path.join(shapes_root, d, f"{d}.scm")
        for d in sorted(os.listdir(shapes_root))
        if os.path.isdir(os.path.join(shapes_root, d))
    }
    nested = {d: p for d, p in nested.items() if os.path.exists(p)}
    flat = {
        f[:-4]: os.path.join(shapes_root, f)
        for f in sorted(os.listdir(shapes_root))
        if f.endswith(".scm")
    }
    sources = nested or flat
    if not sources:
        sys.exit(f"no programs under {PROGRAMS}/")

    tmp = tempfile.mkdtemp()
    try:
        for prog, scm in sources.items():

            # --- correctness gate + compile + warm up ---
            exes, seen, itrs = {}, {}, {}
            for spec in SHAPES:
                for t in THREADS:
                    tag = f"{prog}_{spec.replace(':', '_')}"
                    exe = build(scm, tmp, tag, spec, t)
                    exes[(spec, t)] = exe
                    r, i, _ = run(exe)
                    seen.setdefault(spec, set()).add(r)
                    itrs[(spec, t)] = i
            if len({tuple(sorted(seen[s])) for s in SHAPES}) != 1:
                print(f"{prog}: SHAPES DISAGREE -- refusing to rank")
                for s in SHAPES:
                    print(f"    {s}: {sorted(seen[s])}")
                continue

            print(f"\n{prog}  ({len(SHAPES)} shapes agree: {sorted(seen[SHAPES[0]])[0]})"
                  f"  [{ROUNDS} rounds, median]")
            print(f"  {'shape':<12} {'ITRS':>12} " +
                  " ".join(f"{str(t)+'thr':>9}" for t in THREADS))
            times = {}
            for spec in SHAPES:
                for t in THREADS:
                    # Interleaved rounds: every candidate is timed once per
                    # round, so drift hits all of them equally.
                    times[(spec, t)] = statistics.median(
                        [run(exes[(spec, t)])[2] for _ in range(ROUNDS)])
            for spec in SHAPES:
                row = f"  {spec:<12} {itrs[(spec, THREADS[-1])]:>12} "
                row += " ".join(f"{times[(spec, t)]:>8.3f}s" for t in THREADS)
                print(row)

            best = {t: min(SHAPES, key=lambda s: times[(s, t)]) for t in THREADS}
            print("  winner:      " + " ".join(f"{best[t]:>9}" for t in THREADS))
            if len(set(best.values())) > 1:
                print("  -> no single best shape: the winner depends on how many"
                      " threads are available")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
