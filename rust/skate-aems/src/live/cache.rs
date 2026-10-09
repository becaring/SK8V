//! The prepared sample cache (`tools/prepare-skate-audio.py`, docs/AEMS.md
//! "Sample cache"): the user's own Skate 3 audio files, unpacked, and every
//! EA-XMA sample decoded once, continuously (`skate-xma`, bit-identical to
//! `xmadec --continuous`).
//!
//! Layout under the cache directory:
//! - `manifest.json`: format marker (`"format": "skatev-audio-cache"`), version.
//! - `raw/<archive>/<file>`: the archives' files (`.csi`, `.abk`, `.bnk`,
//!   `.grain`, `.snr`, ...), exactly as stored.
//! - `pcm/<archive>/<bank stem>_<table index>.xma16`: a bank sample's decode,
//!   big-endian PCM16 (interleaved when stereo), whole 512-sample frames from
//!   the first frame, named by the index the game addresses (`bank +
//!   *(bank + 32)`, header `i` at `table + table[3 + i]`).
//!
//! Samples load lazily and stay cached; a missing or inconsistent decode
//! makes that one sample silent (logged once), never the engine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::be;
use crate::eac::snr::{self, Snr};
use crate::eac::xma::Sound;

pub const FORMAT: &str = "skatev-audio-cache";

/// A bank file and its samples' byte spans.
pub struct BankFile {
    pub archive: String,
    pub stem: String,
    /// `<archive>/<stem>`: the cache's key for the bank's samples.
    key: String,
    pub bytes: Vec<u8>,
    /// `(header offset, end)` per table index.
    pub spans: Vec<(usize, usize)>,
}

impl BankFile {
    pub fn parse(archive: &str, stem: &str, bytes: Vec<u8>) -> BankFile {
        let spans = sample_spans(&bytes);
        BankFile { archive: archive.into(), stem: stem.into(), key: format!("{archive}/{stem}"), bytes, spans }
    }
}

/// The SNR header and blocks of every sample in an `.abk`, by table index.
pub fn sample_spans(b: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let Some(table) = be::get_u32(b, 32).map(|t| t as usize) else { return out };
    if table == 0 {
        return out;
    }
    for i in 0.. {
        let Some(rel) = be::get_u32(b, table + 4 * (3 + i)) else { break };
        let off = table + rel as usize;
        if off + 12 > b.len() {
            break;
        }
        let snr = Snr::parse(&b[off..]);
        if snr.codec != snr::CODEC_XMA || snr.version != 0 {
            break;
        }
        let mut at = off + snr.data;
        let mut seen = 0i64;
        while seen < snr.samples as i64 && at + 8 <= b.len() {
            let blk = snr::block(b, at, 0);
            seen += blk.samples as i64;
            if blk.size == 0 {
                break;
            }
            at += blk.size as usize;
            if blk.tag & 0x80 != 0 {
                break;
            }
        }
        out.push((off, at.min(b.len())));
    }
    out
}

/// The prepared cache: its files on disk and the samples decoded so far.
pub struct Cache {
    /// Decoded sample directory (`<cache>/pcm`).
    pub pcm: PathBuf,
    /// `.csi` files in the game's load order (name, bytes).
    pub csi: Vec<(String, Vec<u8>)>,
    /// Bank file names (`<archive>/<stem>`) on disk.
    pub bank_paths: HashMap<String, PathBuf>,
    /// Samples by bank key, then table index (`Some(None)`: failed once).
    sounds: HashMap<String, Vec<Option<Option<Arc<Sound>>>>>,
    pub problems: Vec<String>,
}

/// The order Skate 3 loads its `.csi` files (the ids the loader assigns
/// follow it).
pub const CSI_ORDER: [&str; 9] = [
    "SK8_AEMS_skateboard.csi",
    "SK8_AEMS_Foley.csi",
    "Sk8_Emitters_Project.csi",
    "SK8_AEMS_rolling.csi",
    "AEMS_TRAFFIC.csi",
    "SK8_AEMS_Crowds.csi",
    "Sk8_AEMS_MoveableObjects.csi",
    "Sk8_moments.csi",
    "AEMS_Calibrate.csi",
];

