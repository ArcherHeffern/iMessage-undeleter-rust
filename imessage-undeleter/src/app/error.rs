/*!
 Errors that can happen during the application's runtime.
*/

use std::{
    fmt::{Display, Formatter, Result},
    io::Error as IoError,
};

use crabapple::error::BackupError;
use imessage_database::{error::table::TableError};

/// Errors that can happen during the application's runtime
#[derive(Debug)]
pub enum RuntimeError {
    InvalidOptions(String),
    DiskError(IoError),
    DatabaseError(TableError),
    BackupError(BackupError),
    FileNameError,
}

impl Display for RuntimeError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result {
        match self {
            RuntimeError::InvalidOptions(why) => write!(fmt, "Invalid options!\n{why}"),
            RuntimeError::DiskError(why) => write!(fmt, "{why}"),
            RuntimeError::DatabaseError(why) => write!(fmt, "{why}"),
            RuntimeError::BackupError(why) => write!(fmt, "{why}"),
            RuntimeError::FileNameError => write!(fmt, "Invalid file name!"),
        }
    }
}

impl From<TableError> for RuntimeError {
    fn from(err: TableError) -> Self {
        RuntimeError::DatabaseError(err)
    }
}

impl From<BackupError> for RuntimeError {
    fn from(err: BackupError) -> Self {
        RuntimeError::BackupError(err)
    }
}

impl From<IoError> for RuntimeError {
    fn from(err: IoError) -> Self {
        RuntimeError::DiskError(err)
    }
}
