//! The small, normalized functional core consumed by the Bend backend.
use crate::{CompileError, ast};
use std::collections::{BTreeMap, BTreeSet};

const I24_MIN: i64 = -8_388_608;
const I24_MAX: i64 = 8_388_607;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub definitions: Vec<Definition>,
    pub body: Option<Expr>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expr,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Number(i64),
    Bool(bool),
    Var(String),
    Nil,
    Call {
        callee: String,
        args: Vec<Expr>,
    },
    Primitive {
        op: Primitive,
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
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Lt,
    Gt,
    Le,
    Ge,
}

pub fn lower(program: ast::Program) -> Result<Program, CompileError> {
    let mut names = BTreeSet::new();
    for definition in &program.definitions {
        if !names.insert(definition.name.clone()) {
            return Err(CompileError(format!(
                "duplicate top-level definition `{}`",
                definition.name
            )));
        }
    }
    let mut lowerer = Lowerer::default();
    let definitions = program
        .definitions
        .into_iter()
        .map(|definition| {
            let (params, body) = match definition.value {
                ast::Expr::Lambda { params, body } => (params, *body),
                value => (Vec::new(), value),
            };
            let (params, scope) = lowerer.bind_parameters(params, &BTreeMap::new());
            Ok(Definition {
                name: definition.name,
                params,
                body: lowerer.expr(body, &scope)?,
            })
        })
        .collect::<Result<Vec<_>, CompileError>>()?;
    Ok(Program {
        definitions,
        body: program
            .body
            .map(|body| lowerer.expr(body, &BTreeMap::new()))
            .transpose()?,
    })
}

#[derive(Default)]
struct Lowerer {
    next_local: usize,
}

impl Lowerer {
    fn fresh(&mut self, name: &str) -> String {
        let local = format!("schemeLocal{}_{}", self.next_local, name);
        self.next_local += 1;
        local
    }
    fn bind_parameters(
        &mut self,
        params: Vec<String>,
        outer: &BTreeMap<String, String>,
    ) -> (Vec<String>, BTreeMap<String, String>) {
        let mut scope = outer.clone();
        let mut lowered = Vec::new();
        for param in params {
            let local = self.fresh(&param);
            scope.insert(param, local.clone());
            lowered.push(local);
        }
        (lowered, scope)
    }
    fn expr(
        &mut self,
        expr: ast::Expr,
        scope: &BTreeMap<String, String>,
    ) -> Result<Expr, CompileError> {
        match expr {
            ast::Expr::Number(n) if !(I24_MIN..=I24_MAX).contains(&n) => Err(CompileError(format!("number `{n}` is outside Bend's signed 24-bit range ({I24_MIN}..={I24_MAX})"))),
            ast::Expr::Number(n) => Ok(Expr::Number(n)),
            ast::Expr::Bool(b) => Ok(Expr::Bool(b)),
            ast::Expr::Var(v) => Ok(Expr::Var(scope.get(&v).cloned().unwrap_or(v))),
            ast::Expr::Nil => Ok(Expr::Nil),
            ast::Expr::If { condition, then_branch, else_branch } => Ok(Expr::If { condition: Box::new(self.expr(*condition, scope)?), then_branch: Box::new(self.expr(*then_branch, scope)?), else_branch: Box::new(self.expr(*else_branch, scope)?) }),
            ast::Expr::Let { bindings, body } => {
                let values = bindings.into_iter().map(|(name, value)| Ok((name, self.expr(value, scope)?))).collect::<Result<Vec<_>, CompileError>>()?;
                let mut body_scope = scope.clone();
                let bindings = values.into_iter().map(|(name, value)| { let local = self.fresh(&name); body_scope.insert(name, local.clone()); (local, value) }).collect();
                Ok(Expr::Let { bindings, body: Box::new(self.expr(*body, &body_scope)?) })
            }
            ast::Expr::Cons(a, b) => Ok(Expr::Cons(Box::new(self.expr(*a, scope)?), Box::new(self.expr(*b, scope)?))),
            ast::Expr::Car(value) => Ok(Expr::Car(Box::new(self.expr(*value, scope)?))),
            ast::Expr::Cdr(value) => Ok(Expr::Cdr(Box::new(self.expr(*value, scope)?))),
            ast::Expr::Null(value) => Ok(Expr::Null(Box::new(self.expr(*value, scope)?))),
            ast::Expr::Lambda { .. } => Err(CompileError("a lambda may only appear as a top-level definition or the immediate callee of an application; first-class closures are not in v0".into())),
            ast::Expr::Apply { function, args } => {
                let args = args.into_iter().map(|arg| self.expr(arg, scope)).collect::<Result<Vec<_>, _>>()?;
                match *function {
                    ast::Expr::Var(name) if scope.contains_key(&name) => Err(CompileError("only named functions and immediately-invoked lambdas can be called; first-class functions are not in v0".into())),
                    ast::Expr::Var(name) => match primitive(&name) { Some(op) if args.len() == 2 => Ok(Expr::Primitive { op, args }), Some(_) => Err(CompileError(format!("primitive `{name}` expects exactly two arguments"))), None => Ok(Expr::Call { callee: name, args }) },
                    ast::Expr::Lambda { params, body } => {
                        if params.len() != args.len() { return Err(CompileError(format!("lambda expects {} argument(s), got {}", params.len(), args.len()))); }
                        let (locals, lambda_scope) = self.bind_parameters(params, scope);
                        Ok(Expr::Let { bindings: locals.into_iter().zip(args).collect(), body: Box::new(self.expr(*body, &lambda_scope)?) })
                    }
                    _ => Err(CompileError("only named functions and immediately-invoked lambdas can be called; first-class functions are not in v0".into())),
                }
            }
        }
    }
}

fn primitive(name: &str) -> Option<Primitive> {
    Some(match name {
        "+" => Primitive::Add,
        "-" => Primitive::Sub,
        "*" => Primitive::Mul,
        "/" => Primitive::Div,
        "=" => Primitive::Eq,
        "<" => Primitive::Lt,
        ">" => Primitive::Gt,
        "<=" => Primitive::Le,
        ">=" => Primitive::Ge,
        _ => return None,
    })
}
