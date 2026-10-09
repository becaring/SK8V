//! GTA's archives read in place as the folders setup used to extract
//! (tools/gta_extract.py `placements` / `vehicles`), so a hard disk reads only
//! the entries the bake uses instead of writing and re-reading 3 GB of copies.
//!
//! Once `mount_*` has run, `read`, `read_to_string` and `files_under` serve
//! paths below the virtual root `ROOT`; without a mount they are `std::fs`.
use gta_archives::{Archive, Entry, GtaKeys, any_match, walk, walk_order};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub const ROOT: &str = "gta-archives";

enum File {
    Entry(Entry),
    Text(String),
}

struct Vfs {
    keys: GtaKeys,
    /// Lower-case `/` path -> (path as listed, contents).
    files: HashMap<String, (String, File)>,
}

static VFS: OnceLock<Vfs> = OnceLock::new();

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_ascii_lowercase()
}

pub fn read(p: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let p = p.as_ref();
    let Some(v) = VFS.get() else { return std::fs::read(p) };
    match v.files.get(&norm(p)) {
        Some((_, File::Entry(e))) => e.read(&v.keys).map_err(io::Error::other),
        Some((_, File::Text(t))) => Ok(t.as_bytes().to_vec()),
        None => Err(io::ErrorKind::NotFound.into()),
    }
}

pub fn read_to_string(p: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(p)?).map_err(io::Error::other)
}

/// Files below `dir` with extension `ext`, in the order a recursive NTFS
/// directory walk meets them; `None` when nothing is mounted.
pub fn files_under(dir: &Path, ext: &str) -> Option<Vec<PathBuf>> {
    let v = VFS.get()?;
    let prefix = format!("{}/", norm(dir));
    let suffix = format!(".{}", ext.to_ascii_lowercase());
    let mut out: Vec<&String> = v
        .files
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix) && k.ends_with(&suffix))
        .map(|(_, (p, _))| p)
        .collect();
    out.sort_by_cached_key(|p| walk_order(p));
    Some(out.into_iter().map(PathBuf::from).collect())
}

fn open(path: &Path, keys: &GtaKeys) -> Option<Arc<Archive>> {
    match Archive::open(path, keys) {
        Ok(a) => Some(Arc::new(a)),
        Err(e) => {
            eprintln!("skipped {}: {e}", path.display());
            None
        }
    }
}

fn add(files: &mut HashMap<String, (String, File)>, dir: &str, archive: Arc<Archive>, keys: &GtaKeys, patterns: &[String]) {
    let mut failed = Vec::new();
    for e in walk(archive, keys, &|p| any_match(p, patterns), &mut failed) {
        let path = format!("{dir}/{}", e.path);
        files.insert(path.to_ascii_lowercase(), (path, File::Entry(e)));
    }
    for f in failed {
        eprintln!("{dir}: {f}");
    }
}

fn install(keys: GtaKeys, files: HashMap<String, (String, File)>) {
    eprintln!("{} files read in place from GTA's archives", files.len());
    gta_archives::prefetch(files.values().filter_map(|(_, f)| match f {
        File::Entry(e) => Some(e),
        File::Text(_) => None,
    }));
    if VFS.set(Vfs { keys, files }).is_err() {
        panic!("vfs mounted twice");
    }
}

/// gta_extract.py's `PLACEMENT_PATTERNS`: archetypes, prop and interior drawables, fragments.
const PLACEMENTS: [&str; 5] = ["*.ytyp", "*/props/*.ydr", "*/props/*.yft", "*/interiors/*.ydr", "*trailer*.yft"];

/// `ROOT/base/<rpf stem>/...` for every top-level archive, `ROOT/layers/NN_<layer>/...`
/// for update.rpf and each Story Mode patch pack in dlclist.xml order (packs with
/// their `content.xml` / `setup2.xml`), and `ROOT/layers.txt`: the `gta-meta`
/// layout `--templates` reads.
pub fn mount_placements(gta: &Path, keys: GtaKeys) -> io::Result<()> {
    let patterns: Vec<String> = PLACEMENTS.iter().map(|p| p.to_string()).collect();
    let mut files = HashMap::new();
    let mut base: Vec<PathBuf> = std::fs::read_dir(gta)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")))
        .collect();
    base.sort();
    for a in &base {
        let stem = a.file_stem().unwrap_or_default().to_string_lossy();
        if let Some(archive) = open(a, &keys) {
            add(&mut files, &format!("{ROOT}/base/{stem}"), archive, &keys, &patterns);
        }
    }
    let update = gta.join("update/update.rpf");
    let root = Arc::new(Archive::open(&update, &keys).map_err(io::Error::other)?);
    let mut names = Vec::new();
    for (i, (name, archive)) in layers(gta, &root, &keys)?.into_iter().enumerate() {
        let dir = format!("{ROOT}/layers/{i:02}_{name}");
        if i > 0 {
            // rage's non-recursive extract of 'content.xml' 'setup2.xml' (substring patterns).
            for f in archive.files() {
                if f.path.contains("content.xml") || f.path.contains("setup2.xml") {
                    let path = format!("{dir}/{}", f.path);
                    let e = Entry { archive: archive.clone(), file: f.clone(), path: f.path.clone() };
                    files.insert(path.to_ascii_lowercase(), (path, File::Entry(e)));
                }
            }
        }
        add(&mut files, &dir, archive, &keys, &patterns);
        names.push(format!("{i:02}_{name}"));
    }
    let list = format!("{ROOT}/layers.txt");
    files.insert(list.clone(), (list, File::Text(names.join("\n") + "\n")));
    install(keys, files);
    Ok(())
}

