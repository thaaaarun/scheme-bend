use scheme_bend::compile;
use std::{fs, path::Path};

#[test]
fn compiles_the_correctness_corpus() {
    for name in [
        "arithmetic",
        "factorial",
        "fib",
        "lists",
        "map",
        "mergesort",
        "sparse-frontier",
        "static-graph",
        "tree-sum",
    ] {
        let source = fs::read_to_string(Path::new("examples").join(format!("{name}.scm"))).unwrap();
        let output = compile(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(output.contains("def main():"), "{name}");
        assert!(!output.contains("statement-only expression"), "{name}");
    }
}

#[test]
fn emits_only_live_static_graph_nodes_in_topological_order() {
    let source = fs::read_to_string("examples/static-graph.scm").unwrap();
    let output = compile(&source).expect("static graph should compile");
    let graph = output.split("def s_layer").nth(1).unwrap();
    assert!(graph.contains("s_layerN3 ="));
    assert!(graph.contains("s_layerN4 ="));
    assert!(graph.contains("s_layerN5 ="));
    assert!(graph.contains("s_layerN6 ="));
    assert!(graph.contains("s_layerN7 ="));
    assert!(!graph.contains("N8"));
    assert!(!graph.contains("N9"));
    assert!(graph.find("N3").unwrap() < graph.find("N5").unwrap());
    assert!(graph.find("N5").unwrap() < graph.find("N6").unwrap());
    assert!(output.contains("return s_sum(s_layer(+2, +3, +5), +0)"));
}

#[test]
fn emits_the_dynamic_sparse_frontier_kernel() {
    let source = fs::read_to_string("examples/sparse-frontier.scm").unwrap();
    let output = compile(&source).expect("sparse frontier should compile");
    assert!(output.contains("Map/set("));
    assert!(output.contains("scheme_map_get0("));
    assert!(output.contains("scheme_map_accumulate("));
    assert!(output.contains("scheme_sparse_frontier("));
    assert!(output.contains("Map/get_check("));
}

#[test]
fn emits_the_specialized_sparse_scatter_kernel() {
    let source = fs::read_to_string("benchmarks/matmul-frontier.scm").unwrap();
    let output = compile(&source).expect("sparse scatter should compile");
    assert!(output.contains("scheme_sparse_scatter("));
    assert!(output.contains("scheme_sparse_scatter_edges("));
    assert!(output.contains("scheme_sparse_square_sum("));
    assert!(output.contains("scheme_sparse_chunk_tree("));
    assert!(output.contains("scheme_sparse_parallel_chunks("));
    assert!(output.contains("Map/get_check("));
}

#[test]
fn rejects_cycles_that_reach_a_static_graph_output() {
    let source = "(define-graph cyclic\n  (nodes 3)\n  (inputs (0))\n  (outputs (2))\n  (edges (0 1 1) (1 2 1) (2 1 1)))\n(cyclic 4)";
    let error = compile(source).unwrap_err().to_string();
    assert!(error.contains("cycle on a live path"), "{error}");
}

#[test]
fn compiles_the_sparse_matmul_kernel() {
    let source = fs::read_to_string("benchmarks/matmul-sparse.scm").unwrap();
    let output = compile(&source).expect("sparse matmul should compile");
    assert!(output.contains("def s_scatter("));
    assert!(output.contains("def s_rowsQ45_checksum("));
}

#[test]
fn compiles_the_mul0_pruning_workload() {
    let source = fs::read_to_string("benchmarks/mul0-prune.scm").unwrap();
    let output = compile(&source).expect("mul0 pruning workload should compile");
    let main = output.split("def main():").nth(1).unwrap();
    assert!(main.contains("if (scheme_tmp_"));
    assert_eq!(main.matches("s_work(+100000, +0)").count(), 1);
}

#[test]
fn desugars_immediate_lambda_and_emits_let() {
    // `rec` cannot be inlined, so its result stays opaque and the immediately
    // invoked lambda still desugars to a `let`. A literal argument would be
    // propagated and the body folded away, which is correct but would not
    // exercise the desugaring.
    let output = compile(
        "(define (rec n) (if (= n 0) 0 (+ 1 (rec (- n 1)))))\n\
         ((lambda (x) (+ x x)) (rec 3))",
    )
    .unwrap();
    assert!(
        output.contains("s_schemeLocal1Q95_x = s_rec(+3)"),
        "{output}"
    );
    assert!(
        output.contains("return (s_schemeLocal1Q95_x + s_schemeLocal1Q95_x)"),
        "{output}"
    );
}

#[test]
fn compiles_nested_control_flow_and_signed_i24_arithmetic() {
    // Again the argument is opaque, so the branch cannot fold at compile time.
    let output = compile(
        "(define (rec n) (if (= n 0) 0 (+ 1 (rec (- n 1)))))\n\
         (define (f n)\n\
           (let ((x (if (< n +0) -7 2)))\n\
             (if (< x +0) (/ x 2) 0)))\n\
         (f (rec 3))",
    )
    .unwrap();
    assert!(!output.contains("statement-only expression"));
    assert!(output.contains("-7"));
    assert!(output.contains("+0"));
    assert!(output.contains("if "));
}

#[test]
fn inlines_helpers_and_shares_repeated_numeric_work() {
    let output =
        compile("(define (square x) (* x x))\n(define (hot x) (+ (square x) (square x)))\n(hot 9)")
            .unwrap();
    assert_eq!(output.matches("s_square(").count(), 1);
    assert!(output.contains("s_schemeOpt"));
}

#[test]
fn emits_zero_annihilating_multiply_as_a_guarded_operation() {
    let output = compile(
        "(define (opaque n) (if (= n 0) 0 (opaque (- n 1))))\n\
         (mul0 (opaque 1) (opaque 2))",
    )
    .unwrap();
    assert!(output.contains("if (scheme_tmp_"), "{output}");
    assert!(output.contains("s_opaque(+1)"), "{output}");
    assert!(output.contains("s_opaque(+2)"), "{output}");
}

#[test]
fn unrolls_a_closed_scalar_tail_recurrence() {
    let output =
        compile("(define (sum n acc)\n  (if (= n 0) acc (sum (- n 1) (+ acc n))))\n(sum 4 0)")
            .unwrap();
    // One recursive call remains after four source iterations are fused into
    // the same ordinary Bend function body, plus the initial call from main.
    assert_eq!(output.matches("return s_sum(").count(), 2);
    // Each fused iteration keeps its own exit test.
    assert!(output.matches("if (").count() >= 4);
}

#[test]
fn never_emits_bend_switch_for_signed_values() {
    // Bend's `switch` is a U24 construct and its arms do not preserve I24
    // tags, so a value returned from one silently becomes unsigned and later
    // signed comparisons are wrong. Verified directly against bend-lang
    // 0.2.38: a hand-written `switch` returns 16777208 where the equivalent
    // `if` returns -8. The emitter must therefore stay on `if`.
    let output = compile("(define (count n) (if (= n 0) 0 (count (- n 1))))\n(count 3)").unwrap();
    assert!(!output.contains("switch "), "{output}");
    assert!(output.contains("if ("), "{output}");
}

#[test]
fn propagates_constants_and_folds_the_dead_branch() {
    // The inliner substitutes `mode`, leaving a literal `let`; propagation then
    // folds the `if`, so the branch and its `* 10` arm leave the emitted body
    // instead of surviving as a runtime test.
    let output = compile(
        "(define (pick x mode) (if mode (+ x 1) (* x 10)))\n\
         (define (f n) (pick n +1))\n\
         (f 5)",
    )
    .unwrap();
    let f = output.split("def s_f").nth(1).expect("s_f was not emitted");
    let f = f.split("def main").next().unwrap();
    assert!(!f.contains('*'), "dead arm survived: {f}");
    assert!(f.contains("+ +1"), "{f}");
}

#[test]
fn folds_a_closed_constant_program() {
    // With every input known, the cascade runs to a single literal.
    assert!(
        compile("((lambda (x) (* x x)) 21)")
            .unwrap()
            .contains("return +441")
    );
}

#[test]
fn binds_every_list_walked_in_lockstep() {
    // Three lists consumed together must each be bound once. Leaving any of
    // them as separate car/cdr calls duplicates them per element, and eager
    // HVM then copies the whole spine, which is quadratic.
    let output = compile(
        "(define (zip3 xs ys zs acc)\n\
           (if (null? xs)\n\
               acc\n\
               (zip3 (cdr xs) (cdr ys) (cdr zs) (+ acc (+ (car xs) (+ (car ys) (car zs)))))))\n\
         (zip3 (cons 1 ()) (cons 2 ()) (cons 3 ()) 0)",
    )
    .unwrap();
    // Scope the checks to the generated function: the prelude has its own
    // matches and accessor calls.
    let body = output.split("def s_zip3").nth(1).expect("s_zip3 emitted");
    // One match for the tested list plus one per additional list.
    assert_eq!(body.matches("match ").count(), 3, "{body}");
    // The hot path binds every list once. Accessor calls survive only in the
    // exhausted-list arms, which fire when the lists differ in length -- there
    // the sentinels scheme_car/scheme_cdr return are the faithful behaviour.
    let accessors = body.matches("scheme_car(").count() + body.matches("scheme_cdr(").count();
    assert!(accessors <= 2, "{accessors} accessor calls:\n{body}");
}

#[test]
fn rejects_numbers_outside_the_signed_i24_range() {
    assert!(
        compile("8388608")
            .unwrap_err()
            .to_string()
            .contains("signed 24-bit")
    );
}

#[test]
fn rejects_mutation_and_first_class_functions() {
    assert!(
        compile("(set! x 1)")
            .unwrap_err()
            .to_string()
            .contains("outside")
    );
    assert!(
        compile("((if #t f g) 1)")
            .unwrap_err()
            .to_string()
            .contains("first-class")
    );
}
