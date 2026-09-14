//! Fuel-bounded partial evaluation of a closed language.
//!
//! `main` takes no input, there is no I/O, and nothing else arrives from
//! outside, so every value in a program is a function of the program text. A
//! compiler can therefore *know* values an ordinary compiler cannot -- including
//! which entries of a matrix are zero. The only reason the rest of the pipeline
//! does not is that it never computes them.
//!
//! This pass computes them. Bodies are rewritten top-down: a call whose
//! arguments are all known is inlined with its parameters substituted, and
//! whatever folds is folded. Everything else is left as a residual, so partial
//! progress is kept rather than discarded.
//!
//! Fuel bounds the number of inlined calls. That bound is load-bearing in two
//! directions. Turned up, the pass will evaluate an entire program and emit a
//! constant -- correct, but it means a benchmark would be measuring this
//! interpreter instead of the backend. Turned down, it specialises only the
//! first few levels. `--peval N` is that budget and callers must choose it.
//!
//! Values are the target's: signed 24-bit numbers that wrap, immutable cons
//! cells, `#t`/`#f` and `()`. Arithmetic that would leave 24 bits is *not*
//! folded rather than wrapped, so a folded result can never disagree with the
//! same expression left to run.

use crate::ir::{Definition, Expr, Primitive, Program};
use std::collections::BTreeMap;

/// Ceiling on the residual's node count. Past this the rewrite is abandoned and
/// the original body kept, which bounds code growth.
const MAX_NODES: usize = 1 << 20;

/// A literal is a value the compiler knows outright: a number, a boolean, `()`,
/// or a cons chain built from literals. Literals are carried as `Expr`, so a
/// literal list needs no separate representation and no conversion.
fn is_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Number(_) | Expr::Bool(_) | Expr::Nil => true,
        Expr::Cons(head, tail) => is_literal(head) && is_literal(tail),
        _ => false,
    }
}

fn nodes(expr: &Expr) -> usize {
    match expr {
        Expr::Number(_) | Expr::Bool(_) | Expr::Var(_) | Expr::Nil => 1,
        Expr::Primitive { args, .. } | Expr::Call { args, .. } => {
            1 + args.iter().map(nodes).sum::<usize>()
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => 1 + nodes(condition) + nodes(then_branch) + nodes(else_branch),
        Expr::Let { bindings, body } => {
            1 + bindings.iter().map(|(_, v)| nodes(v)).sum::<usize>() + nodes(body)
        }
        Expr::Cons(a, b) => 1 + nodes(a) + nodes(b),
        Expr::Car(a) | Expr::Cdr(a) | Expr::Null(a) => 1 + nodes(a),
    }
}

type Env = BTreeMap<String, Expr>;

struct Pe<'a> {
    defs: &'a BTreeMap<String, Definition>,
    fuel: u64,
}

impl Pe<'_> {
    /// 24-bit wrapping is the target's, so a fold must stay in range or not
    /// happen at all.
    fn arithmetic(&self, op: Primitive, left: i64, right: i64) -> Option<i64> {
        let result = match op {
            Primitive::Add => left.checked_add(right)?,
            Primitive::Sub => left.checked_sub(right)?,
            Primitive::Mul => left.checked_mul(right)?,
            Primitive::Div if right != 0 => left.checked_div(right)?,
            Primitive::Div => return None,
            _ => return None,
        };
        (-8_388_608..=8_388_607).contains(&result).then_some(result)
    }

    /// Rewrite `expr` with `env` substituted, folding whatever is known.
    fn expr(&mut self, expr: &Expr, env: &Env) -> Expr {
        match expr {
            Expr::Number(_) | Expr::Bool(_) | Expr::Nil => expr.clone(),
            Expr::Var(name) => match env.get(name) {
                Some(value) => value.clone(),
                None => expr.clone(),
            },
            Expr::Primitive { op, args } => {
                let args = args.iter().map(|a| self.expr(a, env)).collect::<Vec<_>>();
                if let [Expr::Number(left), Expr::Number(right)] = &args[..] {
                    return match op {
                        Primitive::Add | Primitive::Sub | Primitive::Mul | Primitive::Div => {
                            match self.arithmetic(*op, *left, *right) {
                                Some(value) => Expr::Number(value),
                                None => Expr::Primitive { op: *op, args },
                            }
                        }
                        Primitive::Eq => Expr::Bool(left == right),
                        Primitive::Lt => Expr::Bool(left < right),
                        Primitive::Gt => Expr::Bool(left > right),
                        Primitive::Le => Expr::Bool(left <= right),
                        Primitive::Ge => Expr::Bool(left >= right),
                    };
                }
                Expr::Primitive { op: *op, args }
            }
            Expr::Cons(head, tail) => Expr::Cons(
                Box::new(self.expr(head, env)),
                Box::new(self.expr(tail, env)),
            ),
            // `car`/`cdr` on `()` use the sentinels the prelude documents, so
            // folding them matches what the emitted program would do.
            Expr::Car(value) => match self.expr(value, env) {
                Expr::Cons(head, _) => *head,
                Expr::Nil => Expr::Number(0),
                other => Expr::Car(Box::new(other)),
            },
            Expr::Cdr(value) => match self.expr(value, env) {
                Expr::Cons(_, tail) => *tail,
                Expr::Nil => Expr::Nil,
                other => Expr::Cdr(Box::new(other)),
            },
            Expr::Null(value) => match self.expr(value, env) {
                Expr::Cons(_, _) => Expr::Bool(false),
                Expr::Nil => Expr::Bool(true),
                other => Expr::Null(Box::new(other)),
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.expr(condition, env);
                match condition {
                    // Bend's `if` is a switch: 0 is false, everything else true.
                    Expr::Bool(taken) => {
                        let branch = if taken { then_branch } else { else_branch };
                        self.expr(branch, env)
                    }
                    Expr::Number(n) => {
                        let branch = if n == 0 { else_branch } else { then_branch };
                        self.expr(branch, env)
                    }
                    condition => Expr::If {
                        condition: Box::new(condition),
                        then_branch: Box::new(self.expr(then_branch, env)),
                        else_branch: Box::new(self.expr(else_branch, env)),
                    },
                }
            }
            Expr::Let { bindings, body } => {
                let mut inner = env.clone();
                let mut kept = Vec::with_capacity(bindings.len());
                for (name, value) in bindings {
                    let value = self.expr(value, &inner);
                    if is_literal(&value) {
                        inner.insert(name.clone(), value.clone());
                    }
                    kept.push((name.clone(), value));
                }
                let body = self.expr(body, &inner);
                // Literal bindings are already substituted into the body, so
                // they carry no evaluation and dropping them is free. This is
                // load-bearing: leaving one behind hides a known value inside
                // a `Let`, and the caller then cannot tell it is a literal.
                let kept = kept
                    .into_iter()
                    .filter(|(_, value)| !is_literal(value))
                    .collect::<Vec<_>>();
                if kept.is_empty() {
                    body
                } else {
                    Expr::Let {
                        bindings: kept,
                        body: Box::new(body),
                    }
                }
            }
            Expr::Call { callee, args } => {
                let args = args.iter().map(|a| self.expr(a, env)).collect::<Vec<_>>();
                if self.fuel == 0 || !args.iter().all(is_literal) {
                    return Expr::Call {
                        callee: callee.clone(),
                        args,
                    };
                }
                let Some(definition) = self.defs.get(callee) else {
                    return Expr::Call {
                        callee: callee.clone(),
                        args,
                    };
                };
                if definition.params.len() != args.len() {
                    return Expr::Call {
                        callee: callee.clone(),
                        args,
                    };
                }
                self.fuel -= 1;
                let inner = definition
                    .params
                    .iter()
                    .cloned()
                    .zip(args)
                    .collect::<Env>();
                self.expr(&definition.body, &inner)
            }
        }
    }
}

