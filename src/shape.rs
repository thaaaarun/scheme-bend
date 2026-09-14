//! Shape selection for recognized associative reductions.
//!
//! HVM discovers parallelism from the *shape* of the interaction net rather
//! than from annotations, so the clearest lever a compiler has over parallelism
//! is the shape of the code it emits. Measurements back this: re-associating
//! one sum changed nothing but its shape, and ran 5.2x faster on eight threads
//! for +0.23% interactions (`benchmarks/sum-squares.scm`).
//!
//! This pass recognizes a class of counted reductions and re-emits one in a
//! chosen shape:
//!
//! * `AsWritten` -- leave the chain alone.
//! * `Balanced` -- re-associate the range into a binary tree. Span becomes
//!   O(log N) and the redex frontier is wide, at the cost of one extra combine
//!   node per element.
//! * `Chunked(k)` -- split the range into `k` chain segments and combine those
//!   with a balanced tree. Keeps the chain's interaction count while exposing a
//!   bounded frontier. Needs the bound to be a parameter, so a segment can be
//!   written as one call; see `segment_call`.
//!
//! The recognized form is a counted reduction whose combiner is associative and
//! commutative under wrapping arithmetic:
//!
//! ```text
//! (define (f i n acc)                     (define (f k acc)
//!   (if (> i n)                             (if (= k 0)
//!       acc                                     acc
//!       (f (+ i 1) n (OP acc G))))              (f (- k 1) (OP acc G))))
//! ```
//!
//! Either direction, either guard; the counter may be compared against a
//! parameter or a literal.
//!
//! Re-association is legal because the target's arithmetic wraps modulo 2^24,
//! which is a ring: `+` and `*` are associative and commutative there. `-` and
//! truncating `/` are not, so they are not whitelisted. Each combiner has an
//! identity (`0` for `+`, `1` for `*`) which is what lets a range be split
//! anywhere, including into empty segments.
//!
//! A modular accumulator such as `(mod- (+ acc G) M)` is deliberately *not*
//! recognized, even though `mod` distributes over `+` in exact arithmetic.
//! Truncating `mod` composes with `+` only up to a signed representative, and
//! the target contributes a *second* modulus -- the 2^24 wrap. Composing two
//! different moduli does not re-associate: which representative is chosen
//! depends on evaluation order, so the rewrite is safe only while no
//! intermediate wraps. The benchmarks here happen to satisfy that (modulus
//! 4,000,000 < 2^23 with elements below it keeps `acc + y < 2^24`), but that is
//! a property of the data rather than of the rewrite. A program with larger
//! magnitudes silently got a different answer, and proving the bound needs
//! value analysis this pass does not have, so the rewrite is refused outright.
//!
//! A call site is rewritten only when it is worth it. Below
//! `MIN_RESHAPE_SPAN` iterations the extra combine nodes cannot repay
//! themselves, so only reductions whose element contains a call -- where the
//! per-element work swamps the combine -- are reshaped at small sizes.
//!
//! Limits, all deliberate: the counter's guard must have the counter on the
//! left; the element may read only the counter (it may call other functions);
//! the combiner must be a bare `+` or `*`; and every other parameter must be
//! passed through unchanged.

use crate::ir::{Definition, Expr, Primitive, Program};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Below this many iterations a reduction is only reshaped if its element
/// contains a call, where the combine nodes are lost in the per-element work.
const MIN_RESHAPE_SPAN: i64 = 1024;

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

/// The comparison in the guard, which is the *stop* condition: the `then` branch
/// is the accumulator, so iteration continues while this is false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// What the counter is compared against in the guard.
#[derive(Debug, Clone)]
enum Bound {
    Literal(i64),
    /// Index into the reduction's parameters.
    Param(usize),
}

/// A recognized reduction, decomposed into the pieces a shape rewrite needs.
#[derive(Clone)]
struct Reduction {
    name: String,
    params: Vec<String>,
    /// Parameter index of the counter and of the accumulator.
    counter: usize,
    accumulator: usize,
    bound: Bound,
    guard: Guard,
    /// Counter step per iteration: `+1` or `-1`.
    delta: i64,
    /// The associative, commutative combiner. Only a bare `+` or `*`.
    combiner: Primitive,
    /// The per-element expression, which may read only the counter.
    element: Expr,
}

