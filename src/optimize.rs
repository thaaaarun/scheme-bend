//! Bend-aware, source-level optimizations over the small functional IR.
//!
//! These passes never change the target: their output is still ordinary Bend
//! source.  Their job is to reduce the number of calls and numeric interaction
//! nodes that the Bend compiler needs to lower.

use crate::ir::{Definition, Expr, Primitive, Program};
use crate::shape::Shape;
use std::collections::{BTreeMap, BTreeSet};

/// Tuning knobs for the whole-program optimizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Inline small, call-free top-level helpers at their direct call sites.
    pub inline_helpers: bool,
    /// Share repeated total numeric expressions, such as `(* zr zr)`.
    pub common_subexpressions: bool,
    /// Number of scalar tail-recursion iterations to place in one emitted
    /// Bend function body. `1` leaves recursion unchanged. More fusion means
    /// fewer recursive call expansions but a larger emitted function.
    ///
    /// Defaults to 8 because that is where wall clock stops improving. On the
    /// 256x256 Mandelbrot benchmark, interaction count keeps falling all the
    /// way to 64 (254.9M at 4, 239.9M at 8, 226.0M at 64), but median wall
    /// clock is 0.50s at 4, 0.49s at 8 and 0.56s at 16: past 8 the larger
    /// function body costs more than the saved call expansions return. The
    /// interaction count alone is not a safe proxy for speed.
    pub tail_unroll: usize,
    /// Substitute literal `let` bindings into their uses so that conditions
    /// over them can fold. Without this, an inlined constant argument stays a
    /// variable read and both branches survive into the emitted Bend.
    pub constant_propagation: bool,
    /// Which shape to emit for a recognized associative reduction. Shape is
    /// the lever over parallelism, since HVM finds parallelism in the net
    /// rather than from annotations; `benchmarks/shape_search.py` picks it.
    pub shape: Shape,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            inline_helpers: true,
            common_subexpressions: true,
            tail_unroll: 8,
            constant_propagation: true,
            shape: Shape::AsWritten,
        }
    }
}

pub fn optimize(program: Program, options: Options) -> Program {
    // Shape first: it can introduce a new definition for the other passes to
    // work on, and every later pass preserves the shape it chooses.
    let mut program = crate::shape::apply(program, options.shape);
    let mut fresh = Fresh::from_program(&program);

    if options.inline_helpers {
        let helpers = program
            .definitions
            .iter()
            .filter(|definition| inline_candidate(definition))
            .map(|definition| (definition.name.clone(), definition.clone()))
            .collect::<BTreeMap<_, _>>();
        if !helpers.is_empty() {
            for definition in &mut program.definitions {
                definition.body = inline_expr(definition.body.clone(), &helpers, &mut fresh);
            }
            if let Some(body) = program.body.take() {
                program.body = Some(inline_expr(body, &helpers, &mut fresh));
            }
        }
    }

    if options.tail_unroll > 1 {
        for definition in &mut program.definitions {
            definition.body = unroll_tail_recurrence(definition, options.tail_unroll, &mut fresh);
        }
    }

    for definition in &mut program.definitions {
        definition.body = settle(definition.body.clone(), options.constant_propagation);
        if options.common_subexpressions {
            definition.body = share_common(definition.body.clone(), &mut fresh);
        }
    }
    if let Some(body) = program.body.take() {
        let body = settle(body, options.constant_propagation);
        program.body = Some(if options.common_subexpressions {
            share_common(body, &mut fresh)
        } else {
            body
        });
    }

    program
}

/// A helper is safe to inline when it is small and its body has no calls. This
/// deliberately excludes recursion and avoids an unbounded inlining cascade.
fn inline_candidate(definition: &Definition) -> bool {
    !contains_call(&definition.body) && expr_size(&definition.body) <= 24
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
            bindings.iter().any(|(_, value)| contains_call(value)) || contains_call(body)
        }
        Expr::Cons(head, tail) => contains_call(head) || contains_call(tail),
        Expr::Car(value) | Expr::Cdr(value) | Expr::Null(value) => contains_call(value),
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => false,
    }
}

