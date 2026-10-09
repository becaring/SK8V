//! GTA V's archives read in place, for setup: an archive is memory-mapped and
//! only its tables and the entries asked for are read, so a hard disk reads
//! megabytes where `rage extract --recursive` (which decompresses every entry
//! before matching it) read the whole game for each step.
//!
//! Paths and bytes are the ones `rage extract --recursive` writes: an entry's
//! path within its archive, nested archives as folders, resources with their
//! RSC7 header. `Archive` follows rage-cli's `rpf.rs` (VIRUXE, Unlicense).

use anyhow::Result;
use rpf_archive::{DirNode, RpfArchive, build_directory_tree, list_all_files};
pub use rpf_archive::{FileRef, GtaKeys};
use std::path::{Path, PathBuf};
use std::sync::Arc;

enum Buffer {
    Owned(Vec<u8>),
    Mapped(memmap2::Mmap),
}

/// The bytes an archive reads from: a shared buffer and this archive's window of it.
#[derive(Clone)]
struct Backing {
    buffer: Arc<Buffer>,
    range: std::ops::Range<usize>,
}

impl Backing {
    fn bytes(&self) -> &[u8] {
        let all: &[u8] = match &*self.buffer {
            Buffer::Owned(v) => v,
            Buffer::Mapped(m) => m,
        };
        &all[self.range.clone()]
    }
}

pub struct Archive {
    archive: RpfArchive,
    root: DirNode,
    data: Backing,
}

impl Archive {
    /// Memory-maps `path` (a plain read where mapping fails, e.g. an empty file).
    pub fn open(path: &Path, keys: &GtaKeys) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: read-only map; an archive changed by another process meanwhile reads as garbage, not UB.
        let data = match unsafe { memmap2::Mmap::map(&file) } {
            Ok(map) => Backing { range: 0..map.len(), buffer: Arc::new(Buffer::Mapped(map)) },
            Err(_) => {
                let v = std::fs::read(path)?;
                Backing { range: 0..v.len(), buffer: Arc::new(Buffer::Owned(v)) }
            }
        };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        Self::from_backing(data, name, keys)
    }

    fn from_backing(data: Backing, name: &str, keys: &GtaKeys) -> Result<Self> {
        let archive = RpfArchive::parse(data.bytes(), name, Some(keys))?;
        let root = build_directory_tree(&archive.entries);
        Ok(Self { archive, root, data })
    }

    /// A nested `.rpf`: a window onto this archive's bytes when stored plain (always, in practice).
    pub fn open_nested(&self, file: &FileRef, keys: &GtaKeys) -> Result<Self> {
        if let Some(range) = self.stored_range(file) {
            let mut data = self.data.clone();
            data.range = data.range.start + range.start..data.range.start + range.end;
            return Self::from_backing(data, &file.name, keys);
        }
        let bytes = self.extract(file, keys)?;
        let data = Backing { range: 0..bytes.len(), buffer: Arc::new(Buffer::Owned(bytes)) };
        Self::from_backing(data, &file.name, keys)
    }

    fn stored_range(&self, file: &FileRef) -> Option<std::ops::Range<usize>> {
        let rpf_archive::RpfEntryKind::BinaryFile { file_offset, file_size, uncompressed_size, is_encrypted } =
            self.archive.entries[file.entry_index].kind
        else {
            return None;
        };
        if is_encrypted || (file_size != 0 && file_size != uncompressed_size) || uncompressed_size == 0 {
            return None;
        }
        let offset = match self.archive.version {
            rpf_archive::RpfVersion::V7 => file_offset as usize * 512,
            _ => file_offset as usize,
        };
        let start = self.archive.start_offset + offset;
        let end = start.checked_add(uncompressed_size as usize)?;
        (end <= self.data.range.len()).then_some(start..end)
    }

    /// Where the entry's stored bytes lie: (mapping identity, byte range in that mapping).
    fn stored(&self, file: &FileRef) -> Option<(usize, std::ops::Range<usize>)> {
        let (offset, size) = match self.archive.entries[file.entry_index].kind {
            rpf_archive::RpfEntryKind::BinaryFile { file_offset, file_size, uncompressed_size, .. } => {
                (file_offset, if file_size > 0 { file_size } else { uncompressed_size })
            }
            rpf_archive::RpfEntryKind::ResourceFile { file_offset, file_size, .. } => (file_offset, file_size),
            _ => return None,
        };
        let offset = match self.archive.version {
            rpf_archive::RpfVersion::V7 => offset as usize * 512,
            _ => offset as usize,
        };
        let start = self.data.range.start + self.archive.start_offset + offset;
        let end = (start + size as usize).min(self.data.range.end);
        Some((Arc::as_ptr(&self.data.buffer) as usize, start..end))
    }

    pub fn files(&self) -> Vec<FileRef> {
        list_all_files(&self.root).into_iter().cloned().collect()
    }

    /// The entry's bytes as `rage extract` writes them.
    pub fn extract(&self, file: &FileRef, keys: &GtaKeys) -> Result<Vec<u8>> {
        self.archive.extract_entry(self.data.bytes(), &self.archive.entries[file.entry_index], keys.into())
    }
}

