use crate::{CompileError, sexpr::SExpr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub definitions: Vec<Definition>,
    /// The optional final top-level expression becomes generated Bend `main`.
    pub body: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Number(i64),
    Bool(bool),
    Var(String),
    Lambda {
        params: Vec<String>,
        body: Box<Expr>,
    },
    Apply {
        function: Box<Expr>,
        args: Vec<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Let {
        bindings: Vec<(String, Expr)>,
        body: Box<Expr>,
    },
    Cons(Box<Expr>, Box<Expr>),
    Car(Box<Expr>),
    Cdr(Box<Expr>),
    Null(Box<Expr>),
    Nil,
}

const FORBIDDEN: &[&str] = &[
    "set!",
    "set-car!",
    "set-cdr!",
    "call/cc",
    "eq?",
    "define-syntax",
    "syntax-rules",
    "with-exception-handler",
    "raise",
    "display",
    "newline",
    "read",
];

pub fn parse_program(forms: Vec<SExpr>) -> Result<Program, CompileError> {
    let mut definitions = Vec::new();
    let mut body = None;
    for form in forms {
        if is_define(&form) {
            if body.is_some() {
                return Err(CompileError(
                    "a top-level define cannot follow the program expression".into(),
                ));
            }
            definitions.push(parse_define(form)?);
        } else if body.replace(parse_expr(form)?).is_some() {
            return Err(CompileError(
                "only one final top-level expression is supported".into(),
            ));
        }
    }
    Ok(Program { definitions, body })
}

fn is_define(form: &SExpr) -> bool {
    matches!(form, SExpr::List(items) if matches!(items.first(), Some(SExpr::Symbol(s)) if s == "define"))
}

fn parse_define(form: SExpr) -> Result<Definition, CompileError> {
    let SExpr::List(items) = form else {
        unreachable!()
    };
    if items.len() != 3 {
        return Err(CompileError(
            "define expects a name and exactly one expression".into(),
        ));
    }
    match (&items[1], &items[2]) {
        (SExpr::Symbol(name), value) => Ok(Definition {
            name: valid_name(name)?,
            value: parse_expr(value.clone())?,
        }),
        (SExpr::List(signature), body) => {
            let Some(SExpr::Symbol(name)) = signature.first() else {
                return Err(CompileError("function define needs a function name".into()));
            };
            let params = signature[1..]
                .iter()
                .map(symbol)
                .collect::<Result<Vec<_>, _>>()?;
            ensure_distinct(&params, "lambda parameters")?;
            Ok(Definition {
                name: valid_name(name)?,
                value: Expr::Lambda {
                    params,
                    body: Box::new(parse_expr(body.clone())?),
                },
            })
        }
        _ => Err(CompileError(
            "define needs an identifier or function signature".into(),
        )),
    }
}

fn parse_expr(form: SExpr) -> Result<Expr, CompileError> {
    match form {
        SExpr::Number(n) => Ok(Expr::Number(n)),
        SExpr::Bool(b) => Ok(Expr::Bool(b)),
        SExpr::Symbol(s) => {
            reject_forbidden(&s)?;
            Ok(Expr::Var(s))
        }
        SExpr::List(items) if items.is_empty() => Ok(Expr::Nil),
        SExpr::List(items) => {
            let head = items.first().and_then(|x| match x {
                SExpr::Symbol(s) => Some(s.as_str()),
                _ => None,
            });
            match head {
                Some("lambda") => parse_lambda(&items),
                Some("if") => parse_if(&items),
                Some("let") => parse_let(&items),
                Some("cons") => unary_or_binary(&items, "cons", |a, b| {
                    Ok(Expr::Cons(Box::new(a), Box::new(b)))
                }),
                Some("car") => unary(&items, "car", |x| Ok(Expr::Car(Box::new(x)))),
                Some("cdr") => unary(&items, "cdr", |x| Ok(Expr::Cdr(Box::new(x)))),
                Some("null?") => unary(&items, "null?", |x| Ok(Expr::Null(Box::new(x)))),
                Some(word) if FORBIDDEN.contains(&word) => Err(CompileError(format!(
                    "`{word}` is intentionally outside scheme-bend"
                ))),
                _ => {
                    let mut it = items.into_iter();
                    let function = Box::new(parse_expr(it.next().unwrap())?);
                    let args = it.map(parse_expr).collect::<Result<Vec<_>, _>>()?;
                    Ok(Expr::Apply { function, args })
                }
            }
        }
    }
}

fn parse_lambda(items: &[SExpr]) -> Result<Expr, CompileError> {
    if items.len() != 3 {
        return Err(CompileError(
            "lambda expects parameters and exactly one body expression".into(),
        ));
    }
    let SExpr::List(params) = &items[1] else {
        return Err(CompileError("lambda parameters must be a list".into()));
    };
    let params = params.iter().map(symbol).collect::<Result<Vec<_>, _>>()?;
    ensure_distinct(&params, "lambda parameters")?;
    Ok(Expr::Lambda {
        params,
        body: Box::new(parse_expr(items[2].clone())?),
    })
}

fn parse_if(items: &[SExpr]) -> Result<Expr, CompileError> {
    if items.len() != 4 {
        return Err(CompileError(
            "if expects condition, then, and else expressions".into(),
        ));
    }
    Ok(Expr::If {
        condition: Box::new(parse_expr(items[1].clone())?),
        then_branch: Box::new(parse_expr(items[2].clone())?),
        else_branch: Box::new(parse_expr(items[3].clone())?),
    })
}

fn parse_let(items: &[SExpr]) -> Result<Expr, CompileError> {
    if items.len() != 3 {
        return Err(CompileError(
            "let expects bindings and exactly one body expression".into(),
        ));
    }
    let SExpr::List(bindings) = &items[1] else {
        return Err(CompileError("let bindings must be a list".into()));
    };
    let mut parsed = Vec::new();
    for binding in bindings {
        let SExpr::List(pair) = binding else {
            return Err(CompileError(
                "each let binding must be (name expression)".into(),
            ));
        };
        if pair.len() != 2 {
            return Err(CompileError(
                "each let binding must be (name expression)".into(),
            ));
        }
        parsed.push((symbol(&pair[0])?, parse_expr(pair[1].clone())?));
    }
    ensure_distinct(
        &parsed
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        "let bindings",
    )?;
    Ok(Expr::Let {
        bindings: parsed,
        body: Box::new(parse_expr(items[2].clone())?),
    })
}

fn unary<F>(items: &[SExpr], name: &str, f: F) -> Result<Expr, CompileError>
where
    F: FnOnce(Expr) -> Result<Expr, CompileError>,
{
    if items.len() != 2 {
        return Err(CompileError(format!("{name} expects one argument")));
    }
    f(parse_expr(items[1].clone())?)
}
fn unary_or_binary<F>(items: &[SExpr], name: &str, f: F) -> Result<Expr, CompileError>
where
    F: FnOnce(Expr, Expr) -> Result<Expr, CompileError>,
{
    if items.len() != 3 {
        return Err(CompileError(format!("{name} expects two arguments")));
    }
    f(parse_expr(items[1].clone())?, parse_expr(items[2].clone())?)
}
fn symbol(expr: &SExpr) -> Result<String, CompileError> {
    let SExpr::Symbol(s) = expr else {
        return Err(CompileError("expected an identifier".into()));
    };
    valid_name(s)
}
fn valid_name(name: &str) -> Result<String, CompileError> {
    reject_forbidden(name)?;
    if name.is_empty() {
        Err(CompileError("empty identifier".into()))
    } else {
        Ok(name.to_owned())
    }
}
fn reject_forbidden(name: &str) -> Result<(), CompileError> {
    if FORBIDDEN.contains(&name) {
        Err(CompileError(format!(
            "`{name}` is intentionally outside scheme-bend"
        )))
    } else {
        Ok(())
    }
}
fn ensure_distinct(names: &[String], description: &str) -> Result<(), CompileError> {
    for (i, name) in names.iter().enumerate() {
        if names[..i].contains(name) {
            return Err(CompileError(format!("duplicate {description}: `{name}`")));
        }
    }
    Ok(())
}
