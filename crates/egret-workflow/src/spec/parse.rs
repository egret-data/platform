use serde::Serialize;

use crate::models::Flow;

/// A YAML syntax or shape error, located.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParseError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Parse a flow's YAML source into the model.
///
/// `serde_norway::Error::location()` is the reason that crate was chosen over a
/// bare YAML parser: an editor needs to put the squiggle somewhere.
pub fn parse(source: &str) -> Result<Flow, ParseError> {
    serde_norway::from_str::<Flow>(source).map_err(|e| {
        let loc = e.location();
        ParseError {
            message: e.to_string(),
            line: loc.as_ref().map(|l| l.line()),
            column: loc.as_ref().map(|l| l.column()),
        }
    })
}
