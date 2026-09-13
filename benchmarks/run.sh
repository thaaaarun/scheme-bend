#!/bin/sh
# Run from the repository root. Reports wall-clock seconds (median of 5 runs).
set -eu

runs=5
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Baselines are generated from the Scheme sources so the two cannot drift.
for name in fib parallel-sum mandelbrot; do
  python3 "$root/benchmarks/gen_baseline.py" "$root/benchmarks/$name.scm" > "$tmp/$name.lisp"
done

cargo run --quiet --manifest-path "$root/Cargo.toml" -- "$root/benchmarks/fib.scm" > "$tmp/fib.bend"
"${HOME}/.cargo/bin/bend" gen-hvm "$tmp/fib.bend" > "$tmp/fib.hvm" 2>/dev/null
cargo run --quiet --manifest-path "$root/Cargo.toml" -- "$root/benchmarks/parallel-sum.scm" > "$tmp/parallel-sum.bend"
"${HOME}/.cargo/bin/bend" gen-hvm "$tmp/parallel-sum.bend" > "$tmp/parallel-sum.hvm" 2>/dev/null
"${HOME}/.cargo/bin/bend" gen-c "$tmp/parallel-sum.bend" > "$tmp/parallel-sum.c" 2>/dev/null
clang -O3 -pthread "$tmp/parallel-sum.c" -o "$tmp/parallel-sum-c"
cargo run --quiet --manifest-path "$root/Cargo.toml" -- "$root/benchmarks/mandelbrot.scm" > "$tmp/mandelbrot.bend"
"${HOME}/.cargo/bin/bend" gen-c "$tmp/mandelbrot.bend" > "$tmp/mandelbrot.c" 2>/dev/null
clang -O3 -pthread "$tmp/mandelbrot.c" -o "$tmp/mandelbrot-c"
"${HOME}/.cargo/bin/bend" gen-hvm "$tmp/mandelbrot.bend" > "$tmp/mandelbrot.hvm" 2>/dev/null

measure() {
  label=$1
  shift
  values=""
  i=1
  while [ "$i" -le "$runs" ]; do
    /usr/bin/time -p "$@" > /dev/null 2> "$tmp/time"
    value=$(awk '/^real / { print $2 }' "$tmp/time")
    values="$values $value"
    i=$((i + 1))
  done
  median=$(printf '%s\n' "$values" | tr ' ' '\n' | awk 'NF' | sort -n | awk 'NR == 3 { print }')
  printf '%-28s %s s median  [%s]\n' "$label" "$median" "$values"
}

echo "Machine: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m)"
echo "Bend: $("${HOME}/.cargo/bin/bend" --version); SBCL: $(sbcl --version)"
echo "Workload: naive fib(34), 5 independent process runs"
measure "Bend frontend + C runtime" "${HOME}/.cargo/bin/bend" run-c "$tmp/fib.bend"
measure "HVM C runtime only" "${HOME}/.cargo/bin/hvm" run-c "$tmp/fib.hvm"
measure "SBCL compiled native function" sbcl --noinform --non-interactive --load "$tmp/fib.lisp" --eval '(benchmark-result)'

echo
echo "Workload: balanced sum of 1..1,000,000 (independent halves), 5 process runs"
measure "Bend frontend + C runtime" "${HOME}/.cargo/bin/bend" run-c "$tmp/parallel-sum.bend"
measure "HVM C runtime only" "${HOME}/.cargo/bin/hvm" run-c "$tmp/parallel-sum.hvm"
measure "Bend generated standalone C" "$tmp/parallel-sum-c"
measure "SBCL compiled native function" sbcl --noinform --non-interactive --load "$tmp/parallel-sum.lisp" --eval '(benchmark-result)'

echo
echo "Workload: fixed-point Mandelbrot 256x256 escape-time checksum, 5 process runs"
measure "Bend generated standalone C" "$tmp/mandelbrot-c"
measure "Bend frontend + C runtime" "${HOME}/.cargo/bin/bend" run-c "$tmp/mandelbrot.bend"
measure "HVM C runtime only" "${HOME}/.cargo/bin/hvm" run-c "$tmp/mandelbrot.hvm"
measure "SBCL compiled native function" sbcl --noinform --non-interactive --load "$tmp/mandelbrot.lisp" --eval '(benchmark-result)'
