//! `rage extract --recursive` for setup, reading only what the patterns name:
//!
//!     sk8v-rpf [--no-update-check] --keys <dir> extract <archive> [pattern]... -o <dir> [--recursive]
//!
//! Same output paths and bytes, same "Extracted: N / M  Failed: F" line. A
//! nested archive that cannot be opened is a warning; exit 1 only when a
//! wanted entry fails.

use gta_archives::{Archive, any_match, keys, walk};
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut keys_dir, mut out, mut archive, mut patterns, mut command) = (None, None, None, Vec::new(), None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-update-check" | "--recursive" | "-r" => {}
            "--keys" => keys_dir = args.next().map(PathBuf::from),
            "-o" | "--output" => out = args.next().map(PathBuf::from),
            _ if command.is_none() => command = Some(a),
            _ if archive.is_none() => archive = Some(PathBuf::from(a)),
            _ => patterns.push(a.to_lowercase()),
        }
    }
    let usage = "usage: sk8v-rpf --keys <dir> extract <archive> [pattern]... -o <dir>";
    let (Some("extract"), Some(keys_dir), Some(archive), Some(out)) = (command.as_deref(), keys_dir, archive, out) else {
        anyhow::bail!(usage);
    };
    let keys = keys(&keys_dir)?;
    let root = Arc::new(Archive::open(&archive, &keys)?);
    let mut warnings = Vec::new();
    let entries = walk(root, &keys, &|p| any_match(p, &patterns), &mut warnings);
    for w in &warnings {
        eprintln!("{w}");
    }
    let (mut ok, mut failed) = (0usize, 0usize);
    for e in &entries {
        let dest = out.join(&e.path);
        let written = e.read(&keys).and_then(|bytes| {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok(std::fs::write(&dest, bytes)?)
        });
        match written {
            Ok(()) => ok += 1,
            Err(err) => {
                eprintln!("Failed to extract {}: {err}", e.path);
                failed += 1;
            }
        }
    }
    println!("Extracted: {ok} / {}  Failed: {failed}", entries.len());
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
