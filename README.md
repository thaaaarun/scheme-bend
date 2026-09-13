# scheme-bend

`scheme-bend` is a small Rust compiler for an immutable Scheme subset. It emits
human-readable Bend source, letting Bend/HVM own the low-level interaction-net
and CPU/CUDA work.

```
Scheme source -> S-expression parser -> Scheme AST/desugaring -> functional IR -> Bend source
```

## Build and run

```sh
cargo build --release
./target/release/scheme-bend examples/factorial.scm > factorial.bend
cargo test
```

The CLI writes Bend to standard output, so it is easy to pipe to a file or a
Bend toolchain. Example programs live in `examples/`.

The default compiler pipeline performs Bend-only source optimization before
rendering: it inlines small direct helpers, shares repeated total numeric
expressions, and unrolls eligible closed scalar tail recurrences eight times.
It also propagates literal `let` bindings into their uses so that conditions
over them fold; without this an inlined constant argument stays a variable read
and every branch survives into the emitted Bend.
Those are ordinary Bend constructs, not an HVM fork or an FFI escape hatch.

The emitter binds list variables once as well. Lowering
`(if (null? xs) base (f (cdr xs) (... (car xs) ...)))` to three separate
accessor calls uses `xs` three times, and a repeated use costs a DUP node.
HVM2 is eager, so duplicating a cons cell splits the node *and* its children
immediately and the copy cascades down the spine: one spine copy per element
makes traversal O(N^2). Emitting a single `match` that destructures once makes
it O(N). The same applies to a second list walked in lockstep, as in a dot
product or a merge, which is bound with a nested `match`.
Use `--no-opt` for direct lowering, `--no-cse` to evaluate recomputation rather
than sharing, `--no-const-prop` to disable literal propagation, or
`--tail-unroll 1` to keep inlining/CSE while disabling recurrence
specialization.

Deliberately *not* used: Bend's `switch`. It is a U24 construct and its arms do
not preserve I24 tags, so a value returned from one silently becomes unsigned
and later signed comparisons are wrong. Measured directly against bend-lang
0.2.38, a hand-written `switch` returns `16777208` where the equivalent `if`
returns `-8`. The emitter therefore stays on `if`, and a regression test pins
this.

## Shape selection

HVM finds parallelism in the interaction net rather than in annotations, so the
clearest lever a compiler has over parallelism is the *shape* of the code it
emits. The default emitter preserves the shape you wrote; `--shape` re-associates
a recognized reduction into a different one.

```sh
scheme-bend --shape asis       prog.scm   # as written (default)
scheme-bend --shape balanced   prog.scm   # re-associate the whole range
scheme-bend --shape chunked:8  prog.scm   # 8 chain segments, balanced combine
```

The recognized form is an ascending counted reduction whose combiner is `+` or `*`:

```scheme
(define (f i n acc)
  (if (> i n) acc (f (+ i 1) n (OP acc G))))
```

Re-association is legal because the target's arithmetic wraps modulo 2^24, which
is a ring: `+` and `*` are associative and commutative there, while `-` and
truncating `/` are not and are never rewritten. The pass fires only when the
top-level call has literal, non-negative bounds, so every synthesized midpoint
stays in range and the generated recursion is well founded.

`benchmarks/shape_search.py` sweeps the shapes, refuses to rank a program whose
shapes disagree on the answer, and reports the winner per thread count. For the
sum of 1..2,000,000 -- every shape returns `+5857856` -- median of 3:

| shape | ITRS | 1 thread | 2 threads | 4 threads | 8 threads |
|---|---:|---:|---:|---:|---:|
| asis | 40,000,019 | 0.399s | 0.564s | 0.517s | 0.491s |
| balanced | 167,999,952 | 1.344s | 1.376s | 1.361s | 0.288s |
| chunked:2 | 40,000,042 | 0.385s | 0.203s | 0.266s | 0.260s |
| chunked:4 | 40,000,084 | 0.391s | 0.201s | 0.197s | 0.218s |
| chunked:8 | 40,000,168 | 0.393s | 0.204s | 0.163s | 0.199s |

