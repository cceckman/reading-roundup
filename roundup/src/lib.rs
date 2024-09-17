use chrono::NaiveDate;
use http::Uri;
use markdown::mdast::Node;
use regex_lite::Regex;
use std::{
    ffi::OsStr,
    fmt::Display,
    fs::{read_dir, File},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::LazyLock,
    vec,
};
use thiserror::Error;

static ENTRY_REGEX: std::sync::LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("^.*#(reading|read|tbr)[ :]*(.*)$").expect("invalid regex provided")
});

#[derive(Error, Debug)]
#[error("error in getting links from {file}: {kind}")]
pub struct RoundupError {
    file: PathBuf,
    #[source]
    kind: RoundupErrorKind,
}

#[derive(Error, Debug)]
pub enum RoundupErrorKind {
    #[error("I/O error scanning input file: {0}")]
    ScanIOError(#[from] std::io::Error),
    #[error("I/O error walking directories: {0}")]
    StatIOError(std::io::Error),
    #[error("invalid input file name: {0}")]
    InvalidFile(&'static str),
    #[error("error parsing Markdown string: {0}")]
    MarkdownError(String),
    #[error("no valid link found in body: {0}")]
    MissingLink(String),
}

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

/// Recursive visitor to search for a URI.
fn find_url(node: &Node) -> Option<Uri> {
    match node {
        Node::Link(link) => return link.url.parse().ok(),
        _ => {
            if let Some(children) = node.children() {
                for child in children {
                    if let Some(v) = find_url(child) {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

/// Scan the file at the given path and find any reading-list entries in it.
pub fn scan_file(file: &Path) -> Result<Vec<ReadingListEntry>, RoundupErrorKind> {
    let stem = file.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        RoundupErrorKind::InvalidFile("cannot determine file stem, or stem is not UTF-8")
    })?;
    let source_date: NaiveDate = stem
        .parse()
        .map_err(|_| RoundupErrorKind::InvalidFile("file stem is not YYYY-MM-DD"))?;

    let parseopts = markdown::ParseOptions {
        constructs: markdown::Constructs {
            autolink: true,
            ..markdown::Constructs::default()
        },
        ..markdown::ParseOptions::default()
    };

    let f = BufReader::new(File::open(file)?);
    let mut entries = Vec::new();
    for line in f.lines() {
        let line = line?;
        if let Some(captures) = ENTRY_REGEX.captures(&line) {
            let tag = captures
                .get(1)
                .expect("failed to retrieve non-optional capture of tag");
            let body = captures
                .get(2)
                .expect("failed to retrieve non-optional capture of body");

            let read = match tag.as_str() {
                "read" => Some(true),
                "tbr" => Some(false),
                _ => None,
            };

            let body_ast = markdown::to_mdast(body.as_str(), &parseopts)
                .map_err(|_err| RoundupErrorKind::MarkdownError(line.clone()))?;
            let url =
                find_url(&body_ast).ok_or_else(|| RoundupErrorKind::MissingLink(line.clone()))?;
            entries.push(ReadingListEntry {
                url,
                body_text: body.as_str().to_owned(),
                original_text: line.clone(),
                source_date,
                read,
            })
        }
    }

    Ok(entries)
}

/// Scan all the files in the provided directory, recursively, and collect their reading-list
/// entries and errors.
pub fn scan_files(dir: &Path) -> (Vec<ReadingListEntry>, Vec<RoundupError>) {
    let mut ok = Vec::new();
    let mut err = Vec::new();
    let mut dir_stack = vec![dir.to_owned()];
    while let Some(dir) = dir_stack.pop() {
        tracing::debug!("visiting directory {}", dir.display());
        let it = match read_dir(&dir) {
            Ok(it) => it,
            Err(e) => {
                err.push(RoundupError {
                    file: dir.clone(),
                    kind: RoundupErrorKind::StatIOError(e),
                });
                continue;
            }
        };
        for direntry in it {
            let (path, metadata) =
                match direntry.and_then(|v| v.metadata().map(|md| (v.path(), md))) {
                    Ok(v) => v,
                    Err(e) => {
                        err.push(RoundupError {
                            file: dir.clone(),
                            kind: RoundupErrorKind::StatIOError(e),
                        });
                        continue;
                    }
                };
            tracing::debug!(
                "visiting path {} (directory: {})",
                path.display(),
                metadata.is_dir()
            );
            if metadata.is_dir() {
                dir_stack.push(path);
            } else if path.extension().map(|ext| ext == "md").unwrap_or(false) {
                match scan_file(&path) {
                    Ok(mut v) => ok.append(&mut v),
                    Err(e) => err.push(RoundupError {
                        file: path.clone(),
                        kind: e,
                    }),
                }
            }
        }
    }

    (ok, err)
}
