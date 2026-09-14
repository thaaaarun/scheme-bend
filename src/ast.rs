use crate::{CompileError, sexpr::SExpr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub definitions: Vec<Definition>,
    pub graphs: Vec<GraphDefinition>,
    /// The optional final top-level expression becomes generated Bend `main`.
    pub body: Option<Expr>,
}

/// A statically declared weighted graph.
///
/// Node values are scalar sums of weighted predecessor values. `inputs` are
/// supplied as function arguments and `outputs` are returned as a list. The
/// graph is deliberately structural: zero-weight edges are not represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphDefinition {
    pub name: String,
    pub nodes: usize,
    pub inputs: Vec<usize>,
    pub outputs: Vec<usize>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub src: usize,
    pub dst: usize,
    pub weight: i64,
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
    let mut graphs = Vec::new();
    let mut body = None;
    for form in forms {
        if is_graph_define(&form) {
            if body.is_some() {
                return Err(CompileError(
                    "a top-level definition cannot follow the program expression".into(),
                ));
            }
            graphs.push(parse_graph_define(form)?);
        } else if is_define(&form) {
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
    Ok(Program {
        definitions,
        graphs,
        body,
    })
}

fn is_define(form: &SExpr) -> bool {
    matches!(form, SExpr::List(items) if matches!(items.first(), Some(SExpr::Symbol(s)) if s == "define"))
}

fn is_graph_define(form: &SExpr) -> bool {
    matches!(form, SExpr::List(items) if matches!(items.first(), Some(SExpr::Symbol(s)) if s == "define-graph"))
}

fn parse_graph_define(form: SExpr) -> Result<GraphDefinition, CompileError> {
    let SExpr::List(items) = form else {
        unreachable!()
    };
    if items.len() != 6 {
        return Err(CompileError(
            "define-graph expects a name, nodes, inputs, outputs, and edges".into(),
        ));
    }
    let SExpr::Symbol(name) = &items[1] else {
        return Err(CompileError("define-graph needs an identifier".into()));
    };
    let name = valid_name(name)?;
    let nodes = parse_graph_count(&items[2], "nodes")?;
    let inputs = parse_graph_id_list(&items[3], "inputs")?;
    let outputs = parse_graph_id_list(&items[4], "outputs")?;
    let edges = parse_graph_edges(&items[5])?;

    ensure_distinct_usize(&inputs, "graph inputs")?;
    ensure_distinct_usize(&outputs, "graph outputs")?;
    for &node in inputs.iter().chain(&outputs) {
        if node >= nodes {
            return Err(CompileError(format!(
                "graph node {node} is outside the declared node range 0..{nodes}"
            )));
        }
    }
    for edge in &edges {
        if edge.src >= nodes || edge.dst >= nodes {
            return Err(CompileError(format!(
                "graph edge ({}, {}) is outside the declared node range 0..{}",
                edge.src, edge.dst, nodes
            )));
        }
        if edge.weight == 0 {
            return Err(CompileError(
                "graph edges must have nonzero weights; omit zero edges".into(),
            ));
        }
    }
    for (i, edge) in edges.iter().enumerate() {
        if edges[..i]
            .iter()
            .any(|prior| prior.src == edge.src && prior.dst == edge.dst)
        {
            return Err(CompileError(format!(
                "duplicate graph edge ({}, {})",
                edge.src, edge.dst
            )));
        }
    }
    Ok(GraphDefinition {
        name,
        nodes,
        inputs,
        outputs,
        edges,
    })
}

fn parse_graph_count(form: &SExpr, label: &str) -> Result<usize, CompileError> {
    let SExpr::List(items) = form else {
        return Err(CompileError(format!(
            "graph {label} must be written as ({label} n)"
        )));
    };
    if items.len() != 2 || items[0] != SExpr::Symbol(label.into()) {
        return Err(CompileError(format!(
            "graph {label} must be written as ({label} n)"
        )));
    }
    graph_usize(&items[1], label)
}

fn parse_graph_id_list(form: &SExpr, label: &str) -> Result<Vec<usize>, CompileError> {
    let SExpr::List(items) = form else {
        return Err(CompileError(format!(
            "graph {label} must be a list of node ids"
        )));
    };
    if items.len() != 2 || items[0] != SExpr::Symbol(label.into()) {
        return Err(CompileError(format!(
            "graph {label} must be written as ({label} (...))"
        )));
    }
    let SExpr::List(ids) = &items[1] else {
        return Err(CompileError(format!(
            "graph {label} must be a list of node ids"
        )));
    };
    ids.iter().map(|id| graph_usize(id, label)).collect()
}

fn parse_graph_edges(form: &SExpr) -> Result<Vec<GraphEdge>, CompileError> {
    let SExpr::List(items) = form else {
        return Err(CompileError(
            "graph edges must be written as (edges (...))".into(),
        ));
    };
    if items.is_empty() || items[0] != SExpr::Symbol("edges".into()) {
        return Err(CompileError(
            "graph edges must be written as (edges (...))".into(),
        ));
    }
    let edge_forms = match items.get(1) {
        Some(SExpr::List(edges))
            if edges.is_empty() || matches!(edges.first(), Some(SExpr::List(_))) =>
        {
            edges.clone()
        }
        _ => items[1..].to_vec(),
    };
    edge_forms
        .into_iter()
        .map(|form| {
            let SExpr::List(edge) = form else {
                return Err(CompileError(
                    "each graph edge must be (source target weight)".into(),
                ));
            };
            if edge.len() != 3 {
                return Err(CompileError(
                    "each graph edge must be (source target weight)".into(),
                ));
            }
            Ok(GraphEdge {
                src: graph_usize(&edge[0], "edge source")?,
                dst: graph_usize(&edge[1], "edge target")?,
                weight: graph_number(&edge[2], "edge weight")?,
            })
        })
        .collect()
}

fn graph_usize(form: &SExpr, label: &str) -> Result<usize, CompileError> {
    let SExpr::Number(value) = form else {
        return Err(CompileError(format!(
            "graph {label} must be a nonnegative integer"
        )));
    };
    usize::try_from(*value)
        .map_err(|_| CompileError(format!("graph {label} must be a nonnegative integer")))
}

fn graph_number(form: &SExpr, label: &str) -> Result<i64, CompileError> {
    let SExpr::Number(value) = form else {
        return Err(CompileError(format!("graph {label} must be an integer")));
    };
    Ok(*value)
}

fn ensure_distinct_usize(values: &[usize], description: &str) -> Result<(), CompileError> {
    for (i, value) in values.iter().enumerate() {
        if values[..i].contains(value) {
            return Err(CompileError(format!("duplicate {description}: `{value}`")));
        }
    }
    Ok(())
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