impl Reduction {
    fn counter_name(&self) -> &str {
        &self.params[self.counter]
    }

    /// The identity element of the combiner, so an empty range is neutral.
    fn identity(&self) -> Expr {
        match self.combiner {
            Primitive::Mul => Expr::Number(1),
            _ => Expr::Number(0),
        }
    }

    /// Combine two values. Sound under wrapping: 2^24 is a ring, so `+` and
    /// `*` agree at every association.
    fn combine(&self, left: Expr, right: Expr) -> Expr {
        Expr::Primitive {
            op: self.combiner,
            args: vec![left, right],
        }
    }

    /// The element for one counter value.
    fn leaf(&self, value: Expr) -> Expr {
        substitute(&self.element, self.counter_name(), &value)
    }

    /// The inclusive ascending counter range the loop visits, given a call.
    ///
    /// Iteration continues while the guard is false, so the guard's strictness
    /// decides which end is included. Direction fixes which end is the start.
    fn range(&self, args: &[Expr]) -> Option<(i64, i64)> {
        let start = as_int(args.get(self.counter)?)?;
        let stop = match &self.bound {
            Bound::Literal(value) => *value,
            Bound::Param(index) => as_int(args.get(*index)?)?,
        };
        let (lo, hi) = if self.delta > 0 {
            let last = match self.guard {
                // Continue while `counter <= stop`.
                Guard::Gt => stop,
                // Continue while `counter < stop`.
                Guard::Ge => stop - 1,
                // Continue while `counter != stop`, approaching from below.
                Guard::Eq => stop - 1,
                Guard::Lt | Guard::Le => return None,
            };
            (start, last)
        } else {
            let last = match self.guard {
                // Continue while `counter != stop`, approaching from above.
                Guard::Eq => stop + 1,
                // Continue while `counter >= stop`.
                Guard::Lt => stop,
                // Continue while `counter > stop`.
                Guard::Le => stop + 1,
                Guard::Gt | Guard::Ge => return None,
            };
            (last, start)
        };
        // Non-negative bounds keep every synthesized midpoint in range.
        if lo < 0 || lo > hi {
            return None;
        }
        Some((lo, hi))
    }

