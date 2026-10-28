use std::fmt;

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors produced while reading or writing NIF data.
#[derive(Debug)]
pub enum Error {
    /// Underlying I/O failure.
    Io(std::io::Error),
    /// The file does not look like a NIF at all.
    NotANif,
    /// The NIF is valid Gamebryo but not a version this crate supports.
    UnsupportedVersion { header_line: String, version: u32 },
    /// Big-endian NIFs (console exports) are not supported.
    BigEndian,
    /// Structurally invalid data encountered at a known offset.
    Corrupt { offset: usize, what: String },
    /// A typed decode was attempted on a block whose type does not match.
    TypeMismatch {
        expected: &'static str,
        actual: String,
    },
    /// A block index was out of range.
    BadBlockIndex(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "i/o error: {e}"),
            Error::NotANif => write!(f, "not a NIF file (bad header magic)"),
            Error::UnsupportedVersion {
                header_line,
                version,
            } => write!(
                f,
                "unsupported NIF version {:#010x} (header: {header_line:?}); \
                 this build supports 20.2.0.7 (Skyrim SE/LE)",
                version
            ),
            Error::BigEndian => write!(f, "big-endian NIFs are not supported"),
            Error::Corrupt { offset, what } => {
                write!(f, "corrupt NIF data at offset {offset:#x}: {what}")
            }
            Error::TypeMismatch { expected, actual } => {
                write!(
                    f,
                    "typed decode mismatch: expected {expected}, block is {actual}"
                )
            }
            Error::BadBlockIndex(i) => write!(f, "block index {i} out of range"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
