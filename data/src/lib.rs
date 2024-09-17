use http::Uri;
use std::fmt::Display;

/// Entry in or for the reading-list database.
#[derive(Debug)]
pub struct ReadingListEntry {
    pub url: Uri,
    pub original_text: String,
    pub body_text: String,
    pub source_date: chrono::NaiveDate,
    pub read: Option<bool>,
}

impl Display for ReadingListEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} -- {}",
            self.source_date, self.url, self.body_text
        )
    }
}
