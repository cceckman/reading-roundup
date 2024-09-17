use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{OriginalUri, Path, State},
    http::{
        header::{CONTENT_TYPE, LOCATION},
        StatusCode, Uri,
    },
    response::IntoResponse,
    routing::get,
    Form,
};
use chrono::NaiveDate;
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
    html: String,
    entry: ReadingListEntry,
}

fn destruct_roundup_row(row: &rusqlite::Row) -> rusqlite::Result<RoundupRow> {
    let html = markdown::to_html(&row.get::<_, String>("body_text")?);
    let included = row.get::<_, Option<bool>>("included")?.unwrap_or(false);
    Ok(RoundupRow {
        id: row.get("id")?,
        included,
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
    // TODO:"have read" via UI as well

    let mut server = server.lock().unwrap();
    match server.update_article(id, uri, body) {
        Ok(v) => v.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("unexpected error: {e}"),
        )
            .into_response(),
    }
}

impl Server {
    fn update_article(
        &mut self,
        id: isize,
        uri: Uri,
        new_body: &str,
    ) -> Result<impl IntoResponse, Error> {
        self.conn
            .prepare(
                r#"
            UPDATE reading_list
            SET body_text = :body_text
            WHERE id = :id
        "#,
            )?
            .execute(named_params! {":id" : id, ":body_text" : new_body})?;
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
        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body { main {
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
                    button label="Save" type="submit" { "Save" }
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
                (SELECT entry, 1 as included FROM roundup_contents WHERE date = :date)
                ON reading_list.id = entry
            ORDER BY included DESC, source_date DESC
            "#,
            )?
            .query_map(
                named_params! {":date": format!("{date}")},
                destruct_roundup_row,
            )?
            .collect();
        let rows = rows?;
        let included_rows = rows.iter().filter(|v| v.included);
        let excluded_rows = rows.iter().filter(|v| !v.included);

        Ok(maud::html! {
            head { link rel="stylesheet" href="/style.css"; }
            body {
                div class="article-meta" { h1 { "Reading Roundup" } }
                main {
                    div class="summary" {
                        h3 class="tile-title" { a { (format!("Reading Roundup, {date}")) } a { "Save" } }
                    table {
                    @for row in included_rows { tr {
                        td { (maud::PreEscaped(row.html.clone())) }
                        td { input type="checkbox" name=(format!("included-{}", row.id)) checked?[row.included] ; }
                        td { (format!("{}", row.entry.source_date)) }
                        td { a href=(format!("/articles/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                    } }
                    } }

                    h3 { "Add to this roundup: " }
                    table { @for row in excluded_rows { tr {
                        td { input type="checkbox" name=(format!("included-{}", row.id)) checked?[row.included] ; }
                        td { (format!("{}", row.entry.source_date)) }
                        td { a href=(format!("/articles/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                        td { (maud::PreEscaped(row.html.clone())) }
                    } } }
                }
            }
        })
    }
}
