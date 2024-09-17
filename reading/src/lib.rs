// TODO:
// - Javascript to auto-save?

use std::{
    collections::HashMap,
    io::{Cursor, Write},
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{OriginalUri, Path, State},
    http::{
        header::{CONTENT_DISPOSITION, CONTENT_TYPE, LOCATION},
        uri::PathAndQuery,
        StatusCode, Uri,
    },
    response::IntoResponse,
    routing::get,
};
use axum_extra::extract::Form;
use chrono::NaiveDate;
use maud::PreEscaped;
use reading_roundup_data::ReadingListEntry;
use roundup::scan_files;
use rusqlite::{named_params, Connection};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("error in performing SQL query: {0}")]
    SqlError(#[from] rusqlite::Error),
    #[error("error in performing I/O query: {0}")]
    IoError(#[from] std::io::Error),
    #[error("error in scanning body: {0}")]
    ScanningError(#[from] roundup::RoundupErrorKind),
}

pub fn serve<P: AsRef<std::path::Path>>(db: P, sources: P) -> Result<axum::Router, Error> {
    let conn = Connection::open(db)?;
    let s = Arc::new(Mutex::new(Server {
        conn,
        sources: sources.as_ref().to_owned(),
    }));
    Ok(axum::Router::new()
        .route(
            "/",
            get(|| async { (StatusCode::FOUND, [(LOCATION, "roundups/")]) }),
        )
        .route("/update/", get(update))
        .route("/roundups/:date/", get(render_roundup).post(update_roundup))
        .route("/roundups/:date/md", get(render_roundup_md))
        .route("/roundups/", get(list_roundups).post(create_roundup))
        .route("/roundups/by-article/:id/", get(list_roundups_by_article))
        .route("/articles/", get(list_articles).post(create_article))
        .route("/articles/:id/", get(render_article).post(update_article))
        .route("/style.css", get(css))
        .with_state(s))
}

struct Server {
    conn: rusqlite::Connection,
    sources: PathBuf,
}

struct RoundupRow {
    id: isize,
    included: bool,
    count: isize,
    html: String,
    entry: ReadingListEntry,
}

async fn update(State(s): State<Arc<Mutex<Server>>>) -> impl IntoResponse {
    let dir = { s.lock().unwrap().sources.clone() };
    let (entries, errors) = scan_files(&dir);
    let mut s = s.lock().unwrap();
    let mut count_pre: isize = 0;
    let mut count_post: isize = 0;
    let tx_done: rusqlite::Result<()> = (|| {
        let mut tx = s.conn.transaction()?;
        count_pre = tx.query_row(
            "SELECT COUNT(url) FROM reading_list",
            named_params! {},
            |r| r.get(0),
        )?;
        roundup::insert(entries.iter(), &mut tx)?;
        count_post = tx.query_row(
            "SELECT COUNT(url) FROM reading_list",
            named_params! {},
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(())
    })();
    let html = maud::html!(
        head { link rel="stylesheet" href="/style.css"; }
        body { (nav(1)) main {
            h2 { "Update results" }
            h3 { "Scanning report" }
            p { (format!("{} links found, with {} errors", entries.len(), errors.len())) }
            @for error in &errors {
                p class="error scan-error" { (format!("{error}")) }
            }
            h3 { "Databse report" }
            @match tx_done {
                Ok(_) => { p { (format!("Update results: {} before, new total {}", count_pre, count_post)) } }
                Err(ref e) => { p class="error db-error" { (format!("Database error: {e}")) } }
            }
        } }
    );
    let code = if tx_done.is_err() || !errors.is_empty() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    };
    (code, html).into_response()
}

fn destruct_roundup_row(row: &rusqlite::Row) -> rusqlite::Result<RoundupRow> {
    let html = markdown::to_html(&row.get::<_, String>("body_text")?);
    let included = row.get::<_, Option<bool>>("included")?.unwrap_or(false);
    let count = row.get::<_, Option<isize>>("count")?.unwrap_or(0);
    Ok(RoundupRow {
        id: row.get("id")?,
        included,
        count,
        html,
        entry: destruct_entry(row)?,
    })
}

fn destruct_entry(row: &rusqlite::Row) -> rusqlite::Result<ReadingListEntry> {
    Ok(ReadingListEntry {
        url: row.get::<_, String>("url")?.parse().unwrap(),
        source_date: row.get::<_, String>("source_date")?.parse().unwrap(),
        original_text: row.get::<_, String>("original_text")?.to_owned(),
        body_text: row.get::<_, String>("body_text")?.to_owned(),
        read: row.get("read")?,
    })
}

async fn css() -> impl IntoResponse {
    static CSS_CONTENT: &str = include_str!("style.css");
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/css")],
        CSS_CONTENT.to_owned(),
    )
}

/// Render the editor for a roundup post.
async fn render_roundup(
    State(server): State<Arc<Mutex<Server>>>,
    Path(p): Path<String>,
) -> impl IntoResponse {
    let date: NaiveDate = match p.parse() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "reading roundups are identified by date",
            )
                .into_response()
        }
    };

    let mut server = server.lock().unwrap();
    match server.render_roundup(date) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

