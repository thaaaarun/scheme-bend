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
        "tree-sum",
    ] {
        let source = fs::read_to_string(Path::new("examples").join(format!("{name}.scm"))).unwrap();
        let output = compile(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(output.contains("def main():"), "{name}");
        assert!(!output.contains("statement-only expression"), "{name}");
    }
}

#[test]
fn desugars_immediate_lambda_and_emits_let() {
    let output = compile("((lambda (x) (+ x x)) 21)").unwrap();
    assert!(output.contains("s_schemeLocal0_95_x = +21"));
    assert!(output.contains("return (s_schemeLocal0_95_x + s_schemeLocal0_95_x)"));
}

#[test]
fn compiles_nested_control_flow_and_signed_i24_arithmetic() {
    let output = compile("(let ((x (if #t -7 2))) (if (< x +0) (/ x 2) 0))").unwrap();
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
fn unrolls_a_closed_scalar_tail_recurrence() {
    let output =
        compile("(define (sum n acc)\n  (if (= n 0) acc (sum (- n 1) (+ acc n))))\n(sum 4 0)")
            .unwrap();
    // One recursive call remains after four source iterations are fused into
    // the same ordinary Bend function body, plus the initial call from main.
    assert_eq!(output.matches("return s_sum(").count(), 2);
    assert!(output.matches("if (s_scheme").count() >= 4);
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