fn expr_size(expr: &Expr) -> usize {
    match expr {
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => 1,
        Expr::Call { args, .. } | Expr::Primitive { args, .. } => {
            1 + args.iter().map(expr_size).sum::<usize>()
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => 1 + expr_size(condition) + expr_size(then_branch) + expr_size(else_branch),
        Expr::Let { bindings, body } => {
            1 + bindings
                .iter()
                .map(|(_, value)| expr_size(value))
                .sum::<usize>()
                + expr_size(body)
        }
        Expr::Cons(head, tail) => 1 + expr_size(head) + expr_size(tail),
        Expr::Car(value) | Expr::Cdr(value) | Expr::Null(value) => 1 + expr_size(value),
    }
}

fn inline_expr(expr: Expr, helpers: &BTreeMap<String, Definition>, fresh: &mut Fresh) -> Expr {
    match expr {
        Expr::Call { callee, args } => {
            let args = args
                .into_iter()
                .map(|arg| inline_expr(arg, helpers, fresh))
                .collect::<Vec<_>>();
            let Some(helper) = helpers.get(&callee) else {
                return Expr::Call { callee, args };
            };
            if helper.params.len() != args.len() {
                // The normal compiler will report the arity problem later; do
                // not make optimization change its diagnostic.
                return Expr::Call { callee, args };
            }
            // Variables can be substituted directly. Besides avoiding a
            // needless bind/copy, this preserves the identical expression
            // shape needed for CSE across the caller and inlined helper.
            if args.iter().all(|arg| matches!(arg, Expr::Var(_))) {
                let environment = helper
                    .params
                    .iter()
                    .zip(args.iter())
                    .map(|(parameter, argument)| {
                        let Expr::Var(argument) = argument else {
                            unreachable!()
                        };
                        (parameter.clone(), argument.clone())
                    })
                    .collect::<BTreeMap<_, _>>();
                return clone_with_renaming(&helper.body, &environment, fresh);
            }
            let mut environment = BTreeMap::new();
            let mut bindings = Vec::new();
            for (parameter, argument) in helper.params.iter().zip(args) {
                let local = fresh.local();
                environment.insert(parameter.clone(), local.clone());
                bindings.push((local, argument));
            }
            let body = clone_with_renaming(&helper.body, &environment, fresh);
            if bindings.is_empty() {
                body
            } else {
                Expr::Let {
                    bindings,
                    body: Box::new(body),
                }
            }
        }
        Expr::Primitive { op, args } => Expr::Primitive {
            op,
            args: args
                .into_iter()
                .map(|arg| inline_expr(arg, helpers, fresh))
                .collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(inline_expr(*condition, helpers, fresh)),
            then_branch: Box::new(inline_expr(*then_branch, helpers, fresh)),
            else_branch: Box::new(inline_expr(*else_branch, helpers, fresh)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .into_iter()
                .map(|(name, value)| (name, inline_expr(value, helpers, fresh)))
                .collect(),
            body: Box::new(inline_expr(*body, helpers, fresh)),
        },
        Expr::Cons(head, tail) => Expr::Cons(
            Box::new(inline_expr(*head, helpers, fresh)),
            Box::new(inline_expr(*tail, helpers, fresh)),
        ),
        Expr::Car(value) => Expr::Car(Box::new(inline_expr(*value, helpers, fresh))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(inline_expr(*value, helpers, fresh))),
        Expr::Null(value) => Expr::Null(Box::new(inline_expr(*value, helpers, fresh))),
        atom => atom,
    }
}

/// Expands a closed, scalar tail recurrence by a fixed factor. The expansion is
/// still Bend source: it only replaces several REF-call reductions with one
/// larger ordinary function body. Every intermediate state is bound first, so
/// Scheme's eager, simultaneous call-argument semantics are preserved.
fn unroll_tail_recurrence(definition: &Definition, factor: usize, fresh: &mut Fresh) -> Expr {
    if !is_scalar_tail_recurrence(&definition.body, &definition.name) {
        return definition.body.clone();
    }
    // Bend's net-size guard is expressed in HVM nodes, not source expressions.
    // This source estimate keeps generic programs from exploding while still
    // admitting a numeric recurrence at the largest factor the CLI allows.
    // At 192 an escape-style body of ~33 expressions was silently refused for
    // factors above four, which left `--tail-unroll 6/8` behaving like `1`.
    if expr_size(&definition.body).saturating_mul(factor) > 8192 {
        return definition.body.clone();
    }
    expand_tail(
        &definition.body,
        definition,
        factor.saturating_sub(1),
        fresh,
    )
}

fn is_scalar_tail_recurrence(expr: &Expr, name: &str) -> bool {
    let mut saw_self_call = false;
    scalar_tail_calls_only(expr, name, true, &mut saw_self_call) && saw_self_call
}

fn scalar_tail_calls_only(expr: &Expr, name: &str, tail: bool, saw_self_call: &mut bool) -> bool {
    match expr {
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) => true,
        Expr::Nil | Expr::Cons(_, _) | Expr::Car(_) | Expr::Cdr(_) | Expr::Null(_) => false,
        Expr::Primitive { args, .. } => args
            .iter()
            .all(|argument| scalar_tail_calls_only(argument, name, false, saw_self_call)),
        Expr::Call { callee, args } => {
            if callee != name || !tail {
                return false;
            }
            *saw_self_call = true;
            args.iter()
                .all(|argument| scalar_tail_calls_only(argument, name, false, saw_self_call))
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            scalar_tail_calls_only(condition, name, false, saw_self_call)
                && scalar_tail_calls_only(then_branch, name, tail, saw_self_call)
                && scalar_tail_calls_only(else_branch, name, tail, saw_self_call)
        }
        Expr::Let { bindings, body } => {
            bindings
                .iter()
                .all(|(_, value)| scalar_tail_calls_only(value, name, false, saw_self_call))
                && scalar_tail_calls_only(body, name, tail, saw_self_call)
        }
    }
}

fn expand_tail(expr: &Expr, definition: &Definition, remaining: usize, fresh: &mut Fresh) -> Expr {
    match expr {
        Expr::Call { callee, args } if callee == &definition.name && remaining > 0 => {
            let mut environment = BTreeMap::new();
            let mut bindings = Vec::new();
            for (parameter, argument) in definition.params.iter().zip(args.iter()) {
                // Bind every next-state argument exactly once. In particular,
                // retaining a local name for an invariant input lets Bend's
                // linearizer arrange its copies more cheaply than repeatedly
                // wiring the caller's port through the expanded recurrence.
                let local = fresh.local();
                environment.insert(parameter.clone(), local.clone());
                bindings.push((local, argument.clone()));
            }
            let activated = clone_with_renaming(&definition.body, &environment, fresh);
            Expr::Let {
                bindings,
                body: Box::new(expand_tail(&activated, definition, remaining - 1, fresh)),
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: condition.clone(),
            then_branch: Box::new(expand_tail(then_branch, definition, remaining, fresh)),
            else_branch: Box::new(expand_tail(else_branch, definition, remaining, fresh)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings.clone(),
            body: Box::new(expand_tail(body, definition, remaining, fresh)),
        },
        other => other.clone(),
    }
}

/// Clone an expression, replacing variables in `environment` and giving each
/// nested binding a fresh name. The source IR has already alpha-renamed locals,
/// but cloned call bodies need a second round of freshness.
fn clone_with_renaming(
    expr: &Expr,
    environment: &BTreeMap<String, String>,
    fresh: &mut Fresh,
) -> Expr {
    match expr {
        Expr::Var(name) => Expr::Var(
            environment
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
        ),
        Expr::Primitive { op, args } => Expr::Primitive {
            op: *op,
            args: args
                .iter()
                .map(|arg| clone_with_renaming(arg, environment, fresh))
                .collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: callee.clone(),
            args: args
                .iter()
                .map(|arg| clone_with_renaming(arg, environment, fresh))
                .collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(clone_with_renaming(condition, environment, fresh)),
            then_branch: Box::new(clone_with_renaming(then_branch, environment, fresh)),
            else_branch: Box::new(clone_with_renaming(else_branch, environment, fresh)),
        },
        Expr::Let { bindings, body } => {
            let values = bindings
                .iter()
                .map(|(name, value)| (name.clone(), clone_with_renaming(value, environment, fresh)))
                .collect::<Vec<_>>();
            let mut body_environment = environment.clone();
            let bindings = values
                .into_iter()
                .map(|(name, value)| {
                    let local = fresh.local();
                    body_environment.insert(name, local.clone());
                    (local, value)
                })
                .collect();
            Expr::Let {
                bindings,
                body: Box::new(clone_with_renaming(body, &body_environment, fresh)),
            }
        }
        Expr::Cons(head, tail) => Expr::Cons(
            Box::new(clone_with_renaming(head, environment, fresh)),
            Box::new(clone_with_renaming(tail, environment, fresh)),
        ),
        Expr::Car(value) => Expr::Car(Box::new(clone_with_renaming(value, environment, fresh))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(clone_with_renaming(value, environment, fresh))),
        Expr::Null(value) => Expr::Null(Box::new(clone_with_renaming(value, environment, fresh))),
        Expr::Number(number) => Expr::Number(*number),
        Expr::Bool(boolean) => Expr::Bool(*boolean),
        Expr::Nil => Expr::Nil,
    }
}

/// Run simplification and literal propagation to a bounded fixed point.
///
/// The two interact: folding exposes new literal bindings, and propagation
/// exposes new foldable conditions.
fn settle(expr: Expr, propagate: bool) -> Expr {
    let mut expr = expr;
    for _ in 0..4 {
        let before = expr.clone();
        if propagate {
            expr = substitute_literal_bindings(expr);
            expr = drop_unused_bindings(expr);
        }
        expr = simplify(expr);
        if expr == before {
            break;
        }
    }
    expr
}

fn is_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Number(_) | Expr::Bool(_))
}

fn map_children(expr: Expr, f: impl Fn(Expr) -> Expr) -> Expr {
    match expr {
        Expr::Primitive { op, args } => Expr::Primitive {
            op,
            args: args.into_iter().map(&f).collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args.into_iter().map(&f).collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(f(*condition)),
            then_branch: Box::new(f(*then_branch)),
            else_branch: Box::new(f(*else_branch)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .into_iter()
                .map(|(name, value)| (name, f(value)))
                .collect(),
            body: Box::new(f(*body)),
        },
        Expr::Cons(head, tail) => Expr::Cons(Box::new(f(*head)), Box::new(f(*tail))),
        Expr::Car(value) => Expr::Car(Box::new(f(*value))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(f(*value))),
        Expr::Null(value) => Expr::Null(Box::new(f(*value))),
        leaf => leaf,
    }
}

fn replace_vars(expr: Expr, values: &BTreeMap<String, Expr>) -> Expr {
    if values.is_empty() {
        return expr;
    }
    match expr {
        Expr::Var(name) => values.get(&name).cloned().unwrap_or(Expr::Var(name)),
        other => map_children(other, |child| replace_vars(child, values)),
    }
}

/// Replace literal-valued `let` bindings with the literal at each use site.
///
/// The inliner already substitutes constant arguments, but it leaves them as
/// bindings, so a condition such as `if mode` remains a variable read and never
/// folds -- both arms survive into the emitted Bend. Literals are immediates in
/// HVM, so copying one to each use site is free, and removing the binding also
/// removes a copy. Names are already alpha-renamed, so no shadowing is possible.
fn substitute_literal_bindings(expr: Expr) -> Expr {
    match expr {
        Expr::Let { bindings, body } => {
            let mut literals = BTreeMap::new();
            let mut kept = Vec::new();
            for (name, value) in bindings {
                let value = replace_vars(substitute_literal_bindings(value), &literals);
                if is_literal(&value) {
                    literals.insert(name, value);
                } else {
                    kept.push((name, value));
                }
            }
            let body = substitute_literal_bindings(replace_vars(*body, &literals));
            if kept.is_empty() {
                body
            } else {
                Expr::Let {
                    bindings: kept,
                    body: Box::new(body),
                }
            }
        }
        other => map_children(other, substitute_literal_bindings),
    }
}

/// Whether dropping a computation as unused is guaranteed safe.
///
/// Calls are excluded because removing one could change termination, and
/// division is excluded because removing one could delete an error the original
/// program performed. Anything else here is total and side-effect free.
fn droppable(expr: &Expr) -> bool {
    match expr {
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => true,
        Expr::Primitive {
            op: Primitive::Div, ..
        } => false,
        Expr::Primitive { args, .. } => args.iter().all(droppable),
        _ => false,
    }
}

fn collect_uses(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name) => {
            names.insert(name.clone());
        }
        Expr::Call { callee, args } => {
            names.insert(callee.clone());
            for arg in args {
                collect_uses(arg, names);
            }
        }
        Expr::Primitive { args, .. } => {
            for arg in args {
                collect_uses(arg, names);
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_uses(condition, names);
            collect_uses(then_branch, names);
            collect_uses(else_branch, names);
        }
        Expr::Let { bindings, body } => {
            for (_, value) in bindings {
                collect_uses(value, names);
            }
            collect_uses(body, names);
        }
        Expr::Cons(head, tail) => {
            collect_uses(head, names);
            collect_uses(tail, names);
        }
        Expr::Car(value) | Expr::Cdr(value) | Expr::Null(value) => collect_uses(value, names),
        Expr::Number(_) | Expr::Bool(_) | Expr::Nil => {}
    }
}

/// Drop `let` bindings that nothing references any more.
///
/// Folding a branch can strand the bindings that only fed the dead arm; leaving
/// them would keep computing work the program no longer needs.
fn drop_unused_bindings(expr: Expr) -> Expr {
    match expr {
        Expr::Let { bindings, body } => {
            let body = drop_unused_bindings(*body);
            let mut kept = bindings
                .into_iter()
                .map(|(name, value)| (name, drop_unused_bindings(value)))
                .collect::<Vec<_>>();
            loop {
                let mut used = BTreeSet::new();
                collect_uses(&body, &mut used);
                for (_, value) in &kept {
                    collect_uses(value, &mut used);
                }
                let before = kept.len();
                kept = kept
                    .into_iter()
                    .filter(|(name, value)| used.contains(name) || !droppable(value))
                    .collect();
                if kept.len() == before {
                    break;
                }
            }
            if kept.is_empty() {
                body
            } else {
                Expr::Let {
                    bindings: kept,
                    body: Box::new(body),
                }
            }
        }
        other => map_children(other, drop_unused_bindings),
    }
}

fn simplify(expr: Expr) -> Expr {
    match expr {
        Expr::Primitive { op, args } => {
            let args = args.into_iter().map(simplify).collect::<Vec<_>>();
            fold_primitive(op, &args).unwrap_or(Expr::Primitive { op, args })
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let condition = simplify(*condition);
            let then_branch = simplify(*then_branch);
            let else_branch = simplify(*else_branch);
            match condition {
                Expr::Bool(true) => then_branch,
                Expr::Bool(false) => else_branch,
                // Bend's `if` is a switch: 0 is false and every other number
                // -- including a negative I24 -- is true.
                Expr::Number(number) if number == 0 => else_branch,
                Expr::Number(_) => then_branch,
                condition => Expr::If {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
            }
        }
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .into_iter()
                .map(|(name, value)| (name, simplify(value)))
                .collect(),
            body: Box::new(simplify(*body)),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args.into_iter().map(simplify).collect(),
        },
        Expr::Cons(head, tail) => Expr::Cons(Box::new(simplify(*head)), Box::new(simplify(*tail))),
        Expr::Car(value) => Expr::Car(Box::new(simplify(*value))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(simplify(*value))),
        Expr::Null(value) => Expr::Null(Box::new(simplify(*value))),
        atom => atom,
    }
}

fn fold_primitive(op: Primitive, args: &[Expr]) -> Option<Expr> {
    let [Expr::Number(left), Expr::Number(right)] = args else {
        return None;
    };
    let number = match op {
        Primitive::Add => left.checked_add(*right)?,
        Primitive::Sub => left.checked_sub(*right)?,
        Primitive::Mul => left.checked_mul(*right)?,
        Primitive::Div if *right != 0 => left.checked_div(*right)?,
        Primitive::Div => return None,
        Primitive::Eq => return Some(Expr::Bool(left == right)),
        Primitive::Lt => return Some(Expr::Bool(left < right)),
        Primitive::Gt => return Some(Expr::Bool(left > right)),
        Primitive::Le => return Some(Expr::Bool(left <= right)),
        Primitive::Ge => return Some(Expr::Bool(left >= right)),
    };
    (-8_388_608..=8_388_607)
        .contains(&number)
        .then_some(Expr::Number(number))
}

/// Introduce `let` bindings for repeated, total numeric expression trees.
/// Division and calls are excluded so moving the calculation ahead of a branch
/// cannot introduce a new error or change termination.
fn share_common(expr: Expr, fresh: &mut Fresh) -> Expr {
    let expr = match expr {
        Expr::Primitive { op, args } => Expr::Primitive {
            op,
            args: args
                .into_iter()
                .map(|arg| share_common(arg, fresh))
                .collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args
                .into_iter()
                .map(|arg| share_common(arg, fresh))
                .collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(share_common(*condition, fresh)),
            then_branch: Box::new(share_common(*then_branch, fresh)),
            else_branch: Box::new(share_common(*else_branch, fresh)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .into_iter()
                .map(|(name, value)| (name, share_common(value, fresh)))
                .collect(),
            body: Box::new(share_common(*body, fresh)),
        },
        Expr::Cons(head, tail) => Expr::Cons(
            Box::new(share_common(*head, fresh)),
            Box::new(share_common(*tail, fresh)),
        ),
        Expr::Car(value) => Expr::Car(Box::new(share_common(*value, fresh))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(share_common(*value, fresh))),
        Expr::Null(value) => Expr::Null(Box::new(share_common(*value, fresh))),
        atom => atom,
    };

    let mut current = expr;
    let mut bindings = Vec::new();
    loop {
        let mut candidates = Vec::new();
        collect_numeric_trees(&current, &mut candidates);
        let Some(candidate) = best_candidate(candidates) else {
            break;
        };
        let local = fresh.local();
        current = replace_expr(&current, &candidate, &Expr::Var(local.clone()));
        bindings.push((local, candidate));
    }
    if bindings.is_empty() {
        current
    } else {
        Expr::Let {
            bindings,
            body: Box::new(current),
        }
    }
}

fn collect_numeric_trees(expr: &Expr, candidates: &mut Vec<Expr>) {
    if total_numeric_tree(expr) && numeric_cost(expr) > 0 {
        candidates.push(expr.clone());
    }
    match expr {
        Expr::Primitive { args, .. } | Expr::Call { args, .. } => {
            for arg in args {
                collect_numeric_trees(arg, candidates);
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_numeric_trees(condition, candidates);
            collect_numeric_trees(then_branch, candidates);
            collect_numeric_trees(else_branch, candidates);
        }
        Expr::Let { bindings, body } => {
            for (_, value) in bindings {
                collect_numeric_trees(value, candidates);
            }
            collect_numeric_trees(body, candidates);
        }
        Expr::Cons(head, tail) => {
            collect_numeric_trees(head, candidates);
            collect_numeric_trees(tail, candidates);
        }
        Expr::Car(value) | Expr::Cdr(value) | Expr::Null(value) => {
            collect_numeric_trees(value, candidates)
        }
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => {}
    }
}

fn total_numeric_tree(expr: &Expr) -> bool {
    match expr {
        Expr::Number(_) | Expr::Var(_) => true,
        Expr::Primitive {
            op: Primitive::Div, ..
        } => false,
        Expr::Primitive { args, .. } => args.iter().all(total_numeric_tree),
        _ => false,
    }
}

fn numeric_cost(expr: &Expr) -> usize {
    match expr {
        Expr::Primitive { args, .. } => 1 + args.iter().map(numeric_cost).sum::<usize>(),
        _ => 0,
    }
}

fn best_candidate(candidates: Vec<Expr>) -> Option<Expr> {
    let mut counts = Vec::<(Expr, usize)>::new();
    for candidate in candidates {
        if let Some((_, count)) = counts.iter_mut().find(|(known, _)| *known == candidate) {
            *count += 1;
        } else {
            counts.push((candidate, 1));
        }
    }
    counts
        .into_iter()
        .filter(|(candidate, count)| *count >= 2 && numeric_cost(candidate) > 0)
        .max_by_key(|(candidate, count)| {
            (
                numeric_cost(candidate) * (count - 1),
                numeric_cost(candidate),
            )
        })
        .map(|(candidate, _)| candidate)
}

fn replace_expr(expr: &Expr, needle: &Expr, replacement: &Expr) -> Expr {
    if expr == needle {
        return replacement.clone();
    }
    match expr {
        Expr::Primitive { op, args } => Expr::Primitive {
            op: *op,
            args: args
                .iter()
                .map(|arg| replace_expr(arg, needle, replacement))
                .collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee: callee.clone(),
            args: args
                .iter()
                .map(|arg| replace_expr(arg, needle, replacement))
                .collect(),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(replace_expr(condition, needle, replacement)),
            then_branch: Box::new(replace_expr(then_branch, needle, replacement)),
            else_branch: Box::new(replace_expr(else_branch, needle, replacement)),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings
                .iter()
                .map(|(name, value)| (name.clone(), replace_expr(value, needle, replacement)))
                .collect(),
            body: Box::new(replace_expr(body, needle, replacement)),
        },
        Expr::Cons(head, tail) => Expr::Cons(
            Box::new(replace_expr(head, needle, replacement)),
            Box::new(replace_expr(tail, needle, replacement)),
        ),
        Expr::Car(value) => Expr::Car(Box::new(replace_expr(value, needle, replacement))),
        Expr::Cdr(value) => Expr::Cdr(Box::new(replace_expr(value, needle, replacement))),
        Expr::Null(value) => Expr::Null(Box::new(replace_expr(value, needle, replacement))),
        atom => atom.clone(),
    }
}

#[derive(Default)]
struct Fresh {
    next: usize,
    used: BTreeSet<String>,
}

impl Fresh {
    fn from_program(program: &Program) -> Self {
        let mut used = BTreeSet::new();
        for definition in &program.definitions {
            used.insert(definition.name.clone());
            used.extend(definition.params.iter().cloned());
            collect_names(&definition.body, &mut used);
        }
        if let Some(body) = &program.body {
            collect_names(body, &mut used);
        }
        Self { next: 0, used }
    }

    fn local(&mut self) -> String {
        loop {
            let candidate = format!("schemeOpt{}", self.next);
            self.next += 1;
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

fn collect_names(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(name) => {
            names.insert(name.clone());
        }
        Expr::Call { callee, args } => {
            names.insert(callee.clone());
            for arg in args {
                collect_names(arg, names);
            }
        }
        Expr::Primitive { args, .. } => {
            for arg in args {
                collect_names(arg, names);
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_names(condition, names);
            collect_names(then_branch, names);
            collect_names(else_branch, names);
        }
        Expr::Let { bindings, body } => {
            for (name, value) in bindings {
                names.insert(name.clone());
                collect_names(value, names);
            }
            collect_names(body, names);
        }
        Expr::Cons(head, tail) => {
            collect_names(head, names);
            collect_names(tail, names);
        }
        Expr::Car(value) | Expr::Cdr(value) | Expr::Null(value) => collect_names(value, names),
        Expr::Number(_) | Expr::Bool(_) | Expr::Nil => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_a_repeated_square() {
        let mut fresh = Fresh::default();
        let square = Expr::Primitive {
            op: Primitive::Mul,
            args: vec![Expr::Var("x".into()), Expr::Var("x".into())],
        };
        let optimized = share_common(
            Expr::Primitive {
                op: Primitive::Add,
                args: vec![square.clone(), square],
            },
            &mut fresh,
        );
        assert!(matches!(optimized, Expr::Let { .. }));
    }

    #[test]
    fn propagates_literals_so_constant_branches_fold() {
        // Mirrors what the inliner produces: `mode` becomes a literal binding.
        let body = Expr::Let {
            bindings: vec![
                ("x".into(), Expr::Var("arg".into())),
                ("mode".into(), Expr::Number(1)),
            ],
            body: Box::new(Expr::If {
                condition: Box::new(Expr::Var("mode".into())),
                then_branch: Box::new(Expr::Primitive {
                    op: Primitive::Add,
                    args: vec![Expr::Var("x".into()), Expr::Number(1)],
                }),
                else_branch: Box::new(Expr::Primitive {
                    op: Primitive::Mul,
                    args: vec![Expr::Var("x".into()), Expr::Number(10)],
                }),
            }),
        };
        let optimized = settle(body, true);
        assert!(!format!("{optimized:?}").contains("Mul"));
        assert!(!format!("{optimized:?}").contains("mode"));
    }

    #[test]
    fn folds_if_on_a_numeric_condition() {
        let body = Expr::If {
            condition: Box::new(Expr::Number(-5)),
            then_branch: Box::new(Expr::Number(1)),
            else_branch: Box::new(Expr::Number(2)),
        };
        assert_eq!(settle(body.clone(), true), Expr::Number(1));
        let zero = Expr::If {
            condition: Box::new(Expr::Number(0)),
            then_branch: Box::new(Expr::Number(1)),
            else_branch: Box::new(Expr::Number(2)),
        };
        assert_eq!(settle(zero, true), Expr::Number(2));
    }

    #[test]
    fn drops_unused_bindings_but_keeps_calls_and_division() {
        // Unused and cheap: dropped.
        let cheap = Expr::Let {
            bindings: vec![("dead".into(), Expr::Number(7))],
            body: Box::new(Expr::Number(1)),
        };
        assert_eq!(drop_unused_bindings(cheap), Expr::Number(1));
        // Unused but calls a function: kept, because dropping could change
        // termination.
        let calling = Expr::Let {
            bindings: vec![(
                "dead".into(),
                Expr::Call {
                    callee: "f".into(),
                    args: vec![],
                },
            )],
            body: Box::new(Expr::Number(1)),
        };
        assert!(matches!(drop_unused_bindings(calling), Expr::Let { .. }));
        // Unused but divides: kept, because dropping could delete an error.
        let dividing = Expr::Let {
            bindings: vec![(
                "dead".into(),
                Expr::Primitive {
                    op: Primitive::Div,
                    args: vec![Expr::Number(1), Expr::Number(0)],
                },
            )],
            body: Box::new(Expr::Number(1)),
        };
        assert!(matches!(drop_unused_bindings(dividing), Expr::Let { .. }));
    }

    #[test]
    fn does_not_hoist_division() {
        let mut fresh = Fresh::default();
        let division = Expr::Primitive {
            op: Primitive::Div,
            args: vec![Expr::Var("x".into()), Expr::Var("y".into())],
        };
        let optimized = share_common(
            Expr::Primitive {
                op: Primitive::Add,
                args: vec![division.clone(), division],
            },
            &mut fresh,
        );
        assert!(!matches!(optimized, Expr::Let { .. }));
    }
}
