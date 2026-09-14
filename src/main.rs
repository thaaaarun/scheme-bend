use scheme_bend::optimize::Options;
use scheme_bend::shape::Shape;
use std::{env, fs, process::ExitCode};

fn main() -> ExitCode {
    let mut options = Options::default();
    let mut path = None;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--no-opt" => {
                options = Options {
                    inline_helpers: false,
                    common_subexpressions: false,
                    tail_unroll: 1,
                    constant_propagation: false,
                    // Shape is an independent axis; --no-opt leaves it alone.
                    shape: options.shape,
                    peval: options.peval,
                };
            }
            "--tail-unroll" => {
                let Some(value) = args.next() else {
                    return usage();
                };
                match value.parse::<usize>() {
                    Ok(value) if (1..=8).contains(&value) => options.tail_unroll = value,
                    _ => {
                        eprintln!("scheme-bend: --tail-unroll expects an integer from 1 to 8");
                        return ExitCode::from(2);
                    }
                }
            }
            "--shape" => {
                let Some(value) = args.next() else {
                    return usage();
                };
                match parse_shape(&value) {
                    Ok(shape) => options.shape = shape,
                    Err(()) => return usage(),
                }
            }
            _ if argument.starts_with("--peval=") => {
                match argument["--peval=".len()..].parse::<u64>() {
                    Ok(fuel) => options.peval = fuel,
                    Err(_) => return usage(),
                }
            }
            _ if argument.starts_with("--shape=") => {
                match parse_shape(&argument["--shape=".len()..]) {
                    Ok(shape) => options.shape = shape,
                    Err(()) => return usage(),
                }
            }
            "--peval" => {
                let Some(value) = args.next() else {
                    return usage();
                };
                match value.parse::<u64>() {
                    Ok(fuel) => options.peval = fuel,
                    Err(_) => {
                        eprintln!("scheme-bend: --peval expects a non-negative integer");
                        return ExitCode::from(2);
                    }
                }
            }
            "--no-cse" => options.common_subexpressions = false,
            "--no-const-prop" => options.constant_propagation = false,
            "--help" | "-h" => return usage(),
            _ if argument.starts_with('-') => {
                eprintln!("scheme-bend: unknown option `{argument}`");
                return usage();
            }
            _ => {
                if path.is_some() {
                    return usage();
                }
                path = Some(argument);
            }
        }
    }
    let Some(path) = path else { return usage() };
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("scheme-bend: cannot read {path}: {error}");
            return ExitCode::from(1);
        }
    };
    match scheme_bend::compile_with_options(&source, options) {
        Ok(bend) => {
            print!("{bend}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("scheme-bend: {error}");
            ExitCode::from(1)
        }
    }
}

fn parse_shape(value: &str) -> Result<Shape, ()> {
    Shape::parse(value).ok_or_else(|| {
        eprintln!("scheme-bend: --shape expects asis, balanced, or chunked:N");
    })
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: scheme-bend [--no-opt] [--no-cse] [--no-const-prop] [--tail-unroll 1..8] \
         [--shape asis|balanced|chunked:N] [--peval N] <input.scm>"
    );
    ExitCode::from(2)
}