/// Render the editor for a roundup post.
async fn render_roundup_md(
    State(server): State<Arc<Mutex<Server>>>,
    Path(p): Path<String>,
) -> impl IntoResponse {
    let date: NaiveDate = match p.parse() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "reading roundups are identified by date",
            )
                .into_response()
        }
    };

    let mut server = server.lock().unwrap();
    match server.render_roundup_md(date) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn list_roundups(State(server): State<Arc<Mutex<Server>>>) -> impl IntoResponse {
    let mut server = server.lock().unwrap();
    match server.list_roundups() {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn list_roundups_by_article(
    State(server): State<Arc<Mutex<Server>>>,
    Path(id): Path<isize>,
) -> impl IntoResponse {
    let mut server = server.lock().unwrap();
    match server.list_roundups_by_article(id) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn list_articles(State(server): State<Arc<Mutex<Server>>>) -> impl IntoResponse {
    let mut server = server.lock().unwrap();
    match server.list_articles() {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

/// Render the editor for a roundup post.
async fn render_article(
    State(server): State<Arc<Mutex<Server>>>,
    Path(p): Path<isize>,
) -> impl IntoResponse {
    let mut server = server.lock().unwrap();
    match server.render_article(p) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn create_article(
    State(server): State<Arc<Mutex<Server>>>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let body = match form.get("text") {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "missing text for new article").into_response(),
    };

    let mut server = server.lock().unwrap();
    match server.create_article(body) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn update_article(
    State(server): State<Arc<Mutex<Server>>>,
    Path(id): Path<isize>,
    OriginalUri(uri): OriginalUri,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let body = match form.get("body_text") {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, "missing body_text for update").into_response(),
    };
    let read_state = form.get("read").map(|v| v == "read");

    let mut server = server.lock().unwrap();
    match server.update_article(id, uri, body, read_state) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

async fn update_roundup(
    State(server): State<Arc<Mutex<Server>>>,
    Path(date): Path<String>,
    OriginalUri(uri): OriginalUri,
    Form(form): Form<HashMap<String, Vec<isize>>>,
) -> impl IntoResponse {
    let date: NaiveDate = match date.parse() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "reading roundups are identified by date",
            )
                .into_response()
        }
    };
    let articles = form
        .get("article-included")
        .cloned()
        .unwrap_or_else(Vec::new);

    let mut server = server.lock().unwrap();
    match server.update_roundup(uri, date, &articles) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

/// Misleadingly named: an empty roundup has no data.
/// This "just" redirects to the relevant path to edit the new roundup.
async fn create_roundup(
    OriginalUri(uri): OriginalUri,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let date: NaiveDate = match form
        .get("new-roundup")
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing new-roundup date").into_response())
        .and_then(|s| {
            s.parse().map_err(|_| {
                (StatusCode::BAD_REQUEST, "invalid date for new roundup").into_response()
            })
        }) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let mut origin = uri.into_parts();
    origin.path_and_query = Some(match origin.path_and_query {
        None => PathAndQuery::from_str(&format!("{date}/")).unwrap(),
        Some(pq) => PathAndQuery::from_str(&(pq.path().to_owned() + &format!("{date}/"))).unwrap(),
    });

    (
        StatusCode::SEE_OTHER,
        [(LOCATION, Uri::from_parts(origin).unwrap().to_string())],
    )
        .into_response()
}

fn nav(depth: usize) -> PreEscaped<String> {
    let mut prefix = PathBuf::new();
    for _ in 0..depth {
        prefix.push("..");
    }
    let prefix = prefix.display();
    maud::html!(
        nav { ul class="menu" {
            li { a href=(format!("{prefix}/roundups/")) { "Roundups" } }
            li { a href=(format!("{prefix}/articles/")) { "Articles" } }
            li { a href=(format!("{prefix}/update/")) { "Update" } }
        } }
    )
}

impl Server {
    fn update_roundup(
        &mut self,
        uri: Uri,
        date: chrono::NaiveDate,
        articles: &[isize],
    ) -> Result<impl IntoResponse, Error> {
        let date_str = format!("{date}");
        // Do "remove all other entries" and "add new entries" as a single,
        // atomic, transaction.
        let tx = self.conn.transaction()?;
        tx.prepare("DELETE FROM roundup_contents WHERE date = :date")?
            .execute(named_params! {":date": &date_str})?;
        let mut st =
            tx.prepare("INSERT INTO roundup_contents (date, entry) VALUES (:date, :id)")?;
        for id in articles {
            st.execute(named_params! {":date": &date_str, ":id": id})?;
        }
        drop(st);
        tx.commit()?;
        Ok((StatusCode::SEE_OTHER, [(LOCATION, uri.to_string())]))
    }

    fn create_article(&mut self, new_body: &str) -> Result<impl IntoResponse, Error> {
        let now: chrono::NaiveDate = chrono::Local::now().date_naive();
        let entry: ReadingListEntry = roundup::scan_body(now, new_body)?;
        let mut tx = self.conn.transaction()?;
        roundup::insert([&entry].into_iter(), &mut tx)?;
        tx.commit()?;
        let id: isize = self.conn.query_row(
            "SELECT id FROM reading_list WHERE url = :url",
            named_params! {":url": entry.url.to_string()},
            |row| row.get(0),
        )?;

        Ok((StatusCode::SEE_OTHER, [(LOCATION, format!("{id}/"))]))
    }

    fn update_article(
        &mut self,
        id: isize,
        uri: Uri,
        new_body: &str,
        read_state: Option<bool>,
    ) -> Result<impl IntoResponse, Error> {
        self.conn
            .prepare(
                r#"
            UPDATE reading_list
            SET body_text = :body_text, read = :read
            WHERE id = :id
        "#,
            )?
            .execute(named_params! {":id" : id, ":body_text" : new_body, ":read": read_state})?;
        Ok((StatusCode::SEE_OTHER, [(LOCATION, uri.to_string())]))
    }
    fn render_article(&mut self, id: isize) -> Result<impl IntoResponse, Error> {
        // Query everything, prioritizing stuff in the roundup.
        let (count, entry) = self
            .conn
            .prepare(
                r#"
            SELECT *, COUNT(roundup_contents.date) as roundups
            FROM reading_list
            LEFT JOIN roundup_contents ON reading_list.id = roundup_contents.entry
            WHERE reading_list.id = :id
            "#,
            )?
            .query_row(named_params! {":id": id}, |row| {
                let count: isize = row.get("roundups")?;
                let entry = destruct_entry(row)?;
                Ok((count, entry))
            })?;
        let tbr = !entry.read.unwrap_or(true);
        let read = entry.read.unwrap_or(false);
        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(2))
                main {
                div class="summary" {
                    h3 {
                        a href=(entry.url) { (entry.url) }
                    }
                    h4 class="tile-title" {
                        p { (entry.source_date) }
                        p { a href=(format!("../../roundups/by-article/{id}/")) { (format!("{count} roundups")) } }
                    }
                }
                form action="" method="POST" {
                    div class="controls" {
                        span {
                            input type="radio" value="tbr" id="tbr" name="read" checked?[tbr];
                            label for="tbr" { "TBR" }
                            input type="radio" value="read" id="tbr" name="read" checked?[read];
                            label for="read" { "Read" }
                        }
                        button label="Save" type="submit" { "Save" }
                    }
                    details { summary { "Original" } pre { (entry.original_text) } }
                    details open {
                        summary { "Preview" }
                        div class="summary" {
                            (maud::PreEscaped(markdown::to_html(&entry.body_text)))
                        }
                    }
                    textarea name="body_text" { (entry.body_text) }
                }
            } }
        })
    }

    /// List all articles.
    fn list_articles(&mut self) -> Result<impl IntoResponse, Error> {
        let rows : Result<Vec<_>, _> = self.conn.prepare(r#"
            SELECT *
            FROM reading_list
            LEFT JOIN
                (SELECT entry, COUNT(DISTINCT date) as count, 1 as included FROM roundup_contents GROUP BY entry)
                ON reading_list.id = entry
            ORDER BY count ASC, source_date ASC
            "#)?.query_map(named_params! {}, destruct_roundup_row)?.collect();
        let entries = rows?;

        fn render_row(row: &RoundupRow) -> PreEscaped<String> {
            let unread_sigil = match row.entry.read {
                None => "?",
                Some(true) => "📖",
                Some(false) => "📕",
            };
            maud::html!( tr {
                    td { (maud::PreEscaped(row.html.clone())) }
                    td { a href=(format!("../roundups/by-article/{}/", row.id)) { (row.count) } }
                    td { (unread_sigil) }
                    td { (format!("{}", row.entry.source_date)) }
                    td { a href=(format!("/articles/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                }
            )
        }
        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(1))
                main {
                form method="post" { h3 class="tile-title" {
                    textarea name="text" class="narrow" {  }
                    button type="submit" { "Add article" }
                } }

                table { @for entry in entries { (render_row(&entry)) } }
                }
            }
        })
    }

    /// List the roundups that contain a particular article.
    fn list_roundups_by_article(&mut self, id: isize) -> Result<impl IntoResponse, Error> {
        let rows: Result<Vec<String>, _> = self
            .conn
            .prepare(
                "SELECT DISTINCT date FROM roundup_contents WHERE entry = :id ORDER BY date ASC",
            )?
            .query_map(named_params! {":id": id}, |row| row.get("date"))?
            .collect();
        let rows = rows?;

        Ok(maud::html! {
        head { link rel="stylesheet" href="/style.css"; }
        body {
            (nav(1))
            main {
                p { "Roundups including " a href=(format!("../../../articles/{id}/")) { "article " (id) }}
                @for date in rows {
                    div class="summary" {
                        h3 {
                            a href=(format!("../../{date}/")) { (date) }
                        }
                    }
                }
            }
        } })
    }

    /// List all roundups.
    fn list_roundups(&mut self) -> Result<impl IntoResponse, Error> {
        let rows: Result<Vec<String>, _> = self
            .conn
            .prepare("SELECT DISTINCT date FROM roundup_contents ORDER BY date ASC")?
            .query_map(named_params! {}, |row| row.get("date"))?
            .collect();
        let rows = rows?;

        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(1))
                main {
                form method="POST" class="summary" {
                    label for="new-roundup" { "Start a new roundup: " }
                    input type="date" id="new-roundup" name="new-roundup";
                    button type="submit" { "Go!" }
                }                @for date in rows {
                div class="summary" {
                    h3 {
                        a href=(format!("{date}/")) { (date) }
                    }
                }
                }
            } }
        })
    }

    fn render_roundup_md(&mut self, date: NaiveDate) -> Result<impl IntoResponse, Error> {
        let date_str = format!("{date}");
        let rows: Result<Vec<_>, _> = self
            .conn
            .prepare(
                r#"
            SELECT reading_list.body_text
            FROM roundup_contents LEFT JOIN reading_list ON reading_list.id = roundup_contents.entry
            WHERE roundup_contents.date = :date
            "#,
            )?
            .query_map(named_params! {":date": &date_str}, |row| row.get(0))?
            .collect();
        let bodies: Vec<String> = rows?;
        let mut s = Cursor::new(Vec::<u8>::new());
        write!(
            s,
            r#"---
title: "Reading Roundup, {date_str}"
date: {date_str}
---

"#
        )?;
        for body in bodies {
            writeln!(s, "{body}")?;
            // Additional newline as paragraph break
            writeln!(s)?;
        }
        Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "text/markdown; charset=UTF-8".to_owned()),
                (
                    CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{date}.md\""),
                ),
            ],
            s.into_inner(),
        ))
    }

    fn render_roundup(&mut self, date: NaiveDate) -> Result<impl IntoResponse, Error> {
        // Query everything, prioritizing stuff in the roundup.
        let rows: Result<Vec<_>, _> = self
            .conn
            .prepare(
                r#"
            SELECT *
            FROM reading_list
            LEFT JOIN
                (SELECT entry as entry2, COUNT(DISTINCT date) as count FROM roundup_contents GROUP BY entry2)
                ON reading_list.id = entry2
            LEFT JOIN
                (SELECT entry as entry1, 1 as included FROM roundup_contents WHERE date = :date)
                ON reading_list.id = entry1
            ORDER BY included DESC, count ASC, source_date ASC
            "#)?
            .query_map(
                named_params! {":date": format!("{date}")},
                destruct_roundup_row,
            )?
            .collect();
        let rows = rows?;
        let included_rows = rows.iter().filter(|v| v.included);
        let excluded_rows = rows.iter().filter(|v| !v.included);

        fn render_row(row: &RoundupRow) -> PreEscaped<String> {
            let unread = !row.entry.read.unwrap_or(false);
            maud::html!( tr {
                    td { (maud::PreEscaped(row.html.clone())) }
                    td { a href=(format!("../by-article/{}/", row.id)) { (row.count) } }
                    td { input type="checkbox" name="article-included" value=(row.id) checked?[row.included] disabled?[unread]; }
                    td { (format!("{}", row.entry.source_date)) }
                    td { a href=(format!("/articles/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                }
            )
        }

        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(2))
                main { form method="POST" {
                    div class="summary" {
                        h3 class="tile-title" {
                            (format!("Reading Roundup, {date}"))
                            a href="md" { "Download" }
                            button label="Save" type="submit" { "Save" }
                         }

                        table {
                            @for row in included_rows { (render_row(row)) }
                        }
                    }

                    h3 { "Add to this roundup: " }
                    table {
                    @for row in excluded_rows { (render_row(row)) }
                    }
                } }
            }
        })
    }
}
