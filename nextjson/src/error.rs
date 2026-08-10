//! Error model: JSON errors with precise position (line / column / offset).

use alloc::string::String;
use core::fmt;

/// Global result alias for `nextjson`.
pub type Result<T> = core::result::Result<T, Error>;

/// A JSON error carrying positional information.
///
/// Every parse error records the trigger position; byte-stream inputs also
/// carry a precise 1-based line and column.
#[derive(Debug, Clone)]
pub struct Error {
    kind: ErrorKind,
    line: Option<u32>,
    column: Option<u32>,
    offset: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum ErrorKind {
    Eof,
    Expected {
        what: &'static str,
        found: Option<u8>,
    },
    InvalidNumber,
    NumberOutOfRange,
    ControlCharInString,
    InvalidEscape(char),
    InvalidSurrogate,
    InvalidUtf8,
    RecursionLimitExceeded,
    UnknownField(String),
    MissingField(&'static str),
    UnknownVariant(String),
    InvalidType {
        expected: &'static str,
        found: &'static str,
    },
    InvalidLength {
        len: usize,
        expected: &'static str,
    },
    NonFiniteFloat,
    Custom(String),
}

impl Error {
    pub(crate) fn new(
        kind: ErrorKind,
        line: Option<u32>,
        column: Option<u32>,
        offset: usize,
    ) -> Self {
        Error {
            kind,
            line,
            column,
            offset,
        }
    }

    /// Build a custom error for user code.
    pub fn custom(msg: impl Into<String>) -> Self {
        Error::new(ErrorKind::Custom(msg.into()), None, None, 0)
    }

    /// IO error (only reachable under the `std` feature).
    #[cfg(feature = "std")]
    pub(crate) fn io(err: std::io::Error) -> Self {
        Error::custom(alloc::format!("io error: {err}"))
    }

    /// A required field is missing.
    pub fn missing_field(field: &'static str) -> Self {
        Error::new(ErrorKind::MissingField(field), None, None, 0)
    }

    /// An unknown field was encountered.
    pub fn unknown_field(field: String) -> Self {
        Error::new(ErrorKind::UnknownField(field), None, None, 0)
    }

    /// An unknown enum variant was encountered.
    pub fn unknown_variant(variant: String) -> Self {
        Error::new(ErrorKind::UnknownVariant(variant), None, None, 0)
    }

    /// A length mismatch.
    pub fn invalid_length(len: usize, expected: &'static str) -> Self {
        Error::new(ErrorKind::InvalidLength { len, expected }, None, None, 0)
    }

    /// A type mismatch.
    pub fn invalid_type(expected: &'static str, found: &'static str) -> Self {
        Error::new(ErrorKind::InvalidType { expected, found }, None, None, 0)
    }

    /// 1-based line (byte-stream inputs only).
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    /// 1-based column (byte-stream inputs only).
    pub fn column(&self) -> Option<u32> {
        self.column
    }

    /// Byte offset (byte-stream input) or token index (content replay).
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Whether this is a custom error.
    pub fn is_custom(&self) -> bool {
        matches!(self.kind, ErrorKind::Custom(_))
    }

    /// Coarse classification of the error.
    pub fn classification(&self) -> &'static str {
        match &self.kind {
            ErrorKind::Eof => "unexpected end of input",
            ErrorKind::Expected { .. } => "expected a specific token",
            ErrorKind::InvalidNumber => "invalid number",
            ErrorKind::NumberOutOfRange => "number out of range",
            ErrorKind::ControlCharInString => "unexpected control character in string",
            ErrorKind::InvalidEscape(_) => "invalid escape sequence",
            ErrorKind::InvalidSurrogate => "invalid surrogate pair",
            ErrorKind::InvalidUtf8 => "invalid utf-8",
            ErrorKind::RecursionLimitExceeded => "recursion limit exceeded",
            ErrorKind::UnknownField(_) => "unknown field",
            ErrorKind::MissingField(_) => "missing field",
            ErrorKind::UnknownVariant(_) => "unknown variant",
            ErrorKind::InvalidType { .. } => "invalid type",
            ErrorKind::InvalidLength { .. } => "invalid length",
            ErrorKind::NonFiniteFloat => "non-finite float",
            ErrorKind::Custom(_) => "custom error",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Eof => write!(f, "unexpected end of input"),
            ErrorKind::Expected { what, found } => match found {
                Some(b) => write!(
                    f,
                    "expected {what}, found byte 0x{b:02x} ('{}')",
                    *b as char
                ),
                None => write!(f, "expected {what}, found end of input"),
            },
            ErrorKind::InvalidNumber => write!(f, "invalid number"),
            ErrorKind::NumberOutOfRange => write!(f, "number out of range"),
            ErrorKind::ControlCharInString => {
                write!(
                    f,
                    "control character (\\u0000-\\u001F) must be escaped in JSON string"
                )
            }
            ErrorKind::InvalidEscape(c) => write!(f, "invalid escape sequence '\\{c}'"),
            ErrorKind::InvalidSurrogate => {
                write!(f, "lone or invalid surrogate pair in \\u escape")
            }
            ErrorKind::InvalidUtf8 => write!(f, "invalid utf-8 sequence in string"),
            ErrorKind::RecursionLimitExceeded => write!(f, "recursion limit exceeded"),
            ErrorKind::UnknownField(field) => write!(f, "unknown field `{field}`"),
            ErrorKind::MissingField(field) => write!(f, "missing field `{field}`"),
            ErrorKind::UnknownVariant(v) => write!(f, "unknown variant `{v}`"),
            ErrorKind::InvalidType { expected, found } => {
                write!(f, "invalid type: expected {expected}, found {found}")
            }
            ErrorKind::InvalidLength { len, expected } => {
                write!(f, "invalid length {len}, expected {expected}")
            }
            ErrorKind::NonFiniteFloat => {
                write!(
                    f,
                    "floating point value cannot be represented as JSON (NaN or infinity)"
                )
            }
            ErrorKind::Custom(msg) => write!(f, "{msg}"),
        }?;

        if let (Some(line), Some(column)) = (self.line, self.column) {
            write!(f, " at line {line} column {column}")?;
        }
        Ok(())
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[cfg(feature = "std")]
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::io(err)
    }
}

impl From<String> for Error {
    fn from(msg: String) -> Self {
        Error::custom(msg)
    }
}

impl From<&str> for Error {
    fn from(msg: &str) -> Self {
        Error::custom(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn display_and_classify() {
        let e = Error::missing_field("x");
        assert!(e.to_string().contains("missing field `x`"));
        assert_eq!(e.classification(), "missing field");
        assert!(Error::custom("boom").is_custom());
        assert!(Error::custom("boom").to_string().contains("boom"));
    }
}
