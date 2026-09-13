//! A deliberately small compiler pipeline: S-expressions -> Scheme AST -> IR -> Bend.

pub mod ast;
pub mod codegen;
pub mod ir;
pub mod optimize;
pub mod sexpr;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError(pub String);

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for CompileError {}

pub fn compile(source: &str) -> Result<String, CompileError> {
    compile_with_options(source, optimize::Options::default())
}

pub fn compile_with_options(
    source: &str,
    options: optimize::Options,
) -> Result<String, CompileError> {
    let forms = sexpr::parse(source)?;
    let program = ast::parse_program(forms)?;
    let ir = ir::lower(program)?;
    Ok(codegen::emit(&optimize::optimize(ir, options)))
}