/// Partially evaluate every body with at most `fuel` inlined calls.
pub fn apply(program: Program, fuel: u64) -> Program {
    if fuel == 0 {
        return program;
    }
    let defs = program
        .definitions
        .iter()
        .map(|d| (d.name.clone(), d.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut program = program;
    for definition in &mut program.definitions {
        let original = definition.body.clone();
        let mut pe = Pe { defs: &defs, fuel };
        let rewritten = pe.expr(&original, &BTreeMap::new());
        // Abandon a rewrite that would blow up the output.
        if nodes(&rewritten) <= MAX_NODES {
            definition.body = rewritten;
        }
    }
    if let Some(body) = program.body.take() {
        let mut pe = Pe { defs: &defs, fuel };
        let rewritten = pe.expr(&body, &BTreeMap::new());
        program.body = Some(if nodes(&rewritten) <= MAX_NODES {
            rewritten
        } else {
            body
        });
    }
    program
}

#[cfg(test)]
mod tests {
    use crate::optimize::Options;
    use crate::compile_with_options;

    fn compile_with(source: &str, fuel: u64) -> String {
        compile_with_options(
            source,
            Options {
                peval: fuel,
                ..Options::default()
            },
        )
        .expect("compiles")
    }

    #[test]
    fn folds_a_closed_call_to_its_value() {
        // `(double 21)` is closed, so partial evaluation knows it outright.
        let source = "(define (double x) (+ x x))\n(define (once) (double 21))\n(once)";
        assert!(compile_with(source, 64).contains("return +42"));
    }

    #[test]
    fn sees_a_zero_that_was_computed_at_runtime() {
        // This is the whole point: `w` returns 0 for every `i` it is asked
        // about, and once the walk over the literal list is unfolded the
        // multiply is a literal `0 * ...`, which the identity rule removes.
        let source = "(define (w i) 0)\n\
             (define (dot i n acc)\n\
               (if (> i n) acc (dot (+ i 1) n (+ acc (* (w i) i)))))\n\
             (dot 1 4 0)";
        let naive = compile_with(source, 0);
        let folded = compile_with(source, 64);
        // Scope to `main`: the prelude's own `scheme_car` returns `+0` too.
        let main_of = |out: &str| out.split("def main()").nth(1).unwrap_or("").to_owned();
        assert!(main_of(&folded).contains("return +0"), "{folded}");
        assert!(!main_of(&naive).contains("return +0"), "{naive}");
    }

    #[test]
    fn fuel_bounds_how_far_it_goes() {
        let source = "(define (step n) (if (= n 0) 0 (+ 1 (step (- n 1)))))\n(step 32)";
        let shallow = compile_with(source, 4);
        let deep = compile_with(source, 4096);
        assert!(deep.contains("return +32"), "{deep}");
        assert!(!shallow.contains("return +32"), "{shallow}");
    }

    #[test]
    fn refuses_to_wrap_rather_than_disagreeing_with_the_runtime() {
        // 8,388,607 + 1 would wrap at runtime; folding it must not happen.
        let source = "(define (f) (+ 8388607 1))\n(f)";
        let folded = compile_with(source, 64);
        assert!(folded.contains('+'), "{folded}");
        assert!(!folded.contains("return -8388608"), "{folded}");
    }
}
