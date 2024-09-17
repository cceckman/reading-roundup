use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
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
        .route("/roundup/:date/", get(render_roundup))
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

fn destruct_row(row: &rusqlite::Row) -> rusqlite::Result<RoundupRow> {
    let html = markdown::to_html(&row.get::<_, String>("body_text")?);
    let included = row.get::<_, Option<bool>>("included")?.unwrap_or(false);
    Ok(RoundupRow {
        id: row.get("id")?,
        included,
        html,
        entry: ReadingListEntry {
            url: row.get::<_, String>("url")?.parse().unwrap(),
            source_date: row.get::<_, String>("source_date")?.parse().unwrap(),
            original_text: row.get::<_, String>("original_text")?.to_owned(),
            body_text: row.get::<_, String>("body_text")?.to_owned(),
            read: row.get("read")?,
        },
    })
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

impl Server {
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
            .query_map(named_params! {":date": format!("{date}")}, destruct_row)?
            .collect();
        let rows = rows?;

        Ok(maud::html! {
            h1 { (format!("Reading Roundup, {date}")) }
            form { table {
                @for row in rows { tr {
                    td { input type="checkbox" name=(format!("included-{}", row.id)) checked?[row.included] ; }
                    td { (format!("{}", row.entry.source_date)) }
                    td { a href=(format!("/article/{}/", row.id)) { (maud::PreEscaped("&nbsp;🖉&nbsp;")) } }
                    td { (maud::PreEscaped(row.html)) }
                } }
            } }
        })
    }
}