impl Cache {
    /// Opens a prepared cache.
    pub fn open(dir: &Path) -> Result<Cache, String> {
        let raw = dir.join("raw");
        let audiofiles = raw.join("audiofiles");
        if !audiofiles.is_dir() {
            return Err(format!("{} has no raw/audiofiles (run tools/prepare-skate-audio.py)", dir.display()));
        }
        let manifest = std::fs::read_to_string(dir.join("manifest.json")).map_err(|e| format!("{}: {e}", dir.join("manifest.json").display()))?;
        if !manifest.contains(FORMAT) {
            return Err(format!("{} is not a {FORMAT}", dir.join("manifest.json").display()));
        }
        let mut csi = Vec::new();
        for name in CSI_ORDER {
            let p = audiofiles.join(name);
            let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            csi.push((name.to_string(), bytes));
        }
        let mut bank_paths = HashMap::new();
        for arch in std::fs::read_dir(&raw).map_err(|e| e.to_string())?.flatten() {
            if !arch.path().is_dir() {
                continue;
            }
            let a = arch.file_name().to_string_lossy().into_owned();
            for f in std::fs::read_dir(arch.path()).map_err(|e| e.to_string())?.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("abk")) {
                    let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
                    bank_paths.insert(format!("{a}/{stem}"), p);
                }
            }
        }
        Ok(Cache { pcm: dir.join("pcm"), csi, bank_paths, sounds: HashMap::new(), problems: Vec::new() })
    }

    pub fn bank(&self, key: &str) -> Result<BankFile, String> {
        let p = self.bank_paths.get(key).ok_or_else(|| format!("bank {key} not in the cache"))?;
        let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let (archive, stem) = key.split_once('/').unwrap();
        Ok(BankFile::parse(archive, stem, bytes))
    }

    fn read_sound(&self, bank: &BankFile, index: usize) -> Result<Sound, String> {
        let &(a, b) = bank.spans.get(index).ok_or("no such sample")?;
        let p = self.pcm.join(&bank.archive).join(format!("{}_{index}.xma16", bank.stem));
        let pcm = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        Sound::new(bank.bytes[a..b].to_vec(), &pcm)
    }

    /// Every sample of `bank` that decodes, so the render path never reads
    /// the disk. A sample that does not is left for its first use to report.
    pub fn preload(&mut self, bank: &BankFile) {
        let loaded: Vec<_> = (0..bank.spans.len()).map(|i| self.read_sound(bank, i).ok().map(|s| Some(Arc::new(s)))).collect();
        let slots = self.sounds.entry(bank.key.clone()).or_default();
        for (i, l) in loaded.into_iter().enumerate() {
            if slots.len() <= i {
                slots.push(l);
            } else if slots[i].is_none() {
                slots[i] = l;
            }
        }
    }

    /// Sample `index` of `bank`, decoded (loaded on first use if not preloaded).
    pub fn sound(&mut self, bank: &BankFile, index: usize) -> Option<Arc<Sound>> {
        if let Some(Some(s)) = self.sounds.get(bank.key.as_str()).and_then(|v| v.get(index)) {
            return s.clone();
        }
        let s = match self.read_sound(bank, index) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => {
                self.problems.push(format!("{}/{} sample {index}: {e}", bank.archive, bank.stem));
                None
            }
        };
        let slots = self.sounds.entry(bank.key.clone()).or_default();
        if slots.len() <= index {
            slots.resize(index + 1, None);
        }
        slots[index] = Some(s.clone());
        s
    }
}

/// Classes a bank's records subscribe to (`rec + 4`, a kind-1 import), and
/// every class it imports (child patches create objects of those), by name;
/// read from the file without loading it.
pub fn bank_classes(b: &[u8]) -> (Vec<String>, Vec<String>) {
    let be32 = |o: usize| be::get_u32(b, o).unwrap_or(0) as usize;
    let be16 = |o: usize| be::get_u16(b, o).unwrap_or(0) as usize;
    let cstr = |o: usize| -> String {
        let s = b.get(o..).unwrap_or(&[]);
        let e = s.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&s[..e]).into_owned()
    };
    if b.len() < 96 || &b[..4] != b"ABKC" {
        return (Vec::new(), Vec::new());
    }
    let imps = be32(56);
    let n = be32(imps) as i32;
    let mut by_target: HashMap<usize, String> = HashMap::new();
    let mut imported = Vec::new();
    for k in 0..n.max(0) as usize {
        let e = imps + 4 + 12 * k;
        if e + 12 > b.len() {
            break;
        }
        let (target, rec, kind) = (be32(e), be32(e + 4), b[e + 8]);
        if kind == 1 {
            let name = cstr(rec + 4);
            by_target.insert(target, name.clone());
            if !imported.contains(&name) {
                imported.push(name);
            }
        }
    }
    let mut records = Vec::new();
    let mut rec = be32(28);
    for _ in 0..be16(10) {
        if rec + 60 > b.len() {
            break;
        }
        if let Some(name) = by_target.get(&(rec + 4))
            && !records.contains(name)
        {
            records.push(name.clone());
        }
        rec += 60 + 4 * (b[rec + 39] as usize + b[rec + 36] as usize);
    }
    (records, imported)
}
