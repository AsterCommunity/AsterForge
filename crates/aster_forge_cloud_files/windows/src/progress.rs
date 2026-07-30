//! Product-neutral CFAPI provider progress validation.

use std::cmp::Ordering;

use crate::{Result, WindowsCloudFilesError};

/// One provider progress sample for a pending CFAPI transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsFetchDataProgress {
    total: u64,
    completed: u64,
}

impl WindowsFetchDataProgress {
    /// Creates a progress sample. Zero-sized work is represented as `(0, 0)`.
    pub fn new(total: u64, completed: u64) -> Result<Self> {
        if completed > total {
            return Err(WindowsCloudFilesError::InvalidProviderProgress {
                reason: "completed bytes exceed total bytes",
            });
        }
        if total > i64::MAX as u64 {
            return Err(WindowsCloudFilesError::InvalidProviderProgress {
                reason: "total bytes exceed signed CFAPI boundary",
            });
        }
        if completed > i64::MAX as u64 {
            return Err(WindowsCloudFilesError::InvalidProviderProgress {
                reason: "completed bytes exceed signed CFAPI boundary",
            });
        }
        Ok(Self { total, completed })
    }

    /// Returns the total number of bytes in the provider operation.
    pub const fn total(self) -> u64 {
        self.total
    }

    /// Returns the number of bytes completed so far.
    pub const fn completed(self) -> u64 {
        self.completed
    }

    /// Returns the signed values accepted by `CfReportProviderProgress`.
    pub const fn as_cfapi(self) -> (i64, i64) {
        (self.total as i64, self.completed as i64)
    }

    /// Validates a new sample against a previous sample for the same transfer.
    pub fn advance_from(self, previous: Self) -> Result<Self> {
        if self.total != previous.total {
            return Err(WindowsCloudFilesError::InvalidProviderProgress {
                reason: "progress total changed during one transfer",
            });
        }
        match self.completed.cmp(&previous.completed) {
            Ordering::Less => Err(WindowsCloudFilesError::InvalidProviderProgress {
                reason: "completed bytes regressed during one transfer",
            }),
            Ordering::Equal | Ordering::Greater => Ok(self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_accepts_zero_and_exact_completion() {
        assert_eq!(
            WindowsFetchDataProgress::new(0, 0).unwrap().as_cfapi(),
            (0, 0)
        );
        assert_eq!(
            WindowsFetchDataProgress::new(i64::MAX as u64, i64::MAX as u64)
                .unwrap()
                .as_cfapi(),
            (i64::MAX, i64::MAX)
        );
    }

    #[test]
    fn progress_rejects_overflow_and_regression() {
        assert!(matches!(
            WindowsFetchDataProgress::new(1, 2),
            Err(WindowsCloudFilesError::InvalidProviderProgress { .. })
        ));
        assert!(matches!(
            WindowsFetchDataProgress::new(i64::MAX as u64 + 1, 0),
            Err(WindowsCloudFilesError::InvalidProviderProgress { .. })
        ));
        let previous = WindowsFetchDataProgress::new(10, 5).unwrap();
        let next = WindowsFetchDataProgress::new(10, 4).unwrap();
        assert!(matches!(
            next.advance_from(previous),
            Err(WindowsCloudFilesError::InvalidProviderProgress { .. })
        ));
        let changed_total = WindowsFetchDataProgress::new(11, 5).unwrap();
        assert!(changed_total.advance_from(previous).is_err());
    }
}