Three things this shows. `chunked` holds the interaction count flat while buying
2-2.4x at two or more threads, so it is close to free. `balanced` pays 4.2x the
interactions and is *worse* than asis until the last thread, because its extra
serial work is only repaid once there is real slack to fill. And the winner
depends on how many threads are available, so there is no single best shape.

Note also that ITRS cannot rank these: `chunked:8` reports roughly the same
count as `asis` and runs 2.5x faster, and `asis` gets *slower* when given a
second thread (0.399s to 0.564s) with its interaction count unchanged. Use ITRS
to compare compiler transformations on one thread and to catch regressions; rank
parallel shapes with wall clock.

## Supported subset

Signed 24-bit integers (`-8,388,608..=8,388,607`), `#t`/`#f` (lowered to Bend's `1`/`0` conditions), top-level `define` (including shorthand
function definitions), `lambda`, `if`, parallel `let`, named function calls,
top-level recursion, binary `+ - * / = < > <= >=`, and immutable pairs/lists
via `cons`, `car`, `cdr`, `null?`, and `()` are supported. A final top-level
expression becomes Bend `main`.

Anonymous lambdas can be invoked immediately, which desugars to a `let`.
Top-level functions can be recursive. First-class closures, passing functions
as values, and indirect calls are intentionally outside v0; specialize
higher-order helpers such as `map` to a named mapper instead.

## Intentional limitations

Mutation, I/O, macros, exceptions, `eq?`, `call/cc`, quoting/reader syntax,
multiple body expressions, the full Scheme numeric tower (including bignums,
rationals, floats, and complex numbers), proper runtime type
errors, and first-class closures are out of scope. `car`/`cdr` on `()` has the
usual undefined/error behavior and is not statically diagnosed.

The emitted prelude defines an immutable `SchemeList` and helpers. Generated
`car`/`cdr` use `0`/`()` as a total target-level sentinel for `()`; valid
programs must not rely on it. The current emitter targets Bend 0.2-style
`Type/Constructor` syntax and was validated with `bend-lang 0.2.38` against
every bundled example.

## Benchmark

With Bend, HVM, and SBCL installed, run `sh benchmarks/run.sh`. It compares
naive `fib(34)`, an inherently parallel balanced range sum, and a fixed-point
256x256 Mandelbrot checksum through Bend's C runtime, pre-lowered HVM's C
runtime, Bend-generated standalone C, and SBCL's compiled native function.
Treat it as a backend-shape comparison, not a GPU benchmark: this macOS host
has no supported CUDA path.

The Mandelbrot program uses signed I24 arithmetic throughout and has matching
Scheme/Bend and SBCL output: checksum `3808042`.

For the same signed-I24 Mandelbrot program, the compiler modes produce these
deterministic HVM interaction counts:

| compiler mode | HVM interactions |
|---|---:|
| direct lowering (`--no-opt`) | 376,421,184 |
| inline + CSE (`--tail-unroll 1`) | 350,229,884 |
| 8-way recurrence fusion, without CSE (`--no-cse`) | 249,304,617 |
| default, including 8-way recurrence fusion | 239,870,882 |

The recurrence pass is deliberately narrow: it only accepts a direct, closed,
scalar self-tail-call. It emits eight ordinary nested Bend iterations and one
remaining recursive call, preserving each iteration's exit test. It does not
attempt a general loop optimizer or change Bend's runtime.

Sharing is deliberately measurable: common-subexpression elimination can add
copies in an interaction net, but on this recurrence it saves 8,029,132 total
interactions by computing each repeated square once. `--no-cse` is provided for
checking other programs where recomputation might be the better trade.

Current five-run backend-shape measurements on this host (bend-lang 0.2.38,
SBCL 2.6.8) are:

| workload | Bend | SBCL | gap |
|---|---:|---:|---:|
| `fib(34)` (C runtime) | ~0.44 s | ~0.06 s | ~7x |
| balanced sum 1..1M (standalone C) | ~0.08 s | ~0.02 s | ~4x |
| Mandelbrot 256x256 (standalone C) | ~0.56 s | ~0.08 s | ~7x |

