//! The interpreter module contains all of the different parse steps used to compile a `.shoumei` module.
//!
//! The compilation passes are (in order of execution):
//! - `lexer`
//! - `indent`
//! - `brackets`
//! - `parser`
//! - `types` (after this step, `type_resolve` can be used)
//! - `index`
//!
//!
//! As a general rule, each compilation pass may only use types declared in previous passes.
//!
//! Types may have certain suffixes to declare what information they contain and where they should be used:
//! - `P`: just been Parsed, no extra information has been deduced.
//!   No type has been deduced, and no effort has been made to ensure syntactic correctness
//!   past the (lenient) parser.
//! - `C`: an intermediate data Cache, used when we're still in the middle of computing the index.
//!   After the index has been computed, we should not need to use `P` or `C` data,
//!   only `I` data should be required.
//! - `I`: an Index entry for the item.
//! - (no suffix): types have been deduced and references have been resolved.
//!
//! Using type name suffixes as a form of type state helps to ensure that compiler phases can never leak bad
//! information between each other, ensuring (for example) that after a type check phase, all expressions
//! actually have a type.

use std::{
    fmt::Display,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use crate::{Diagnostic, DiagnosticResult, ErrorMessage, Severity};

pub mod brackets;
pub mod indent;
pub mod index;
pub mod lexer;
pub mod parser;
pub mod type_resolve;
pub mod types;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    /// A 0-indexed line number.
    pub line: u32,
    /// A 0-indexed column number.
    pub col: u32,
}

impl Location {
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Range {
    /// The start of this range of characters, inclusive.
    pub start: Location,
    /// The end of this range of characters, exclusive.
    pub end: Location,
}

impl From<Location> for Range {
    fn from(location: Location) -> Self {
        Self {
            start: location,
            end: Location {
                line: location.line,
                col: location.col + 1,
            },
        }
    }
}

impl Range {
    pub fn union(self, other: Range) -> Range {
        Range {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// A list of path segments. These cannot contain forward or backward slashes, or colons.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModulePath(pub Vec<String>);

impl<'a> From<&'a ModulePath> for PathBuf {
    fn from(path: &'a ModulePath) -> Self {
        path.0.iter().collect()
    }
}

impl Display for ModulePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, item) in self.0.iter().enumerate() {
            if i != 0 {
                write!(f, "/")?;
            }
            write!(f, "{}", item)?;
        }
        Ok(())
    }
}

/// A fully qualified name referring to a top-level item declared in a `.shoumei` module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedName {
    pub module_path: ModulePath,
    pub name: String,
    pub range: Range,
}

pub fn parse(module_path: &ModulePath) -> DiagnosticResult<parser::ModuleP> {
    // This chain of `bind`s is very similar to monadic `do` notation in Haskell.
    // file <- ...
    // lines <- ...
    let file = match File::open(PathBuf::from(module_path)) {
        Ok(file) => file.into(),
        Err(_) => {
            let message = ErrorMessage::new(
                String::from("cannot open file"),
                Severity::Error,
                Diagnostic::in_file(module_path.clone()),
            );
            DiagnosticResult::fail(message)
        }
    };

    let lines = file.bind(|file| {
        let mut lines = Vec::new();
        for (line, line_number) in BufReader::new(file).lines().zip(0..) {
            match line {
                Ok(line) => {
                    lines.push(line);
                }
                Err(_) => {
                    return DiagnosticResult::fail(ErrorMessage::new(
                        format!("file contained invalid UTF-8 on line {}", line_number + 1),
                        Severity::Error,
                        Diagnostic::in_file(module_path.clone()),
                    ));
                }
            }
        }
        DiagnosticResult::ok(lines)
    });

    // The use of `deny` means that any error in any compilation step will abort the compilation after the step is finished.

    lines
        .bind(|lines| lexer::lex(module_path, lines))
        .deny()
        .bind(|tokens| indent::process_indent(module_path, tokens))
        .deny()
        .bind(|token_block| brackets::process_brackets(module_path, token_block))
        .deny()
        .bind(|token_block| parser::parse(module_path, token_block))
        .deny()
        .bind(|module| {
            println!("{:#?}", module);
            let types = types::compute_types(module_path, &module);
            println!("{:#?}", types);
            let project_types = types.map(|types| {
                let mut project_types = types::ProjectTypesC::new();
                project_types.insert(module_path.clone(), types);
                project_types
            });
            project_types.map(|project_types| (project_types, module))
        })
        .deny()
        .bind(|(project_types, module)| {
            let index = index::index(module_path, &module, &project_types);
            println!("{:#?}", index);
            index.map(|index| (project_types, index, module))
        })
        .deny()
        .map(|(project_types, index, module)| module)
        .deny()
}
