use crate::CompileError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SExpr {
    Number(i64),
    Bool(bool),
    Symbol(String),
    List(Vec<SExpr>),
}

pub fn parse(source: &str) -> Result<Vec<SExpr>, CompileError> {
    let tokens = tokenize(source)?;
    let mut cursor = 0;
    let mut forms = Vec::new();
    while cursor < tokens.len() {
        forms.push(parse_one(&tokens, &mut cursor)?);
    }
    Ok(forms)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    Atom(String),
}

fn tokenize(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut result = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            ';' => {
                while let Some(c) = chars.next() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '(' => result.push(Token::Open),
            ')' => result.push(Token::Close),
            '\'' | '`' | ',' | '"' => {
                return Err(CompileError(format!(
                    "unsupported reader syntax starting with `{ch}`"
                )));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut atom = String::from(c);
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '(' | ')' | ';') {
                        break;
                    }
                    if matches!(next, '\'' | '`' | ',' | '"') {
                        return Err(CompileError(format!(
                            "unsupported reader syntax starting with `{next}`"
                        )));
                    }
                    atom.push(next);
                    chars.next();
                }
                result.push(Token::Atom(atom));
            }
        }
    }
    Ok(result)
}

fn parse_one(tokens: &[Token], cursor: &mut usize) -> Result<SExpr, CompileError> {
    let token = tokens
        .get(*cursor)
        .ok_or_else(|| CompileError("unexpected end of input".into()))?;
    *cursor += 1;
    match token {
        Token::Open => {
            let mut items = Vec::new();
            loop {
                match tokens.get(*cursor) {
                    Some(Token::Close) => {
                        *cursor += 1;
                        return Ok(SExpr::List(items));
                    }
                    Some(_) => items.push(parse_one(tokens, cursor)?),
                    None => return Err(CompileError("unclosed `(`".into())),
                }
            }
        }
        Token::Close => Err(CompileError("unexpected `)`".into())),
        Token::Atom(atom) if atom == "#t" => Ok(SExpr::Bool(true)),
        Token::Atom(atom) if atom == "#f" => Ok(SExpr::Bool(false)),
        Token::Atom(atom) => match atom.parse::<i64>() {
            Ok(n) => Ok(SExpr::Number(n)),
            Err(_) => Ok(SExpr::Symbol(atom.clone())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_atoms_and_lists() {
        assert_eq!(
            parse("; hello\n(+ -12 #t)"),
            Ok(vec![SExpr::List(vec![
                SExpr::Symbol("+".into()),
                SExpr::Number(-12),
                SExpr::Bool(true),
            ])])
        );
    }
}