    /// A call to the original chain covering the counter range `[lo, hi]`, or
    /// `None` when the reduction cannot express a sub-range as one call.
    ///
    /// Only the ascending form whose bound is a parameter qualifies: then
    /// `f(lo, hi, 0)` walks exactly `lo..=hi`. The descending form compares the
    /// counter against a literal baked into its body, so a sub-range would need
    /// a different literal and cannot be expressed; `balanced` handles it.
    fn segment_call(&self, lo: i64, hi: i64) -> Option<Expr> {
        let ascending = self.delta > 0
            && self.guard == Guard::Gt
            && self.counter == 0
            && self.accumulator == 2
            && matches!(self.bound, Bound::Param(1))
            && self.params.len() == 3;
        if !ascending {
            return None;
        }
        let mut args = vec![Expr::Number(lo), Expr::Number(hi), self.identity()];
        let _ = &mut args;
        Some(Expr::Call {
            callee: self.name.clone(),
            args,
        })
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

/// `(+ counter 1)` or `(- counter 1)`; returns the step.
fn step_of(expr: &Expr, counter: &str) -> Option<i64> {
    let Expr::Primitive { op, args } = expr else {
        return None;
    };
    let [a, b] = &args[..] else {
        return None;
    };
    if as_var(a) != Some(counter) {
        return None;
    }
    let one = as_int(b)?;
    if one != 1 {
        return None;
    }
    match op {
        Primitive::Add => Some(1),
        Primitive::Sub => Some(-1),
        _ => None,
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

/// Decompose a bare `(OP acc X)`. A wrapped combiner is refused; see the
/// module note on modular accumulators.
fn combiner_of(expr: &Expr, accumulator: &str) -> Option<(Primitive, Expr)> {
    let Expr::Primitive { op, args } = expr else {
        return None;
    };
    if !matches!(op, Primitive::Add | Primitive::Mul) {
        return None;
    }
    let [accumulated, element] = &args[..] else {
        return None;
    };
    if as_var(accumulated) != Some(accumulator) {
        return None;
    }
    Some((*op, element.clone()))
}

/// Match the counted-reduction idiom, or `None` if this definition is not one.
fn recognize(definition: &Definition) -> Option<Reduction> {
    if definition.params.len() < 2 {
        return None;
    }
    // (if <guard> acc <self-call>)
    let Expr::If {
        condition,
        then_branch,
        else_branch,
    } = &definition.body
    else {
        return None;
    };
    let Expr::Call { callee, args } = else_branch.as_ref() else {
        return None;
    };
    if callee != &definition.name || args.len() != definition.params.len() {
        return None;
    }
    let accumulator = definition
        .params
        .iter()
        .position(|p| as_var(then_branch) == Some(p.as_str()))?;

    // Which argument position carries the combine tells us the accumulator.
    let (combiner, element) = combiner_of(&args[accumulator], &definition.params[accumulator])?;

    // Exactly one other position steps the counter.
    let mut counter = None;
    for (index, param) in definition.params.iter().enumerate() {
        if index == accumulator {
            continue;
        }
        match step_of(&args[index], param) {
            Some(delta) => {
                if counter.is_some() {
                    return None;
                }
                counter = Some((index, delta));
            }
            None => {
                // Every other parameter must be passed through unchanged.
                if as_var(&args[index]) != Some(param.as_str()) {
                    return None;
                }
            }
        }
    }
    let (counter, delta) = counter?;

    // (<guard> counter <bound>), counter on the left.
    let Expr::Primitive { op, args: cmp } = condition.as_ref() else {
        return None;
    };
    let guard = match op {
        Primitive::Eq => Guard::Eq,
        Primitive::Lt => Guard::Lt,
        Primitive::Le => Guard::Le,
        Primitive::Gt => Guard::Gt,
        Primitive::Ge => Guard::Ge,
        _ => return None,
    };
    let [lhs, rhs] = &cmp[..] else {
        return None;
    };
    if as_var(lhs) != Some(definition.params[counter].as_str()) {
        return None;
    }
    let bound = match rhs {
        Expr::Number(value) => Bound::Literal(*value),
        Expr::Var(name) => Bound::Param(
            definition
                .params
                .iter()
                .position(|p| p == name)
                .filter(|index| *index != counter && *index != accumulator)?,
        ),
        _ => return None,
    };

    // The element may read only the counter, and must not recurse into the
    // reduction itself; anything else would not survive being split.
    let mut vars = BTreeSet::new();
    free_vars(&element, &mut vars);
    if !vars.iter().all(|v| v == &definition.params[counter]) {
        return None;
    }
    if calls_name(&element, &definition.name) {
        return None;
    }

    Some(Reduction {
        name: definition.name.clone(),
        params: definition.params.clone(),
        counter,
        accumulator,
        bound,
        guard,
        delta,
        combiner,
        element,
    })
}

fn calls_name(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Call { callee, args } => {
            callee == name || args.iter().any(|a| calls_name(a, name))
        }
        Expr::Primitive { args, .. } => args.iter().any(|a| calls_name(a, name)),
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            calls_name(condition, name)
                || calls_name(then_branch, name)
                || calls_name(else_branch, name)
        }
        Expr::Let { bindings, body } => {
            bindings.iter().any(|(_, v)| calls_name(v, name)) || calls_name(body, name)
        }
        Expr::Cons(a, b) => calls_name(a, name) || calls_name(b, name),
        Expr::Car(a) | Expr::Cdr(a) | Expr::Null(a) => calls_name(a, name),
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => false,
    }
}

/// The generated helper's name.
///
/// Suffixed rather than prefixed so a reduction's own name stays the head of
/// it, which keeps the emitted Bend readable next to the function it came from.
fn helper_name(name: &str) -> String {
    format!("{name}ShapeBalanced")
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
    let descend = Expr::Let {
        bindings: vec![(
            mid.to_string(),
            Expr::Primitive {
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
            },
        )],
        body: Box::new(reduction.combine(
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
        then_branch: Box::new(reduction.identity()),
        else_branch: Box::new(Expr::If {
            condition: Box::new(Expr::Primitive {
                op: Primitive::Eq,
                args: vec![Expr::Var(lo.clone()), Expr::Var(hi.clone())],
            }),
            then_branch: Box::new(reduction.leaf(Expr::Var(lo.clone()))),
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
fn chunked_tree(reduction: &Reduction, lo: i64, hi: i64, k: u32) -> Option<Expr> {
    if k <= 1 || lo >= hi {
        return reduction.segment_call(lo, hi);
    }
    let left = k / 2;
    let right = k - left;
    let mid = lo + (hi - lo) / 2;
    Some(reduction.combine(
        chunked_tree(reduction, lo, mid, left)?,
        chunked_tree(reduction, mid + 1, hi, right)?,
    ))
}

/// Rewrites matching call sites anywhere in the program.
struct Rewriter<'a> {
    reductions: &'a BTreeMap<String, Reduction>,
    shape: Shape,
    generated: Vec<Definition>,
    helpers: BTreeSet<String>,
}

impl Rewriter<'_> {
    /// The replacement for a call, if this call site is worth reshaping.
    fn replacement(&mut self, callee: &str, args: &[Expr]) -> Option<Expr> {
        let reduction = self.reductions.get(callee)?;
        let (lo, hi) = reduction.range(args)?;
        // Small reductions only earn their keep when the element is heavy.
        if hi - lo + 1 < MIN_RESHAPE_SPAN && !contains_call(&reduction.element) {
            return None;
        }
        let init = args[reduction.accumulator].clone();
        let reduce = match self.shape {
            Shape::AsWritten => return None,
            Shape::Balanced => {
                let helper = helper_name(callee);
                if self.helpers.insert(helper.clone()) {
                    self.generated.push(balanced_definition(&helper, reduction));
                }
                Expr::Call {
                    callee: helper,
                    args: vec![Expr::Number(lo), Expr::Number(hi)],
                }
            }
            Shape::Chunked(k) => chunked_tree(reduction, lo, hi, k)?,
        };
        Some(reduction.combine(init, reduce))
    }

    /// Rebuild `expr`, replacing reshapeable calls. Replacements are returned
    /// whole rather than re-walked, so a generated call inside one cannot be
    /// rewritten again.
    fn expr(&mut self, expr: Expr) -> Expr {
        match expr {
            Expr::Call { callee, args } => {
                let args: Vec<Expr> = args.into_iter().map(|a| self.expr(a)).collect();
                if let Some(replacement) = self.replacement(&callee, &args) {
                    return replacement;
                }
                Expr::Call { callee, args }
            }
            Expr::Primitive { op, args } => Expr::Primitive {
                op,
                args: args.into_iter().map(|a| self.expr(a)).collect(),
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => Expr::If {
                condition: Box::new(self.expr(*condition)),
                then_branch: Box::new(self.expr(*then_branch)),
                else_branch: Box::new(self.expr(*else_branch)),
            },
            Expr::Let { bindings, body } => Expr::Let {
                bindings: bindings
                    .into_iter()
                    .map(|(n, v)| (n, self.expr(v)))
                    .collect(),
                body: Box::new(self.expr(*body)),
            },
            Expr::Cons(a, b) => Expr::Cons(Box::new(self.expr(*a)), Box::new(self.expr(*b))),
            Expr::Car(a) => Expr::Car(Box::new(self.expr(*a))),
            Expr::Cdr(a) => Expr::Cdr(Box::new(self.expr(*a))),
            Expr::Null(a) => Expr::Null(Box::new(self.expr(*a))),
            other => other,
        }
    }
}

/// Re-emit every recognized reduction call in `shape`.
pub fn apply(program: Program, shape: Shape) -> Program {
    if shape == Shape::AsWritten {
        return program;
    }
    let reductions = program
        .definitions
        .iter()
        .filter_map(|d| recognize(d).map(|r| (d.name.clone(), r)))
        .collect::<BTreeMap<_, _>>();
    if reductions.is_empty() {
        return program;
    }

    let mut program = program;
    let mut rewriter = Rewriter {
        reductions: &reductions,
        shape,
        generated: Vec::new(),
        helpers: BTreeSet::new(),
    };
    for definition in &mut program.definitions {
        let body = std::mem::replace(&mut definition.body, Expr::Nil);
        definition.body = rewriter.expr(body);
    }
    if let Some(body) = program.body.take() {
        program.body = Some(rewriter.expr(body));
    }
    program.definitions.append(&mut rewriter.generated);
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

    /// A big ascending reduction: `[1, 2048]` is over `MIN_RESHAPE_SPAN`.
    const ASCENDING: &str = "(define (run i n acc)\n\
           (if (> i n)\n\
               acc\n\
               (run (+ i 1) n (+ acc i))))\n\
         (run 1 2048 0)";

    /// The repo's `repeat` idiom: descending, equality guard against a literal
    /// bound, and an element that calls out. 2048 iterations so the span
    /// heuristic is not what decides it.
    const DESCENDING: &str = "(define (step k) (+ k k))\n\
         (define (run k acc)\n\
           (if (= k 0)\n\
               acc\n\
               (run (- k 1) (+ acc (step k)))))\n\
         (run 2048 0)";

    #[test]
    fn balanced_emits_a_tree_helper() {
        assert!(compile_with(ASCENDING, Shape::Balanced).contains("runShapeBalanced"));
    }

    #[test]
    fn recognizes_the_descending_equality_guard() {
        // The repo writes every `repeat` loop this way; it used to be invisible.
        assert!(compile_with(DESCENDING, Shape::Balanced).contains("runShapeBalanced"));
    }

    #[test]
    fn refuses_a_modular_accumulator() {
        // Truncating `mod` and the 2^24 wrap are two different moduli, so once
        // an intermediate can wrap the representative depends on evaluation
        // order. This program wraps and used to answer differently per shape.
        let source = "(define (mod- a m) (- a (* (/ a m) m)))\n\
             (define (run k acc)\n\
               (if (= k 0)\n\
                   acc\n\
                   (run (- k 1) (mod- (+ acc (mod- k 8388593)) 8388593))))\n\
             (run 2048 0)";
        assert_eq!(
            compile_with(source, Shape::Balanced),
            compile_with(source, Shape::AsWritten),
        );
    }

    #[test]
    fn still_rewrites_plain_wrapping_arithmetic() {
        // A bare combiner stays sound however it wraps: 2^24 is a ring, so `+`
        // and `*` give the same result at every association.
        let wrapping = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (+ acc (* i 8388593)))))\n\
             (run 1 4096 0)";
        assert!(compile_with(wrapping, Shape::Balanced).contains("runShapeBalanced"));
    }

    #[test]
    fn rewrites_calls_that_are_not_the_top_level_body() {
        // `once` wraps the reduction, so a top-level-only pass sees nothing.
        let source = bisect_the_repo_style();
        assert!(compile_with(source, Shape::Balanced).contains("ShapeBalanced"));
    }

    #[test]
    fn leaves_small_trivial_reductions_alone() {
        // 64 iterations of `(+ acc i)`: below the span heuristic, and the
        // element is trivial, so the combine nodes would not be repaid.
        let small = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (+ acc i))))\n\
             (run 1 64 0)";
        assert_eq!(
            compile_with(small, Shape::Balanced),
            compile_with(small, Shape::AsWritten),
        );
    }

    #[test]
    fn chunked_leaves_call_the_chain_with_literal_bounds() {
        let output = compile_with(ASCENDING, Shape::Chunked(4));
        assert!(!output.contains("ShapeBalanced"), "{output}");
        assert_eq!(output.matches("s_run(+").count(), 4, "{output}");
    }

    #[test]
    fn never_rewrites_a_non_associative_combiner() {
        let subtraction = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (- acc i))))\n\
             (run 1 2048 0)";
        assert_eq!(
            compile_with(subtraction, Shape::Balanced),
            compile_with(subtraction, Shape::AsWritten),
        );
    }

    #[test]
    fn never_rewrites_an_element_that_reads_more_than_the_counter() {
        // `n` is not the counter, so the element cannot be moved into a leaf.
        let source = "(define (run i n acc)\n\
               (if (> i n)\n\
                   acc\n\
                   (run (+ i 1) n (+ acc (- n i)))))\n\
             (run 1 2048 0)";
        assert_eq!(
            compile_with(source, Shape::Balanced),
            compile_with(source, Shape::AsWritten),
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

    /// The repo's repeat idiom, wrapped in a zero-argument helper.
    fn bisect_the_repo_style() -> &'static str {
        "(define (step k) (+ k k))\n\
         (define (run k acc)\n\
           (if (= k 0)\n\
               acc\n\
               (run (- k 1) (+ acc (step k)))))\n\
         (define (once) (run 2048 0))\n\
         (once)"
    }
}
