// TODO:
// - Link at the top to the "roundups" page
// - post handler for each roundup; save / update
// - Javascript to auto-save?

use std::{
    collections::HashMap,
    str::FromStr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{OriginalUri, Path, State},
    http::{
        header::{CONTENT_TYPE, LOCATION},
        uri::PathAndQuery,
        StatusCode, Uri,
    },
    response::IntoResponse,
    routing::get,
    Form,
};
use chrono::NaiveDate;
use maud::PreEscaped;
use reading_roundup_data::ReadingListEntry;
use rusqlite::{named_params, Connection};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("error in performing SQL query: {0}")]
    SqlError(#[from] rusqlite::Error),
}

pub fn serve(db: &std::path::Path) -> Result<axum::Router, Error> {
    let conn = Connection::open(db)?;
    let s = Arc::new(Mutex::new(Server { conn }));
    Ok(axum::Router::new()
        .route("/roundups/:date/", get(render_roundup))
        .route("/roundups/", get(list_roundups).post(create_roundup))
        .route("/articles/:id/", get(render_article).post(update_article))
        .route("/style.css", get(css))
        .with_state(s))
}

struct Server {
    conn: rusqlite::Connection,
}

struct RoundupRow {
    id: isize,
    included: bool,
    count: isize,
    html: String,
    entry: ReadingListEntry,
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
    // TODO: support "get as YAML" too
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

async fn list_roundups(State(server): State<Arc<Mutex<Server>>>) -> impl IntoResponse {
    let mut server = server.lock().unwrap();
    match server.list_roundups(None) {
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
    // TODO: support "get as YAML" too
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

fn nav(individual: bool) -> PreEscaped<String> {
    let path = if individual { "../.." } else { ".." };
    maud::html!(
        nav { ul class="menu" {
            li { a href=(format!("{path}/roundups/")) { "Roundups" } }
            /* li { a href=(format!("{path}/articles/")) { "Articles" } } */
        } }
    )
}

impl Server {
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
    fn render_article(
        &mut self,
        id: isize,
        // TODO: support "get as YAML" too
    ) -> Result<impl IntoResponse, Error> {
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
                (nav(true))
                main {
                div class="summary" {
                    h3 {
                        a href=(entry.url) { (entry.url) }
                    }
                    h4 class="tile-title" {
                        p { (entry.source_date) }
                        p { (format!("{count} roundups")) }
                    }
                }
                form action="" method="POST" {
                    div class="controls" {
                        button label="Save" type="submit" { "Save" }
                        span {
                            input type="radio" value="tbr" id="tbr" name="read" checked?[tbr];
                            label for="tbr" { "TBR" }
                            input type="radio" value="read" id="tbr" name="read" checked?[read];
                            label for="read" { "Read" }
                        }
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

    /// List the roundups. If with_article is provided, filter to just those that have the article
    /// present.
    fn list_roundups(&mut self, with_article: Option<isize>) -> Result<impl IntoResponse, Error> {
        let rows : Result<Vec<String>, _> = match with_article {
            None => self.conn.prepare("SELECT DISTINCT date FROM roundup_contents ORDER BY date ASC")?.query_map(named_params! {}, |row| row.get("date"))?.collect(),
            Some(id) => self.conn.prepare("SELECT DISTINCT date FROM roundup_contents WHERE entry = :id ORDER BY date ASC")?.query_map(named_params! {":id": id}, |row| row.get("date"))?.collect()
        };
        let rows = rows?;

        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(false))
                main {
                form method="POST" class="summary" {
                    label for="new-roundup" { "Start a new roundup: " }
                    input type="date" id="new-roundup" name="new-roundup";
                    button type="submit" { "Go!" }
                }
                @for date in rows {
                div class="summary" {
                    h3 {
                        a href=(format!("{date}/")) { (date) }
                    }
                }
                }
            } }
        })
    }

    fn render_roundup(
        &mut self,
        date: NaiveDate,
        // TODO: support "get as YAML" too
    ) -> Result<impl IntoResponse, Error> {
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
            ORDER BY included DESC, count ASC, source_date DESC
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
                    td { (row.count) }
                    td { input type="checkbox" name=(format!("included-{}", row.id)) checked?[row.included] disabled?[unread]; }
                    td { (format!("{}", row.entry.source_date)) }
                    td { a href=(format!("/articles/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                }
            )
        }

        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                (nav(true))
                main {
                    div class="summary" {
                        h3 class="tile-title" { a { (format!("Reading Roundup, {date}")) } a { "Save" } }
                        table {
                        @for row in included_rows { (render_row(row)) }
                        }
                    }

                    h3 { "Add to this roundup: " }
                    table {
                    @for row in excluded_rows { (render_row(row)) }
                    }
                }
            }
        })
    }
}
