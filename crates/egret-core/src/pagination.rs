//! Offset pagination request parameters.

use serde::{Deserialize, Serialize};

/// Pagination request. `page` is 1-based.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub per_page: u32,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: 50,
        }
    }
}

impl Pagination {
    /// Clamp to safe bounds and compute the SQL OFFSET.
    pub fn offset(&self) -> u32 {
        let page = self.page.max(1);
        let per_page = self.per_page.clamp(1, 500);
        (page - 1) * per_page
    }

    /// Clamped page size (the SQL LIMIT).
    pub fn limit(&self) -> u32 {
        self.per_page.clamp(1, 500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_page_has_zero_offset() {
        let p = Pagination {
            page: 1,
            per_page: 20,
        };
        assert_eq!(p.offset(), 0);
        assert_eq!(p.limit(), 20);
    }

    #[test]
    fn offset_computed_from_page() {
        let p = Pagination {
            page: 3,
            per_page: 20,
        };
        assert_eq!(p.offset(), 40);
    }

    #[test]
    fn per_page_is_clamped() {
        let p = Pagination {
            page: 1,
            per_page: 100_000,
        };
        assert_eq!(p.limit(), 500);
    }

    #[test]
    fn page_zero_treated_as_one() {
        let p = Pagination {
            page: 0,
            per_page: 10,
        };
        assert_eq!(p.offset(), 0);
    }
}
