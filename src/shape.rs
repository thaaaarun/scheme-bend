//! Shape selection for recognized associative reductions.
//!
//! HVM discovers parallelism from the *shape* of the interaction net rather
//! than from annotations, so the clearest lever a compiler has over parallelism
//! is the shape of the code it emits. Measurements back this: re-associating
//! one sum changed nothing but its shape, doubled its interaction count, and
//! still ran 3.5x faster on eight threads.
//!
//! This pass recognizes a narrow class of reductions and re-emits one in a
//! chosen shape:
//!
//! * `AsWritten` -- leave the chain alone.
//! * `Balanced` -- re-associate the range into a binary tree. Span becomes
//!   O(log N) and the redex frontier is wide, at the cost of more interactions.
//! * `Chunked(k)` -- split the range into `k` chain segments and combine those
//!   with a balanced tree. Keeps the chain's interaction count while exposing a
//!   bounded frontier, which is often the best of both.
//!
//! The recognized form is an ascending counted reduction with an associative
//! combiner:
//!
//! ```text
//! (define (f i n acc)
//!   (if (> i n)
//!       acc
//!       (f (+ i 1) n (OP acc G))))
//! ```
//!
//! where `OP` is `+` or `*` and `G` reads only `i`.
//!
//! Re-association is legal because the target's arithmetic wraps modulo 2^24,
//! which is a ring: `+` and `*` are associative and commutative there. `-` and
//! truncating `/` are not, so they are not in the whitelist. Each combiner has
//! an identity (`0` for `+`, `1` for `*`) which is what lets the range be split
//! anywhere, including into empty segments.
//!
//! The pass fires only when the top-level body calls a recognized reduction
//! with literal, non-negative bounds. That keeps every midpoint in range --
//! midpoints are computed as `lo + (hi - lo) / 2`, so they need `hi - lo` not
//! to wrap -- and guarantees the recursion terminates.

use crate::ir::{Definition, Expr, Primitive, Program};
use std::collections::BTreeSet;
use std::fmt;

/// Which shape to emit for a recognized reduction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Leave recognized reductions exactly as written.
    AsWritten,
    /// Re-associate the whole range into a balanced binary tree.
    Balanced,
    /// `k` chain segments combined by a balanced tree.
    Chunked(u32),
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Shape::AsWritten => f.write_str("asis"),
            Shape::Balanced => f.write_str("balanced"),
            Shape::Chunked(k) => write!(f, "chunked:{k}"),
        }
    }
}

impl Shape {
    pub fn parse(spec: &str) -> Option<Shape> {
        match spec {
            "asis" => Some(Shape::AsWritten),
            "balanced" => Some(Shape::Balanced),
            _ => spec
                .strip_prefix("chunked:")
                .and_then(|k| k.parse::<u32>().ok())
                .filter(|k| *k >= 1)
                .map(Shape::Chunked),
        }
    }
}

/// A recognized reduction, decomposed into the pieces a shape rewrite needs.
#[derive(Clone)]
struct Reduction {
    counter: String,
    /// The associative, commutative combiner. Only `+` and `*`.
    combiner: Primitive,
    /// The per-element expression, which may read only `counter`.
    element: Expr,
}

/// The identity element of the combiner, so an empty segment is neutral.
fn identity(combiner: Primitive) -> Expr {
    match combiner {
        Primitive::Mul => Expr::Number(1),
        _ => Expr::Number(0),
    }
}

fn combine(combiner: Primitive, left: Expr, right: Expr) -> Expr {
    Expr::Primitive {
        op: combiner,
        args: vec![left, right],
    }
}

fn as_var(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name) => Some(name),
        _ => None,
    }
}

fn as_int(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Number(n) => Some(*n),
        _ => None,
    }
}

fn is_add_one_of(expr: &Expr, counter: &str) -> bool {
    match expr {
        Expr::Primitive {
            op: Primitive::Add,
            args,
        } => matches!(&args[..], [a, b]
            if as_var(a) == Some(counter) && as_int(b) == Some(1)),
        _ => false,
    }
}

