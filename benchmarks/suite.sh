#!/bin/sh
# Compare scheme-bend against SBCL across every benchmarks/*.scm.
#
# There is one source file per benchmark. The SBCL baseline is generated from it
# by gen_baseline.py, so the two can never drift apart.
#
# Results are compared modulo 2^24. Our target's integers are signed 24-bit and
# wrap; SBCL's fixnums do not. For a benchmark whose wrapped values only reach
# the final answer (e.g. parallel-sum) that is a target limitation, not a
# compiler difference, so the check normalises both sides and the report marks
# which benchmarks actually wrap. Any benchmark where a wrapped value feeds a
# branch would not be comparable this way and must be resized instead.
set -eu

runs=${RUNS:-5}
# Which code shape to compile with; see `scheme-bend --shape` and
# docs in src/shape.rs. Defaults to as-written.
shape=${SHAPE:-asis}
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
bend_bin=${BEND:-"$HOME/.cargo/bin/bend"}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

median() { sort -n | awk '{v[NR]=$1} END {print v[int((NR+1)/2)]}'; }

timeit() {
  i=1
  while [ "$i" -le "$runs" ]; do
    /usr/bin/time -p "$@" > /dev/null 2> "$tmp/t"
    awk '/^real/{print $2}' "$tmp/t"
    i=$((i + 1))
  done | median
}

# Floor: process startup dominates anything near it, so mark those rows.
floor=$(timeit sbcl --noinform --non-interactive --eval '(quit)')

echo "# shape: $shape   runs: $runs"
printf '%-16s %-12s %-8s %-9s %-9s %-6s %s\n' benchmark result match itrs bend_s sbcl_s note
printf '%-16s %-12s %-8s %-9s %-9s %-6s %s\n' ---------------- ------------ -------- --------- --------- ------ ----

for scm in "$root"/benchmarks/*.scm; do
  name=$(basename "$scm" .scm)
  python3 "$root/benchmarks/gen_baseline.py" "$scm" > "$tmp/$name.lisp"

  cargo run --quiet --manifest-path "$root/Cargo.toml" -- --shape="$shape" "$scm" > "$tmp/$name.bend"
  "$bend_bin" gen-c "$tmp/$name.bend" > "$tmp/$name.c" 2>/dev/null
  clang -O3 -pthread "$tmp/$name.c" -o "$tmp/$name-bin"

  bout=$("$tmp/$name-bin")
  braw=$(printf '%s\n' "$bout" | sed -n 's/^Result: //p')
  itrs=$(printf '%s\n' "$bout" | sed -n 's/^- ITRS: //p')
  sraw=$(sbcl --noinform --non-interactive --load "$tmp/$name.lisp" \
           --eval '(format t "~a" (benchmark-result))' 2>/dev/null | tail -1)

  # normalise both to unsigned 24-bit
  bmod=$(python3 -c "import sys;print(int(sys.argv[1],0)%(1<<24))" "$braw")
  smod=$(python3 -c "import sys;print(int(sys.argv[1],0)%(1<<24))" "$sraw")
  # A benchmark "wraps" only when the two disagree numerically while still
  # agreeing modulo 2^24 -- i.e. the true value left the 24-bit domain.
  same=$(python3 -c "import sys;print(1 if int(sys.argv[1],0)==int(sys.argv[2],0) else 0)" "$braw" "$sraw")
  if [ "$same" = "1" ]; then wraps=""; else wraps="yes"; fi

  if [ "$bmod" = "$smod" ]; then match=ok; else match="MISMATCH"; fi

  btime=$(timeit "$tmp/$name-bin")
  stime=$(timeit sbcl --noinform --non-interactive --load "$tmp/$name.lisp" \
            --eval '(benchmark-result)')
  note=""
  awk -v s="$stime" -v f="$floor" 'BEGIN{exit !(s < 3*f)}' && note="too small to time"
  printf '%-16s %-12s %-8s %-9s %-9s %-6s %s\n' \
    "$name" "$braw" "$match" "$itrs" "$btime" "$stime" "$note"
done