/// One entry found by [`walk`]: its archive (kept open for reading later) and its path.
#[derive(Clone)]
pub struct Entry {
    pub archive: Arc<Archive>,
    pub file: FileRef,
    /// The path `rage extract --recursive` writes it at, below the output folder.
    pub path: String,
}

impl Entry {
    pub fn read(&self, keys: &GtaKeys) -> Result<Vec<u8>> {
        self.archive.extract(&self.file, keys)
    }
}

/// Pages in the stored bytes of `entries` in file order, one archive at a
/// time: a hard disk then streams them instead of seeking once per entry when
/// they are read in some other order (by model hash, by name).
pub fn prefetch<'a>(entries: impl IntoIterator<Item = &'a Entry>) {
    let mut spans: Vec<(usize, usize, usize, Arc<Buffer>)> = entries
        .into_iter()
        .filter_map(|e| {
            let (id, r) = e.archive.stored(&e.file)?;
            Some((id, r.start, r.end, e.archive.data.buffer.clone()))
        })
        .collect();
    spans.sort_by_key(|s| (s.0, s.1));
    let mut sum = 0u8;
    for (_, start, end, buffer) in &spans {
        let all: &[u8] = match &**buffer {
            Buffer::Owned(_) => continue,
            Buffer::Mapped(m) => m,
        };
        for i in (*start..*end).step_by(4096).chain(end.checked_sub(1)) {
            // SAFETY: in bounds (ranges are clamped to the mapping); volatile so the touch is not elided.
            sum = sum.wrapping_add(unsafe { std::ptr::read_volatile(all.as_ptr().add(i)) });
        }
    }
    std::hint::black_box(sum);
}

/// Every file of `archive` and its nested archives whose path `want` accepts,
/// in archive order. Nested archives that fail to open are reported to `failed`.
pub fn walk(archive: Arc<Archive>, keys: &GtaKeys, want: &dyn Fn(&str) -> bool, failed: &mut Vec<String>) -> Vec<Entry> {
    let mut out = Vec::new();
    walk_inner(archive, "", keys, want, failed, &mut out, 0);
    out
}

fn walk_inner(archive: Arc<Archive>, prefix: &str, keys: &GtaKeys, want: &dyn Fn(&str) -> bool,
              failed: &mut Vec<String>, out: &mut Vec<Entry>, depth: usize) {
    if depth > 16 {
        return;
    }
    for file in archive.files() {
        let path = if prefix.is_empty() { file.path.clone() } else { format!("{prefix}/{}", file.path) };
        if file.name.to_lowercase().ends_with(".rpf") {
            match archive.open_nested(&file, keys) {
                Ok(nested) => walk_inner(Arc::new(nested), &path, keys, want, failed, out, depth + 1),
                Err(e) => failed.push(format!("failed to open nested {path}: {e}")),
            }
        } else if want(&path) {
            out.push(Entry { archive: archive.clone(), file, path });
        }
    }
}

/// rage-cli's match: `*` matches any run (including `/`); a pattern without `*` is a substring.
pub fn matches_pattern(path: &str, pattern: &str) -> bool {
    if !pattern.contains('*') {
        return path.contains(pattern);
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !path.starts_with(first) {
        return false;
    }
    let mut rest = &path[first.len()..];
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

pub fn any_match(path: &str, patterns: &[String]) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| matches_pattern(path, p))
}

pub fn keys(dir: &Path) -> Result<GtaKeys> {
    GtaKeys::load_from_path(dir)
}

/// Every `.rpf` under `dir` (recursively), sorted as rage's search lists them.
pub fn archives_under(dir: &Path) -> Vec<PathBuf> {
    fn go(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(&p, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    go(dir, &mut out);
    out.sort();
    out
}

/// Sort key reproducing a recursive NTFS directory walk (each folder's entries
/// by upper-cased name, a folder's contents at its place): where two files
/// share a name, the one such a walk meets last wins.
pub fn walk_order(path: &str) -> Vec<String> {
    path.split('/').map(|c| c.to_uppercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_like_rage() {
        assert!(matches_pattern("x64/levels/gta5/props.rpf/a.ydr", "*/props/*.ydr") == false);
        assert!(matches_pattern("x64/levels/gta5/props/x.rpf/a.ydr", "*/props/*.ydr"));
        assert!(matches_pattern("models/cdimages/streamedpeds_players.rpf/player_zero.yft", "*peds*.rpf/*.yft"));
        assert!(matches_pattern("levels/gta5/vehicles.rpf/adder.yft", "*vehicles*.yft"));
        assert!(matches_pattern("a/content.xml", "content.xml"));
        assert!(!matches_pattern("a/b.ytd", "*.ydr"));
    }

    #[test]
    fn walk_order_is_ntfs_like() {
        let mut v = vec!["b/_x.ydr", "b/a.ydr", "B_c/z.ydr", "a.ydr"];
        v.sort_by_key(|p| walk_order(p));
        assert_eq!(v, ["a.ydr", "b/a.ydr", "b/_x.ydr", "B_c/z.ydr"]);
    }
}