fn free_vars(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name) => {
            out.insert(name.clone());
        }
        Expr::Number(_) | Expr::Bool(_) | Expr::Nil => {}
        Expr::Primitive { args, .. } => args.iter().for_each(|a| free_vars(a, out)),
        Expr::Call { args, .. } => args.iter().for_each(|a| free_vars(a, out)),
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            free_vars(condition, out);
            free_vars(then_branch, out);
            free_vars(else_branch, out);
        }
        Expr::Let { bindings, body } => {
            bindings.iter().for_each(|(_, v)| free_vars(v, out));
            free_vars(body, out);
        }
        Expr::Cons(a, b) => {
            free_vars(a, out);
            free_vars(b, out);
        }
        Expr::Car(a) | Expr::Cdr(a) | Expr::Null(a) => free_vars(a, out),
    }
}

fn contains_call(expr: &Expr) -> bool {
    match expr {
        Expr::Call { .. } => true,
        Expr::Primitive { args, .. } => args.iter().any(contains_call),
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => contains_call(condition) || contains_call(then_branch) || contains_call(else_branch),
        Expr::Let { bindings, body } => {
            bindings.iter().any(|(_, v)| contains_call(v)) || contains_call(body)
        }
        Expr::Cons(a, b) => contains_call(a) || contains_call(b),
        Expr::Car(a) | Expr::Cdr(a) | Expr::Null(a) => contains_call(a),
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => false,
    }
}