/// update.rpf, then the Story Mode patch packs in dlclist.xml order (lowest
/// priority first); MP packs (map changes load only in Online) and *g9ec* packs are skipped.
fn layers(gta: &Path, update: &Arc<Archive>, keys: &GtaKeys) -> io::Result<Vec<(String, Arc<Archive>)>> {
    let mut failed = Vec::new();
    let list = walk(update.clone(), keys, &|p| p == "dlclist.xml" || p.ends_with("/dlclist.xml"), &mut failed);
    let text = list
        .first()
        .ok_or_else(|| io::Error::other("dlclist.xml not found in update.rpf"))?
        .read(keys)
        .map_err(io::Error::other)?;
    let text = String::from_utf8_lossy(&text);
    let mut out = vec![("update".to_string(), update.clone())];
    for pack in text.split("dlcpacks:/").skip(1).map(|s| &s[..s.find(['/', '<']).unwrap_or(s.len())]) {
        let rpf = gta.join(format!("update/x64/dlcpacks/{pack}/dlc.rpf"));
        if pack.starts_with("patch") && !pack.contains("g9ec") && rpf.exists() {
            if let Some(a) = open(&rpf, keys) {
                out.push((pack.to_string(), a));
            }
        }
    }
    Ok(out)
}

/// `ROOT/<layer>/...` vehicle fragments (not the render-only `_hi`): `00_common`
/// from x64e.rpf, `50_<pack>_<rpf stem>` for every pack's `dlc*.rpf`, then
/// `99_update` / `99_update2`: the `gta-vehyft` layout `--vehicle-bounds` reads.
pub fn mount_vehicles(gta: &Path, keys: GtaKeys) -> io::Result<()> {
    let mut jobs = vec![("00_common".to_string(), gta.join("x64e.rpf"))];
    let mut packs: Vec<PathBuf> = std::fs::read_dir(gta.join("update/x64/dlcpacks"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    packs.sort();
    for pack in packs {
        let mut rpfs: Vec<PathBuf> = std::fs::read_dir(&pack)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
                n.starts_with("dlc") && n.ends_with(".rpf") && p.is_file()
            })
            .collect();
        rpfs.sort();
        let name = pack.file_name().unwrap_or_default().to_string_lossy().into_owned();
        jobs.extend(rpfs.into_iter().map(|r| (format!("50_{name}_{}", r.file_stem().unwrap_or_default().to_string_lossy()), r)));
    }
    for u in ["update", "update2"] {
        let rpf = gta.join(format!("update/{u}.rpf"));
        if rpf.exists() {
            jobs.push((format!("99_{u}"), rpf));
        }
    }
    let patterns = vec!["*vehicles*.yft".to_string()];
    let mut files = HashMap::new();
    for (layer, rpf) in jobs {
        let Some(archive) = open(&rpf, &keys) else { continue };
        let mut failed = Vec::new();
        let want = |p: &str| any_match(p, &patterns) && !p.ends_with("_hi.yft");
        for e in walk(archive, &keys, &want, &mut failed) {
            let path = format!("{ROOT}/{layer}/{}", e.path);
            files.insert(path.to_ascii_lowercase(), (path, File::Entry(e)));
        }
    }
    install(keys, files);
    Ok(())
}

/// The mounted top-level folders (`vehicles` layers), sorted by name.
pub fn top_dirs() -> Option<Vec<PathBuf>> {
    let v = VFS.get()?;
    let mut dirs: Vec<String> = v
        .files
        .values()
        .filter_map(|(p, _)| p.strip_prefix(&format!("{ROOT}/"))?.split('/').next().map(|d| format!("{ROOT}/{d}")))
        .collect();
    dirs.sort();
    dirs.dedup();
    Some(dirs.into_iter().map(PathBuf::from).collect())
}

/// Files below `dir`, sorted as `vehicles::fragments` sorts a real folder (by name, byte order).
pub fn files_sorted(dir: &Path) -> Option<Vec<PathBuf>> {
    let v = VFS.get()?;
    let prefix = format!("{}/", norm(dir));
    let mut out: Vec<&String> = v.files.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, (p, _))| p).collect();
    out.sort_by_cached_key(|p| p.split('/').map(str::to_string).collect::<Vec<_>>());
    Some(out.into_iter().map(PathBuf::from).collect())
}
