use std::path::PathBuf;

fn main() {
    tracing_subscriber::fmt::init();

    let from = PathBuf::from("/home/cceckman/Obsidian/Journal");
    let to = PathBuf::from("/home/cceckman/Obsidian/readdb.sqlite");
    let (ok, err) = roundup::scan_files(&from);

    eprintln!("found {} links", ok.len());
    if !err.is_empty() {
        eprintln!("got {} errors", err.len());
        for err in err {
            eprintln!("{err}");
        }
        std::process::exit(1);
    }

    let n = roundup::insert(&ok, &to).unwrap();
    eprintln!("{n} links in the database");
}
