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
expressions, and unrolls eligible closed scalar tail recurrences four times.
It also propagates literal `let` bindings into their uses so that conditions
over them fold; without this an inlined constant argument stays a variable read
and every branch survives into the emitted Bend.
Those are ordinary Bend constructs, not an HVM fork or an FFI escape hatch.
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
| 4-way recurrence specialization, without CSE (`--no-cse`) | 262,976,288 |
| default, including 4-way recurrence specialization | 254,947,156 |

The recurrence pass is deliberately narrow: it only accepts a direct, closed,
scalar self-tail-call. It emits four ordinary nested Bend iterations and one
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
- Specializing a closed scalar recurrence into four ordinary Bend iterations
  removes three of every four recursive call expansions.
- Propagating literal `let` bindings lets a constant branch fold instead of
  being re-tested at runtime. On a loop that calls a small helper with a
  constant mode argument, interactions drop from 470,013 to 310,013 (about 34%)
  with an unchanged result, exactly matching a hand-specialized version.

These are local source-level fixes; ordinary Bend still evaluates each dynamic
arithmetic primitive as an interaction.
