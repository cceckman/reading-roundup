use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let server = reading::serve(&PathBuf::from("/home/cceckman/Obsidian/readdb.sqlite")).unwrap();

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, server).await.unwrap();
}
