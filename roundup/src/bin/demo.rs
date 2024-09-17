use std::path::PathBuf;

fn main() {
    tracing_subscriber::fmt::init();

    let from = PathBuf::from("~/Obsidian/Journal");
    let (ok, err) = roundup::scan_files(&from);

    eprintln!("found {} links", ok.len());
    for entry in ok {
        eprintln!("{entry}");
    }

    if !err.is_empty() {
        eprintln!("got {} errors", err.len());
        for err in err {
            eprintln!("{err}");
        }
    }
}