`benchmarks/suite.sh` covers sixteen programs and reports the deterministic
interaction count alongside wall clock. The SBCL baselines are *generated* from
the `.scm` sources by `benchmarks/gen_baseline.py`, so there is one source of
truth per benchmark and the two cannot drift apart; hand-maintained twins had
quietly diverged in five of fourteen cases. Several kernels are repeated inside
one process because a single run finishes below SBCL's ~10ms process startup.

Wall clock needs care in two ways. Benchmark shape decides how much of HVM's
parallelism is reachable: a divide-and-conquer kernel (matmul, parallel-sum)
keeps the redex frontier wide and gains ~5-7x from eight threads, while a tight
accumulator chain has little slack and gains none. And when threads run dry
HVM calls `sched_yield` on every idle tick and only re-checks for termination
once per 256 ticks, so a low-slack kernel can burn seconds of system time for
no gain -- powmod spends 8.5s of kernel time at eight threads versus 0.18s at
one, with no improvement in wall clock. For sequential kernels the
single-threaded number is the meaningful one.

`parallel-sum` agrees exactly with its SBCL baseline (checksum 5908768). The
Mandelbrot checksum above is the signed-I24 version, so it replaces the older
unsigned-workaround result.

Wall-clock timings on this shared, multicore host fluctuate substantially; use
the interaction-count table to compare compiler transformations and run the
script locally before drawing small timing conclusions.

Two effects dominate. First, Bend parallelizes well (about 6-7x on 8 threads),
but HVM's per-operation constant is large: every primitive is a tagged
interaction. (Function calls are *not* the cost: Bend inlines non-recursive
definitions, so a helper called from one or two sites contributes no call
rewrites at all. Measured directly: a loop calling a trivial helper and the
same loop with the body written out produce identical interaction counts.)
Second, HVM has no native scalar loop corresponding to SBCL's jump, and
control flow is the expensive primitive -- a comparison plus branch costs
roughly twice an arithmetic operation, while a bare loop skeleton costs about
13 interactions per iteration even with no arithmetic in it. Chunky workloads do not
automatically win, because the "chunk" is itself made of interactions. What
helps measurably is reducing the number of those interactions in emitted Bend:

- Inlining small direct helpers keeps hot arithmetic call-free while preserving
  readable Scheme source; `step-re` and `step-im` in
  `benchmarks/mandelbrot.scm` exercise this path.
- Specializing a closed scalar recurrence into eight ordinary Bend iterations
  removes seven of every eight recursive call expansions. The size guard that
  bounds this was previously too tight to admit a factor above four, so raising
  it is worth another 6% on Mandelbrot (`--tail-unroll 6/8` used to behave
  exactly like `1`).
- But more fusion is not better without limit. On Mandelbrot the interaction
  count keeps falling to 64-way fusion (254.9M at 4, 239.9M at 8, 226.0M at 64)
  while median wall clock is 0.50s at 4, 0.49s at 8 and 0.56s at 16. Past
  roughly eight the larger function body costs more than the saved call
  expansions return, so interaction count alone is not a safe proxy for speed
  and the default stops at 8.
- Binding list variables once instead of calling `car`/`cdr`/`null?`
  separately. Traversing a list was O(N^2) in interactions and is now O(N):
  a 2000-element walk fell from 56,176,046 to 92,030. A dot product over two
  2000-element lists fell from 28,216,050 to 198,050, and the 32x32 matmul
  benchmark fell from 26.4M to 2.7M interactions per matrix.
- Propagating literal `let` bindings lets a constant branch fold instead of
  being re-tested at runtime. On a loop that calls a small helper with a
  constant mode argument, interactions drop from 470,013 to 310,013 (about 34%)
  with an unchanged result, exactly matching a hand-specialized version.

These are local source-level fixes; ordinary Bend still evaluates each dynamic
arithmetic primitive as an interaction.
