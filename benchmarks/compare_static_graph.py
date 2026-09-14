#!/usr/bin/env python3
"""Compare a compiled static sparse layer with the equivalent dense list layer.

Both programs use the same fixed 64x64 weight matrix. The sparse graph keeps
8 nonzero edges per output; the dense version stores and traverses all 4,096
weights. They are generated here so the benchmark stays readable without
checking in a several-thousand-token Scheme literal.
"""

from pathlib import Path
import re
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
N = 64
DEGREE = 8
REPEATS = 256


def nested_cons(values):
    values = list(values)
    result = "()"
    for value in reversed(values):
        result = f"(cons {value} {result})"
    return result


def matrix():
    rows = []
    for dst in range(N):
        row = [0] * N
        for k in range(DEGREE):
            src = (dst * 17 + k * 11) % N
            weight = ((dst * 7 + k * 5) % 7) - 3
            row[src] = weight or 2
        rows.append(row)
    return rows


def static_source(rows):
    inputs = " ".join(str(i) for i in range(N))
    outputs = " ".join(str(N + i) for i in range(N))
    edges = "\n".join(
        f"    ({src} {N + dst} {weight})"
        for dst, row in enumerate(rows)
        for src, weight in enumerate(row)
        if weight
    )
    values = " ".join(str(i + 1) for i in range(N))
    return f"""\
(define-graph layer
  (nodes {2 * N})
  (inputs ({inputs}))
  (outputs ({outputs}))
  (edges
{edges}))

(define (sum xs acc)
  (if (null? xs)
      acc
      (sum (cdr xs) (+ acc (car xs)))))

(define (run n acc)
  (if (= n 0)
      acc
      (run (- n 1) (+ acc (sum (layer {values}) 0)))))

(run {REPEATS} 0)
"""


def dense_source(rows):
    row_definitions = "\n\n".join(
        f"(define row{index} {nested_cons(row)})"
        for index, row in enumerate(rows)
    )
    matrix_expression = nested_cons(f"row{index}" for index in range(N))
    return f"""\
(define inputs {nested_cons(range(1, N + 1))})

{row_definitions}

(define rows {matrix_expression})

(define (dot xs ws acc)
  (if (null? xs)
      acc
      (dot (cdr xs) (cdr ws) (+ acc (* (car xs) (car ws))))))

(define (apply-rows xs rows)
  (if (null? rows)
      ()
      (cons (dot xs (car rows) 0)
            (apply-rows xs (cdr rows)))))

(define (sum xs acc)
  (if (null? xs)
      acc
      (sum (cdr xs) (+ acc (car xs)))))

(define (run n acc)
  (if (= n 0)
      acc
      (run (- n 1) (+ acc (sum (apply-rows inputs rows) 0)))))

(run {REPEATS} 0)
"""


def run_case(name, source, work):
    scm = work / f"{name}.scm"
    bend = work / f"{name}.bend"
    c_file = work / f"{name}.c"
    binary = work / f"{name}-bin"
    scm.write_text(source)
    with bend.open("w") as output:
        subprocess.run(
            ["cargo", "run", "--quiet", "--manifest-path", str(ROOT / "Cargo.toml"), str(scm)],
            cwd=ROOT,
            stdout=output,
            check=True,
        )
    with c_file.open("w") as output:
        subprocess.run(["bend", "gen-c", str(bend)], stdout=output, check=True)
    subprocess.run(["clang", "-O3", "-pthread", str(c_file), "-o", str(binary)], check=True)
    result = subprocess.run([str(binary)], capture_output=True, text=True, check=True).stdout
    answer = re.search(r"^Result: (.+)$", result, re.MULTILINE).group(1)
    itrs = re.search(r"^- ITRS: ([0-9]+)$", result, re.MULTILINE).group(1)
    seconds = re.search(r"^- TIME: ([0-9.]+)s$", result, re.MULTILINE).group(1)
    return {
        "answer": answer,
        "itrs": int(itrs),
        "seconds": float(seconds),
        "source": scm.stat().st_size,
        "bend": bend.stat().st_size,
        "c": c_file.stat().st_size,
    }


def main():
    rows = matrix()
    with tempfile.TemporaryDirectory(prefix="scheme-bend-graph-") as directory:
        work = Path(directory)
        sparse = run_case("static-graph", static_source(rows), work)
        dense = run_case("dense-list", dense_source(rows), work)
    if sparse["answer"] != dense["answer"]:
        raise SystemExit(
            f"result mismatch: static={sparse['answer']} dense={dense['answer']}"
        )
    print("case         result       ITRS          time    source   Bend      C")
    for name, case in (("static-graph", sparse), ("dense-list", dense)):
        print(
            f"{name:<12} {case['answer']:>12} {case['itrs']:>12} "
            f"{case['seconds']:>8.2f}s {case['source']:>8} "
            f"{case['bend']:>8} {case['c']:>8}"
        )
    print(f"ITRS speedup: {dense['itrs'] / sparse['itrs']:.2f}x")
    print(f"runtime speedup: {dense['seconds'] / sparse['seconds']:.2f}x")
    print(f"Bend size ratio: {sparse['bend'] / dense['bend']:.2f}x")
    print(f"C size ratio: {sparse['c'] / dense['c']:.2f}x")


if __name__ == "__main__":
    main()