/// Replace every `Var(from)` with `to`.
fn substitute(expr: &Expr, from: &str, to: &Expr) -> Expr {
    match expr {
        Expr::Var(name) if name == from => to.clone(),
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => expr.clone(),
        Expr::Primitive { op, args } => Expr::Primitive {
            op: *op,
            args: args.iter().map(|a| substitute(a, from, to)).collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: callee.clone(),
            args: args.iter().map(|a| substitute(a, from, to)).collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(substitute(condition, from, to)),
            then_branch: Box::new(substitute(then_branch, from, to)),
            else_branch: Box::new(substitute(else_branch, from, to)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .iter()
                .map(|(n, v)| (n.clone(), substitute(v, from, to)))
                .collect(),
            body: Box::new(substitute(body, from, to)),
        },
        Expr::Cons(a, b) => Expr::Cons(
            Box::new(substitute(a, from, to)),
            Box::new(substitute(b, from, to)),
        ),
        Expr::Car(a) => Expr::Car(Box::new(substitute(a, from, to))),
        Expr::Cdr(a) => Expr::Cdr(Box::new(substitute(a, from, to))),
        Expr::Null(a) => Expr::Null(Box::new(substitute(a, from, to))),
    }
}

/// Match the counted-reduction idiom, or `None` if this definition is not one.
fn recognize(definition: &Definition) -> Option<Reduction> {
    let [counter, bound, accumulator] = &definition.params[..] else {
        return None;
    };

    // (if (> i n) acc <call>)
    let Expr::If {
        condition,
        then_branch,
        else_branch,
    } = &definition.body
    else {
        return None;
    };
    match condition.as_ref() {
        Expr::Primitive {
            op: Primitive::Gt,
            args,
        } if matches!(&args[..], [a, b]
            if as_var(a) == Some(counter.as_str())
                && as_var(b) == Some(bound.as_str())) => {}
        _ => return None,
    }
    if as_var(then_branch) != Some(accumulator.as_str()) {
        return None;
    }

    // (<f> (+ i 1) n (OP acc G))
    let Expr::Call { callee, args } = else_branch.as_ref() else {
        return None;
    };
    if callee != &definition.name || args.len() != 3 {
        return None;
    }
    if !is_add_one_of(&args[0], counter) || as_var(&args[1]) != Some(bound.as_str()) {
        return None;
    }

    // (OP acc G) with OP associative and commutative under wrapping arithmetic
    let Expr::Primitive { op, args: parts } = &args[2] else {
        return None;
    };
    let combiner = *op;
    if !matches!(combiner, Primitive::Add | Primitive::Mul) {
        return None;
    }
    let [accumulated, element] = &parts[..] else {
        return None;
    };
    if as_var(accumulated) != Some(accumulator.as_str()) {
        return None;
    }

    // `element` may read only the counter, so substituting a loop bound for it
    // is safe, and it must not itself call anything.
    let mut vars = BTreeSet::new();
    free_vars(element, &mut vars);
    if !vars.iter().all(|v| v == counter) || contains_call(element) {
        return None;
    }

    Some(Reduction {
        counter: counter.clone(),
        combiner,
        element: element.clone(),
    })
}

/// `def <name>(lo, hi)` reducing `[lo, hi]` in a balanced tree.
///
/// Midpoints are `lo + (hi - lo) / 2` rather than `(lo + hi) / 2`: the
/// subtraction cannot wrap for the non-negative bounds this pass requires, and
/// the form is well founded for any `lo <= hi`.
fn balanced_definition(name: &str, reduction: &Reduction) -> Definition {
    let lo = "shapeLo".to_string();
    let hi = "shapeHi".to_string();
    let mid = "shapeMid";
    let midpoint = Expr::Primitive {
        op: Primitive::Add,
        args: vec![
            Expr::Var(lo.clone()),
            Expr::Primitive {
                op: Primitive::Div,
                args: vec![
                    Expr::Primitive {
                        op: Primitive::Sub,
                        args: vec![Expr::Var(hi.clone()), Expr::Var(lo.clone())],
                    },
                    Expr::Number(2),
                ],
            },
        ],
    };
    let descend = Expr::Let {
        bindings: vec![(mid.to_string(), midpoint)],
        body: Box::new(combine(
            reduction.combiner,
            Expr::Call {
                callee: name.to_string(),
                args: vec![Expr::Var(lo.clone()), Expr::Var(mid.to_string())],
            },
            Expr::Call {
                callee: name.to_string(),
                args: vec![
                    Expr::Primitive {
                        op: Primitive::Add,
                        args: vec![Expr::Var(mid.to_string()), Expr::Number(1)],
                    },
                    Expr::Var(hi.clone()),
                ],
            },
        )),
    };
    let body = Expr::If {
        // An empty range is neutral, which also makes chained splits total.
        condition: Box::new(Expr::Primitive {
            op: Primitive::Gt,
            args: vec![Expr::Var(lo.clone()), Expr::Var(hi.clone())],
        }),
        then_branch: Box::new(identity(reduction.combiner)),
        else_branch: Box::new(Expr::If {
            condition: Box::new(Expr::Primitive {
                op: Primitive::Eq,
                args: vec![Expr::Var(lo.clone()), Expr::Var(hi.clone())],
            }),
            then_branch: Box::new(substitute(
                &reduction.element,
                &reduction.counter,
                &Expr::Var(lo.clone()),
            )),
            else_branch: Box::new(descend),
        }),
    };
    Definition {
        name: name.to_string(),
        params: vec![lo, hi],
        body,
    }
}

/// `k` chain segments over `[lo, hi]`, combined by a balanced tree.
///
/// Split points are compile-time constants, so the leaves are calls to the
/// original chain with literal bounds. An empty segment reduces to the
/// combiner's identity, which the chain already returns.
fn chunked_tree(name: &str, reduction: &Reduction, lo: i64, hi: i64, k: u32) -> Expr {
    if k <= 1 || lo >= hi {
        return Expr::Call {
            callee: name.to_string(),
            args: vec![
                Expr::Number(lo),
                Expr::Number(hi),
                identity(reduction.combiner),
            ],
        };
    }
    let left = k / 2;
    let right = k - left;
    let mid = lo + (hi - lo) / 2;
    combine(
        reduction.combiner,
        chunked_tree(name, reduction, lo, mid, left),
        chunked_tree(name, reduction, mid + 1, hi, right),
    )
}

/// The generated helper's name.
///
/// It must contain no underscore: `codegen::mangle` renders `_` as `_95_`, and
/// Bend rejects any top-level name containing `__`, so two underscores in the
/// source name would produce an unloadable program.
fn helper_name(name: &str) -> String {
    format!("{name}ShapeBalanced")
}

/// Re-emit the top-level reduction in `shape`, if it is one this pass recognizes.
pub fn apply(program: Program, shape: Shape) -> Program {
    if shape == Shape::AsWritten {
        return program;
    }

    let reductions = program
        .definitions
        .iter()
        .filter_map(|d| recognize(d).map(|r| (d.name.clone(), r)))
        .collect::<std::collections::BTreeMap<_, _>>();
    if reductions.is_empty() {
        return program;
    }

    // Destructure the top-level call into owned pieces so the borrow of
    // `program` ends before the rewrite moves it.
    let Some(plan) = (|| {
        let Expr::Call { callee, args } = program.body.as_ref()? else {
            return None;
        };
        let reduction = reductions.get(callee)?;
        let [lo_expr, hi_expr, init] = &args[..] else {
            return None;
        };
        // Non-negative literals only: see the module comment on midpoints.
        let (lo, hi) = (as_int(lo_expr)?, as_int(hi_expr)?);
        if lo < 0 || hi < lo {
            return None;
        }
        Some((callee.clone(), lo, hi, init.clone(), reduction.clone()))
    })() else {
        return program;
    };
    let (callee, lo, hi, init, reduction) = plan;

    let mut program = program;
    let reduce = match shape {
        Shape::AsWritten => unreachable!("handled above"),
        Shape::Balanced => {
            let helper = helper_name(&callee);
            program
                .definitions
                .push(balanced_definition(&helper, &reduction));
            Expr::Call {
                callee: helper,
                args: vec![Expr::Number(lo), Expr::Number(hi)],
            }
        }
        Shape::Chunked(k) => chunked_tree(&callee, &reduction, lo, hi, k),
    };
    program.body = Some(combine(reduction.combiner, init, reduce));
    program
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_with_options;
    use crate::optimize::Options;

    fn compile_with(source: &str, shape: Shape) -> String {
        compile_with_options(
            source,
            Options {
                shape,
                ..Options::default()
            },
        )
        .expect("compiles")
    }

    const CHAIN: &str = "(define (run i n acc)\n\
           (if (> i n)\n\
               acc\n\
               (run (+ i 1) n (+ acc i))))\n\
         (run 1 64 0)";

    #[test]
    fn balanced_emits_a_tree_helper() {
        let output = compile_with(CHAIN, Shape::Balanced);
        assert!(output.contains("runShapeBalanced"), "{output}");
    }

    #[test]
    fn chunked_leaves_call_the_chain_with_literal_bounds() {
        let output = compile_with(CHAIN, Shape::Chunked(4));
        assert!(!output.contains("ShapeBalanced"), "{output}");
        // Four segments means four chain entries, all with literal bounds.
        assert_eq!(output.matches("s_run(+").count(), 4, "{output}");
    }

    #[test]
    fn shapes_agree_on_a_map_but_the_source_stays_untouched() {
        // A reduction whose combiner is not associative must never be rewritten.
        let subtraction = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (- acc i))))\n\
             (run 1 64 0)";
        assert_eq!(
            compile_with(subtraction, Shape::Balanced),
            compile_with(subtraction, Shape::AsWritten),
        );
        // Nor may a call with non-literal bounds be rewritten.
        let open = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (+ acc i))))\n\
             (define (start n) (run 1 n 0))\n\
             (start 64)";
        assert_eq!(
            compile_with(open, Shape::Balanced),
            compile_with(open, Shape::AsWritten),
        );
    }

    #[test]
    fn shape_specs_round_trip() {
        assert_eq!(Shape::parse("asis"), Some(Shape::AsWritten));
        assert_eq!(Shape::parse("balanced"), Some(Shape::Balanced));
        assert_eq!(Shape::parse("chunked:8"), Some(Shape::Chunked(8)));
        assert_eq!(Shape::parse("chunked:0"), None);
        assert_eq!(Shape::parse("chunked:x"), None);
        assert_eq!(Shape::parse("sideways"), None);
    }
}
