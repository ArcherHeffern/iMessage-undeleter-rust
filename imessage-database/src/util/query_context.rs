/*!
 Contains logic for handling query filter configurations.
*/
use std::collections::BTreeSet;

use chrono::prelude::*;

use crate::{
    error::query_context::QueryContextError,
    util::dates::{TIMESTAMP_FACTOR, get_offset},
};

#[derive(Debug, Default, PartialEq, Eq)]
/// Represents filter configurations for a SQL query.
pub struct QueryContext {
    pub limit: Option<i32>,
    pub selected_handle_ids: Option<BTreeSet<i32>>,
    /// Selected chat IDs
    pub selected_chat_ids: Option<BTreeSet<i32>>,
}

impl QueryContext {
    /// Populate a [`QueryContext`] with limit on the number of messages retrieved
    /// # Example:
    ///
    /// ```
    /// use imessage_database::util::query_context::QueryContext;
    ///
    /// let mut context = QueryContext::default();
    /// context.set_limit(2);
    /// ```
    pub fn set_limit(&mut self, limit: i32) {
        self.limit = Some(limit);
    }

    /// Populate a [`QueryContext`] with a list of handle IDs to select
    ///
    /// # Example:
    ///
    /// ```
    /// use std::collections::BTreeSet;
    /// use imessage_database::util::query_context::QueryContext;
    ///
    /// let mut context = QueryContext::default();
    /// context.set_selected_handle_ids(BTreeSet::from([1, 2, 3]));
    /// ```
    pub fn set_selected_handle_ids(&mut self, selected_handle_ids: BTreeSet<i32>) {
        self.selected_handle_ids = (!selected_handle_ids.is_empty()).then_some(selected_handle_ids);
    }

    /// Populate a [`QueryContext`] with a list of chat IDs to select
    ///
    /// # Example:
    ///
    /// ```
    /// use std::collections::BTreeSet;
    /// use imessage_database::util::query_context::QueryContext;
    ///
    /// let mut context = QueryContext::default();
    /// context.set_selected_chat_ids(BTreeSet::from([1, 2, 3]));
    /// ```
    pub fn set_selected_chat_ids(&mut self, selected_chat_ids: BTreeSet<i32>) {
        self.selected_chat_ids = (!selected_chat_ids.is_empty()).then_some(selected_chat_ids);
    }

    /// Ensure a date string is valid
    fn sanitize_date(date: &str) -> Option<i64> {
        if date.len() < 9 {
            return None;
        }

        let year = date.get(0..4)?.parse::<i32>().ok()?;

        if !date.get(4..5)?.eq("-") {
            return None;
        }

        let month = date.get(5..7)?.parse::<u32>().ok()?;
        if month > 12 {
            return None;
        }

        if !date.get(7..8)?.eq("-") {
            return None;
        }

        let day = date.get(8..)?.parse::<u32>().ok()?;
        if day > 31 {
            return None;
        }

        let local = Local.with_ymd_and_hms(year, month, day, 0, 0, 0).single()?;
        let stamp = local.timestamp_nanos_opt().unwrap_or(0);

        Some(stamp - (get_offset() * TIMESTAMP_FACTOR))
    }

    /// Determine if the current `QueryContext` has any filters present
    ///
    /// # Example:
    ///
    /// ```
    /// use imessage_database::util::query_context::QueryContext;
    ///
    /// let mut context = QueryContext::default();
    /// assert!(!context.has_filters());
    /// context.set_start("2023-01-01");
    /// assert!(context.has_filters());
    /// ```
    #[must_use]
    pub fn has_filters(&self) -> bool {
        self.limit.is_some()
            || self.selected_chat_ids.is_some()
            || self.selected_handle_ids.is_some()
    }
}

#[cfg(test)]
mod use_tests {
    use crate::util::{
        query_context::QueryContext,
    };

    #[test]
    fn can_create() {
        let context = QueryContext::default();
        assert!(context.limit.is_none());
        assert!(!context.has_filters());
    }

    #[test]
    fn can_create_limit() {
        let mut context = QueryContext::default();
        context.set_limit(1);

        assert_eq!(context.limit, Some(1));
        assert!(context.limit.is_some());
        assert!(context.has_filters());
    }

}

#[cfg(test)]
mod id_tests {
    use std::collections::BTreeSet;

    use crate::util::query_context::QueryContext;

    #[test]
    fn test_can_set_selected_chat_ids() {
        let mut qc = QueryContext::default();
        qc.set_selected_chat_ids(BTreeSet::from([1, 2, 3]));

        assert_eq!(qc.selected_chat_ids, Some(BTreeSet::from([1, 2, 3])));
        assert!(qc.has_filters());
    }

    #[test]
    fn test_can_set_selected_chat_ids_empty() {
        let mut qc = QueryContext::default();
        qc.set_selected_chat_ids(BTreeSet::new());

        assert_eq!(qc.selected_chat_ids, None);
        assert!(!qc.has_filters());
    }

    #[test]
    fn test_can_overwrite_selected_chat_ids_empty() {
        let mut qc = QueryContext::default();
        qc.set_selected_chat_ids(BTreeSet::from([1, 2, 3]));
        qc.set_selected_chat_ids(BTreeSet::new());

        assert_eq!(qc.selected_chat_ids, None);
        assert!(!qc.has_filters());
    }

    #[test]
    fn test_can_set_selected_handle_ids() {
        let mut qc = QueryContext::default();
        qc.set_selected_handle_ids(BTreeSet::from([1, 2, 3]));

        assert_eq!(qc.selected_handle_ids, Some(BTreeSet::from([1, 2, 3])));
        assert!(qc.has_filters());
    }

    #[test]
    fn test_can_set_selected_handle_ids_empty() {
        let mut qc = QueryContext::default();
        qc.set_selected_handle_ids(BTreeSet::new());

        assert_eq!(qc.selected_handle_ids, None);
        assert!(!qc.has_filters());
    }

    #[test]
    fn test_can_overwrite_selected_handle_ids_empty() {
        let mut qc = QueryContext::default();
        qc.set_selected_handle_ids(BTreeSet::from([1, 2, 3]));
        qc.set_selected_handle_ids(BTreeSet::new());

        assert_eq!(qc.selected_handle_ids, None);
        assert!(!qc.has_filters());
    }
}

#[cfg(test)]
mod sanitize_tests {
    use crate::util::query_context::QueryContext;

    #[test]
    fn can_sanitize_good() {
        let res = QueryContext::sanitize_date("2020-01-01");
        assert!(res.is_some());
    }

    #[test]
    fn can_reject_bad_short() {
        let res = QueryContext::sanitize_date("1-1-20");
        assert!(res.is_none());
    }

    #[test]
    fn can_reject_bad_order() {
        let res = QueryContext::sanitize_date("01-01-2020");
        assert!(res.is_none());
    }

    #[test]
    fn can_reject_bad_month() {
        let res = QueryContext::sanitize_date("2020-31-01");
        assert!(res.is_none());
    }

    #[test]
    fn can_reject_bad_day() {
        let res = QueryContext::sanitize_date("2020-01-32");
        assert!(res.is_none());
    }

    #[test]
    fn can_reject_bad_data() {
        let res = QueryContext::sanitize_date("2020-AB-CD");
        assert!(res.is_none());
    }

    #[test]
    fn can_reject_wrong_hyphen() {
        let res = QueryContext::sanitize_date("2020–01–01");
        assert!(res.is_none());
    }
}
