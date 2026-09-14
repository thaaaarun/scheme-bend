#!/usr/bin/env python3
"""Compare the old sparse-list kernel, the parallel Map-chunk kernel, and SBCL.

The two Scheme files remain the benchmark source of truth. The SBCL row is
generated from the original sparse-list source, so it is the requested native
reference rather than a hand-maintained twin.
"""

from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
OLD = ROOT / "benchmarks/matmul-sparse.scm"
NEW = ROOT / "benchmarks/matmul-frontier.scm"
GEN = ROOT / "benchmarks/gen_baseline.py"


def run(command, *, stdout=None):
    return subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        stdout=stdout,
        stderr=subprocess.PIPE,
        text=True,
    )


def bend_benchmark(source, work):
    bend = work / f"{source.stem}.bend"
    c_file = work / f"{source.stem}.c"
    binary = work / source.stem
    with bend.open("w") as output:
        run(["cargo", "run", "--quiet", "--", str(source)], stdout=output)
    run(["bend", "check", str(bend)])
    with c_file.open("w") as output:
        run(["bend", "gen-c", str(bend)], stdout=output)
    run(["clang", "-O3", "-pthread", str(c_file), "-o", str(binary)])
    started = time.perf_counter()
    result = run([str(binary)], stdout=subprocess.PIPE)
    wall = time.perf_counter() - started
    text = result.stdout
    match = re.search(r"Result:\s*([^\s]+)", text)
    itrs = re.search(r"ITRS:\s*([0-9]+)", text)
    reported = re.search(r"TIME:\s*([0-9.]+)s", text)
    if not (match and itrs and reported):
        raise RuntimeError(f"could not parse Bend output for {source}:\n{text}")
    return {
        "result": f"{int(match.group(1)):+d}",
        "itrs": int(itrs.group(1)),
        "reported": float(reported.group(1)),
        "wall": wall,
    }


def sbcl_benchmark(source, work):
    lisp = work / f"{source.stem}.lisp"
    with lisp.open("w") as output:
        run([sys.executable, str(GEN), str(source)], stdout=output)
    started = time.perf_counter()
    result = run(
        [
            "sbcl",
            "--noinform",
            "--disable-debugger",
            "--load",
            str(lisp),
            "--eval",
            '(format t "Result: ~A~%" (benchmark-result))',
            "--eval",
            "(quit)",
        ],
        stdout=subprocess.PIPE,
    )
    wall = time.perf_counter() - started
    match = re.search(r"Result:\s*([^\s]+)", result.stdout)
    if not match:
        raise RuntimeError(f"could not parse SBCL output for {source}:\n{result.stdout}")
    return {"result": f"{int(match.group(1)):+d}", "wall": wall}


def main():
    with tempfile.TemporaryDirectory(prefix="scheme-bend-matmul-") as directory:
        work = Path(directory)
        old = bend_benchmark(OLD, work)
        new = bend_benchmark(NEW, work)
        sbcl = sbcl_benchmark(OLD, work)

    if not (old["result"] == new["result"] == sbcl["result"]):
        raise RuntimeError(f"result mismatch: old={old} new={new} sbcl={sbcl}")

    print("case                 result     ITRS          Bend time   process wall")
    print(
        f"old scheme→Bend      {old['result']:>7} {old['itrs']:>12}"
        f"   {old['reported']:>8.2f}s    {old['wall']:>8.2f}s"
    )
    print(
        f"new parallel chunks→Bend {new['result']:>7} {new['itrs']:>12}"
        f"   {new['reported']:>8.2f}s    {new['wall']:>8.2f}s"
    )
    print(
        f"original SBCL        {sbcl['result']:>7} {'-':>12}"
        f"   {'-':>8}    {sbcl['wall']:>8.2f}s"
    )
    print(f"ITRS reduction: {100 * (1 - new['itrs'] / old['itrs']):.1f}%")
    print(f"Bend reported speedup: {old['reported'] / new['reported']:.2f}x")
    print(f"process-wall speedup: {old['wall'] / new['wall']:.2f}x")


if __name__ == "__main__":
    main()
