use clap::Parser;
use listenfd::ListenFd;
use std::path::PathBuf;

#[derive(Parser, Debug)]
struct Args {
    /// Path of the database file.
    #[arg(long)]
    db: PathBuf,

    /// Path of the source files to search for new entries.
    #[arg(long, short = 'j')]
    journal: PathBuf,

    /// Bind to this address and TCP port (e.g. 0.0.0.0:3000).
    /// If unspecified (default), listen using the sd_listen_fd protocol.
    #[arg(long, short = 'l')]
    bind_pattern: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args: Args = Args::parse();

    let server =
        reading::serve(&args.db, &args.journal).expect("could not instantiate reading-list server");

    let mut listenfd = ListenFd::from_env();

    // TCP branch:
    let tcp_listener = {
        if let Some(v) = args.bind_pattern {
            tracing::info!("attempting to listen at {}", v);
            Some(
                tokio::net::TcpListener::bind(&v)
                    .await
                    .expect("could not open TCP socket"),
            )
        } else if let Ok(Some(v)) = listenfd.take_tcp_listener(0) {
            Some(
                tokio::net::TcpListener::from_std(v)
                    .expect("could not open TCP socket from environment"),
            )
        } else {
            None
        }
    };
    if let Some(tcp_listener) = tcp_listener {
        tracing::info!("starting server via TCP");
        axum::serve(tcp_listener, server).await.unwrap();
        return;
    }
    tracing::info!("no TCP server available, attempting to listen on Unix domain socket");
    let uds_listener = match listenfd.take_unix_listener(0) {
        Ok(Some(v)) => v,
        _ => {
            tracing::error!("no Unix domain socket available");
            std::process::exit(1);
        }
    };
    let uds_listener = tokio::net::UnixListener::from_std(uds_listener).unwrap();
    tracing::info!("listening on Unix domain socket");

    // Complex example: https://github.com/tokio-rs/axum/blob/main/examples/unix-domain-socket/src/main.rs
    use axum::http::Request;
    use hyper::body::Incoming;
    use hyper_util::{
        rt::{TokioExecutor, TokioIo},
        server,
    };
    use tower::Service;
    let mut make_service = server.into_make_service();
    // Accepting loop:
    while let Ok((socket, remote_addr)) = uds_listener.accept().await {
        tracing::debug!("got a connection from {:?}", remote_addr);
        // Task for the socket
        let service = make_service.call(&socket).await.unwrap();
        tokio::spawn(async move {
            let socket = TokioIo::new(socket);
            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                service.clone().call(request)
            });

            if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(socket, hyper_service)
                .await
            {
                tracing::error!("failed to serve connection: {err:#}");
            }
        });
    }
}
