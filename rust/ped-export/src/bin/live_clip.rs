//! Owned live clip (gen8 / Legacy): a clip dictionary with one two-frame
//! clip, `skatev_body`, whose animation has a raw-float translation,
//! rotation and scale track for every bone tag any GTA ped skeleton has,
//! plus a marker track. The host plays it on the skater and writes Skate's
//! pose into both frames each tick, so GTA's own animation system (and the
//! Rockstar Editor) carries the pose. Port of SkateGTA-B4's owned clip
//! (docs/DECISIONS.md); format: docs/YCD-OWNED-CLIP.md.
//!
//! usage: live_clip <ped-cache dir> <out dir> <out layout .h>
//! writes <out dir>/skatev_live.ycd and the dlc pack <out dir>/dlc.rpf
//! (dlcpacks:/skatev/, laid out like the NFS `iverson` pack), installed as
//! update/x64/dlcpacks/skatev/dlc.rpf and mounted by the host (pack_loader.h).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use rage_formats::blocks::base::PagesInfo;
use rage_formats::blocks::{Block, BlockId, Graph, Writer};
use rage_formats::rage_joaat;
use rpf_archive::crypto::jenkins_hash;

pub const NAME: &str = "skatev_body";
pub const MARKER_TAG: u16 = 65021;
/// Marker translation, frame 0 and frame 1: values no pose produces (B4's).
pub const MARKER: [[f32; 3]; 2] = [[31337.25, -27182.812, 16180.375], [-31337.75, 27182.312, -16180.875]];
const BUCKETS: usize = 11;

/// One fixed-size structure: bytes plus the u64 pointers patched in at write.
struct Fixed {
    data: Vec<u8>,
    ptrs: Vec<(usize, BlockId)>,
}
impl Fixed {
    fn new(len: usize) -> Self { Self { data: vec![0; len], ptrs: Vec::new() } }
    fn u8(mut self, at: usize, v: u8) -> Self { self.data[at] = v; self }
    fn u16(mut self, at: usize, v: u16) -> Self { self.data[at..at + 2].copy_from_slice(&v.to_le_bytes()); self }
    fn u32(mut self, at: usize, v: u32) -> Self { self.data[at..at + 4].copy_from_slice(&v.to_le_bytes()); self }
    fn f32(self, at: usize, v: f32) -> Self { self.u32(at, v.to_bits()) }
    fn ptr(mut self, at: usize, id: BlockId) -> Self { self.ptrs.push((at, id)); self }
}
impl Block for Fixed {
    fn length(&self) -> usize { self.data.len() }
    fn references(&self, _g: &Graph) -> Vec<BlockId> { self.ptrs.iter().map(|p| p.1).collect() }
    fn write(&self, w: &mut Writer, g: &Graph) -> Result<()> {
        let mut d = self.data.clone();
        for &(at, id) in &self.ptrs { d[at..at + 8].copy_from_slice(&g.position(id).to_le_bytes()) }
        w.bytes(&d);
        Ok(())
    }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

fn pointer_array(g: &mut Graph, items: &[Option<BlockId>]) -> BlockId {
    let mut f = Fixed::new(8 * items.len());
    for (i, id) in items.iter().enumerate() {
        if let Some(id) = id { f = f.ptr(8 * i, *id) }
    }
    g.add(f)
}

/// An 11-bucket hash map (CodeWalker CreateClipsMap/CreateAnimationsMap):
/// 32-byte entries {hash, pointer, next}, collisions chained in insertion order.
fn hash_map(g: &mut Graph, entries: &[(u32, BlockId)]) -> BlockId {
    let mut slots: Vec<Option<BlockId>> = vec![None; BUCKETS];
    for (b, slot) in slots.iter_mut().enumerate() {
        let mut next = None;
        for &(hash, target) in entries.iter().rev().filter(|e| e.0 as usize % BUCKETS == b) {
            let mut e = Fixed::new(32).u32(0, hash).ptr(8, target);
            if let Some(n) = next { e = e.ptr(0x10, n) }
            next = Some(g.add(e));
        }
        *slot = next;
    }
    pointer_array(g, &slots)
}

/// A bone track: tag, track kind (0 position, 1 rotation, 2 scale), values per frame.
pub struct Track {
    pub tag: u16,
    pub kind: u8,
    pub frames: [Vec<f32>; 2],
}

/// The Sequence block (32-byte header + Data, RawFloat channels only).
pub fn sequence(tracks: &[Track]) -> Vec<u8> {
    let c: usize = tracks.iter().map(|t| t.frames[0].len()).sum();
    let padded = (c + 3) & !3;
    let data_len = 8 * c + 18 + 2 * padded;
    let mut d = Fixed::new(32 + data_len)
        .u32(4, data_len as u32)
        .u32(0x10, (32 + data_len) as u32) // root-motion refs: none
        .u16(0x16, 2)
        .u16(0x18, (4 * c) as u16)
        .u8(0x1E, 255)
        .data;
    let mut at = 32;
    for f in 0..2 {
        for t in tracks {
            for v in &t.frames[f] {
                d[at..at + 4].copy_from_slice(&v.to_le_bytes());
                at += 4;
            }
        }
    }
    let mut put = |at: &mut usize, v: u16| { d[*at..*at + 2].copy_from_slice(&v.to_le_bytes()); *at += 2; };
    for ty in 0..9 { put(&mut at, if ty == 3 { c as u16 } else { 0 }) }
    for (i, t) in tracks.iter().enumerate() {
        for ch in 0..t.frames[0].len() { put(&mut at, (ch + (i << 2)) as u16) }
    }
    for _ in c..padded { put(&mut at, (tracks.len() << 2) as u16) }
    assert_eq!(at, d.len());
    d
}

/// The whole `.ycd` for one clip over `tracks` (sorted by kind, then tag).
pub fn build(tracks: &[Track]) -> Result<Vec<u8>> {
    let mut g = Graph::new();
    let hash = rage_joaat(NAME);
    let seq = sequence(tracks);
    let seq_len = seq.len();
    let seq_id = g.add(Fixed { data: seq, ptrs: Vec::new() });
    let seqs = pointer_array(&mut g, &[Some(seq_id)]);
    let mut ids = Fixed::new(4 * tracks.len());
    for (i, t) in tracks.iter().enumerate() {
        ids = ids.u16(4 * i, t.tag).u8(4 * i + 3, t.kind);
    }
    let ids = g.add(ids);
    let n = u16::try_from(tracks.len()).context("too many tracks")?;
    let anim = g.add(
        Fixed::new(96)
            .u32(4, 1)
            .u8(0x11, 1)
            .u16(0x14, 2) // frames
            .u16(0x16, 32) // sequence frame limit
            .f32(0x18, 1.0 / 30.0)
            .u32(0x1C, hash.wrapping_add(1))
            .u32(0x38, seq_len as u32)
            .u32(0x3C, 2) // usage: map entry + clip
            .ptr(0x40, seqs).u16(0x48, 1).u16(0x4A, 1)
            .ptr(0x50, ids).u16(0x58, n).u16(0x5A, n),
    );
    let name = format!("pack:/{NAME}.clip");
    let mut name_block = Fixed::new(name.len() + 1);
    name_block.data[..name.len()].copy_from_slice(name.as_bytes());
    let name_id = g.add(name_block);
    let tags = g.add(Fixed::new(32));
    let props_buckets = pointer_array(&mut g, &[None; BUCKETS]);
    let props = g.add(Fixed::new(16).ptr(0, props_buckets).u16(8, BUCKETS as u16).u32(0xC, 0x0100_0000));
    let clip = g.add(
        Fixed::new(112)
            .u32(4, 1)
            .u32(0x10, 1) // ClipType.Animation
            .ptr(0x18, name_id).u16(0x20, name.len() as u16).u16(0x22, name.len() as u16 + 1)
            .u32(0x28, 0x5000_0000)
            .ptr(0x38, tags)
            .ptr(0x40, props)
            .u32(0x48, 1)
            .ptr(0x50, anim)
            .f32(0x5C, 1.0 / 30.0)
            .f32(0x60, 1.0),
    );
    let anim_buckets = hash_map(&mut g, &[(hash, anim)]);
    let anim_map = g.add(
        Fixed::new(48).u32(4, 1).ptr(0x18, anim_buckets).u16(0x20, BUCKETS as u16).u16(0x22, 1)
            .u32(0x24, 0x0100_0000).u32(0x28, 1),
    );
    let clip_buckets = hash_map(&mut g, &[(hash, clip)]);
    let pages = g.add(PagesInfo::default());
    // ClipDictionary: the root, so added last but laid out first.
    let root = Fixed::new(64)
        .u32(4, 1) // pgBase: vft 0, 1, pages-info pointer
        .ptr(8, pages)
        .ptr(0x18, anim_map)
        .u32(0x20, 0x101)
        .ptr(0x28, clip_buckets).u16(0x30, BUCKETS as u16).u16(0x32, 1)
        .u32(0x34, 0x0100_0000);
    let root = g.add(root);
    g.build(root, pages, 46)
}

/// Every bone tag in the exported ped skeletons with its rest pose (first
/// skeleton that has it, the protagonists first), then the clip's tracks.
fn tracks(ped_cache: &Path) -> Result<Vec<Track>> {
    let mut files: Vec<_> = fs::read_dir(ped_cache)?.flatten().map(|e| e.path().join("skeleton.json")).filter(|p| p.exists()).collect();
    files.sort_by_key(|p| (!p.to_string_lossy().contains("player_"), p.clone()));
    let mut rest: BTreeMap<u16, [Vec<f32>; 3]> = BTreeMap::new();
    for f in files {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&f)?)?;
        for b in v["bones"].as_array().context("bones")? {
            let tag = b["tag"].as_u64().context("tag")? as u16;
            let get = |k: &str| b[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect::<Vec<_>>();
            rest.entry(tag).or_insert_with(|| [get("t"), get("r"), get("s")]);
        }
    }
    ensure!(!rest.contains_key(&MARKER_TAG), "a skeleton uses the marker tag {MARKER_TAG}");
    let mut out: Vec<Track> = Vec::new();
    for kind in 0..3u8 {
        for (&tag, r) in &rest {
            let v = r[kind as usize].clone();
            out.push(Track { tag, kind, frames: [v.clone(), v] });
        }
        if kind == 0 {
            out.push(Track { tag: MARKER_TAG, kind: 0, frames: [MARKER[0].to_vec(), MARKER[1].to_vec()] });
        }
    }
    out.sort_by_key(|t| (t.kind, t.tag));
    Ok(out)
}

/// Host header: the channel index of each tag's t/q/s and the marker's.
fn header(tracks: &[Track], ycd_sha: &str) -> String {
    let mut k = 0;
    let mut at: BTreeMap<u16, [usize; 3]> = BTreeMap::new();
    let mut marker = 0;
    for t in tracks {
        if t.tag == MARKER_TAG { marker = k } else { at.entry(t.tag).or_default()[t.kind as usize] = k }
        k += t.frames[0].len();
    }
    let mut s = format!(
        "#pragma once\n// Generated by rust/ped-export/src/bin/live_clip.rs. Do not edit. skatev_live.ycd sha256 {ycd_sha}\n\
         // float(frame f, channel k) = marker frame-0 x + 4 * (k - kMarkerChannel) + f * 4 * kChannels bytes.\n\
         #include <cstdint>\nnamespace liveclip {{\n\
         inline constexpr float kMarker[2][3] = {{{{{}f, {}f, {}f}}, {{{}f, {}f, {}f}}}};\n\
         inline constexpr int kChannels = {k};\ninline constexpr int kMarkerChannel = {marker};\n\
         struct Track {{ std::uint16_t tag, t, q, s; }}; // channel index of translation xyz, rotation xyzw, scale xyz\n\
         inline constexpr Track kTracks[{}] = {{\n",
        MARKER[0][0], MARKER[0][1], MARKER[0][2], MARKER[1][0], MARKER[1][1], MARKER[1][2], at.len()
    );
    for (tag, c) in &at {
        s += &format!("    {{{tag}, {}, {}, {}}},\n", c[0], c[1], c[2]);
    }
    s + "};\n} // namespace liveclip\n"
}

const SETUP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<SSetupData>
  <deviceName>dlc_skatev</deviceName>
  <datFile>content.xml</datFile>
  <timeStamp>10/05/2026 00:00:00</timeStamp>
  <nameHash>skatev</nameHash>
  <contentChangeSetGroups>
    <Item>
      <NameHash>GROUP_STARTUP</NameHash>
      <ContentChangeSets>
        <Item>SKATEV_AUTOGEN</Item>
      </ContentChangeSets>
    </Item>
  </contentChangeSetGroups>
  <type>EXTRACONTENT_COMPAT_PACK</type>
  <order value="13" />
</SSetupData>
"#;

const CONTENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<CDataFileMgr__ContentsOfDataFileXml>
  <disabledFiles />
  <includedXmlFiles />
  <includedDataFiles />
  <dataFiles>
    <Item>
      <filename>dlc_skatev:/%PLATFORM%/anim/skatev_anim.rpf</filename>
      <fileType>RPF_FILE</fileType>
      <overlay value="false" />
      <disabled value="true" />
      <persistent value="true" />
    </Item>{WEAPON_FILES}
  </dataFiles>
  <contentChangeSets>
    <Item>
      <changeSetName>SKATEV_AUTOGEN</changeSetName>
      <mapChangeSetData />
      <filesToInvalidate />
      <filesToDisable />
      <filesToEnable>
        <Item>dlc_skatev:/%PLATFORM%/anim/skatev_anim.rpf</Item>{WEAPON_ENABLE}
      </filesToEnable>
      <txdToLoad />
      <txdToUnload />
      <residentResources />
      <unregisterResources />
    </Item>
  </contentChangeSets>
  <patchFiles />
</CDataFileMgr__ContentsOfDataFileXml>
"#;

/// Weapon data files of the pack (tools/build-board-weapon.py): file name and
/// GTA data file type, loaded at startup with the clip.
const WEAPON_FILES: [(&str, &str); 2] =
    [("weapons_skatev.meta", "WEAPONINFO_FILE"), ("weaponanimations_skatev.meta", "WEAPON_ANIMATIONS_FILE")];

fn content(weapon: bool) -> String {
    let (mut files, mut enable) = (String::new(), String::new());
    for (name, kind) in WEAPON_FILES.iter().filter(|_| weapon) {
        files += &format!("
    <Item>
      <filename>dlc_skatev:/common/data/ai/{name}</filename>
      <fileType>{kind}</fileType>
      <overlay value=\"true\" />
      <disabled value=\"true\" />
      <persistent value=\"true\" />
    </Item>");
        enable += &format!("
        <Item>dlc_skatev:/common/data/ai/{name}</Item>");
    }
    if weapon {
        files += "
    <Item>
      <filename>dlc_skatev:/common/data/dlctext.meta</filename>
      <fileType>TEXTFILE_METAFILE</fileType>
      <overlay value=\"false\" />
      <disabled value=\"true\" />
      <persistent value=\"true\" />
    </Item>";
        enable += "
        <Item>dlc_skatev:/common/data/dlctext.meta</Item>";
    }
    CONTENT.replace("{WEAPON_FILES}", &files).replace("{WEAPON_ENABLE}", &enable)
}

/// Text the pack adds, as the retail packs do (mpluxe): `common/data/dlctext.meta` says the
/// pack has a global text file, GTA loads it from `x64/data/lang/<language>dlc.rpf`.
/// ponytail: English only; other languages show the raw label until copies are packed.
const TEXT: [(&str, &str); 1] = [("WT_SKATEBOARD", "Skateboard")];

const DLCTEXT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<CExtraTextMetaFile>
	<hasGlobalTextFile value="true"/>
	<hasAdditionalText value="false"/>
	<isTitleUpdate value="false"/>
</CExtraTextMetaFile>
"#;

/// GXT2 as in retail `global.gxt2`: "2TXG", count, (label hash, string offset) sorted by
/// hash, "2TXG", file size, then the NUL-terminated UTF-8 strings.
fn gxt2(text: &[(&str, &str)]) -> Vec<u8> {
    let mut rows: Vec<(u32, &str)> = text.iter().map(|(k, v)| (jenkins_hash(k), *v)).collect();
    rows.sort();
    let mut strings = Vec::new();
    let base = 16 + 8 * rows.len();
    let mut out = b"2TXG".to_vec();
    out.extend((rows.len() as u32).to_le_bytes());
    for (hash, s) in &rows {
        out.extend(hash.to_le_bytes());
        out.extend(((base + strings.len()) as u32).to_le_bytes());
        strings.extend(s.as_bytes());
        strings.push(0);
    }
    out.extend(b"2TXG");
    out.extend(((base + strings.len()) as u32).to_le_bytes());
    out.extend(strings);
    out
}

/// The dlc pack: setup2.xml, content.xml, x64/anim/skatev_anim.rpf holding the ycd, and
/// WEAPON_SKATEBOARD's meta and name text when `weapon` holds it. OPEN (clear) archives; the
/// host lets GTA read those (pack_loader.h).
fn dlc(ycd: &[u8], weapon: &[(&str, Vec<u8>)]) -> Result<Vec<u8>> {
    use rpf_archive::{RpfBuilder, RpfEncryption};
    let mut inner = RpfBuilder::new(RpfEncryption::Open);
    inner.add_file("skatev_live.ycd", ycd.to_vec());
    let mut outer = RpfBuilder::new(RpfEncryption::Open);
    outer.add_file("setup2.xml", SETUP.as_bytes().to_vec());
    outer.add_file("content.xml", content(!weapon.is_empty()).into_bytes());
    for (name, bytes) in weapon {
        outer.add_file(&format!("common/data/ai/{name}"), bytes.clone());
    }
    if !weapon.is_empty() {
        let mut lang = RpfBuilder::new(RpfEncryption::Open);
        lang.add_file("global.gxt2", gxt2(&TEXT));
        outer.add_file("common/data/dlctext.meta", DLCTEXT.as_bytes().to_vec());
        outer.add_file("x64/data/lang/americandlc.rpf", stored(lang.build(None)?)?);
    }
    outer.add_file("x64/anim/skatev_anim.rpf", stored(inner.build(None)?)?);
    stored(outer.build(None)?)
}

/// The builder writes a stored binary file's compressed size as its size, but
/// GTA treats any non-zero compressed size as deflated and refuses the pack
/// (ERR_FIL_PACK_3). Stored must be 0, as in the NFS `iverson` pack.
fn stored(mut rpf: Vec<u8>) -> Result<Vec<u8>> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    ensure!(word(&rpf, 0) == 0x5250_4637 && word(&rpf, 12) == 0x4e45_504f, "not an OPEN RPF7");
    for i in 0..word(&rpf, 4) as usize {
        let e = 16 + 16 * i;
        let directory = word(&rpf, e + 4) == 0x7fff_ff00;
        let resource = rpf[e + 7] & 0x80 != 0;
        if !directory && !resource {
            rpf[e + 2..e + 5].fill(0);
        }
    }
    Ok(rpf)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    ensure!(a.len() == 4, "usage: live_clip <ped-cache dir> <out dir> <out layout .h>");
    let tracks = tracks(Path::new(&a[1]))?;
    let ycd = build(&tracks)?;
    let out = Path::new(&a[2]);
    fs::create_dir_all(out)?;
    let ycd_path = out.join("skatev_live.ycd");
    fs::write(&ycd_path, &ycd)?;
    // WEAPON_SKATEBOARD, when tools/build-board-weapon.py wrote it beside the pack.
    let weapon: Vec<(&str, Vec<u8>)> = WEAPON_FILES.iter().filter_map(|(n, _)| fs::read(out.join("weapon").join(n)).ok().map(|b| (*n, b))).collect();
    ensure!(weapon.is_empty() || weapon.len() == WEAPON_FILES.len(), "partial weapon meta in {}", out.join("weapon").display());
    fs::write(out.join("dlc.rpf"), dlc(&ycd, &weapon)?)?;
    if !weapon.is_empty() { eprintln!("WEAPON_SKATEBOARD meta packed"); }
    let sha = std::process::Command::new("certutil").args(["-hashfile".as_ref(), ycd_path.as_os_str(), "SHA256".as_ref()]).output()
        .ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.lines().nth(1).map(|l| l.trim().to_string()))
        .unwrap_or_default();
    fs::write(&a[3], header(&tracks, &sha))?;
    eprintln!("{}: {} tracks, {} bytes; dlc.rpf beside it", ycd_path.display(), tracks.len(), ycd.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // docs/YCD-OWNED-CLIP.md section 5 worked example: pos(3), rot(4), marker(3).
    #[test]
    fn sequence_matches_the_spec_example() {
        let t = |tag, kind, n| Track { tag, kind, frames: [vec![1.0; n], vec![2.0; n]] };
        let s = sequence(&[t(1, 0, 3), t(MARKER_TAG, 0, 3), t(1, 1, 4)]);
        assert_eq!(s.len(), 154);
        assert_eq!(u32::from_le_bytes(s[4..8].try_into().unwrap()), 122);
        assert_eq!(u32::from_le_bytes(s[0x10..0x14].try_into().unwrap()), 154);
        assert_eq!(u16::from_le_bytes(s[0x18..0x1A].try_into().unwrap()), 40);
        assert_eq!(f32::from_le_bytes(s[32 + 40..32 + 44].try_into().unwrap()), 2.0); // frame 1 row
        let items: Vec<u16> = s[32 + 80 + 18..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(items, [0, 1, 2, 4, 5, 6, 8, 9, 10, 11, 12, 12]);
    }

    #[test]
    fn weapon_files_are_listed_and_enabled_only_when_packed() {
        let c = content(true);
        for (name, kind) in WEAPON_FILES {
            let path = format!("dlc_skatev:/common/data/ai/{name}");
            assert_eq!(c.matches(&path).count(), 2, "{name}: data file + filesToEnable");
            assert!(c.contains(&format!("<fileType>{kind}</fileType>")));
        }
        assert!(!content(false).contains("common/data/ai") && !content(false).contains('{'));
        assert_eq!(c.matches("dlc_skatev:/common/data/dlctext.meta").count(), 2);
    }

    #[test]
    fn gxt2_matches_the_retail_layout() {
        assert_eq!(jenkins_hash("WEAPON_SKATEBOARD"), 0x38A8_F85C);
        let g = gxt2(&[("B", "two"), ("A", "one")]);
        let word = |at: usize| u32::from_le_bytes(g[at..at + 4].try_into().unwrap());
        assert_eq!((&g[..4], word(4), &g[24..28], word(28) as usize), (&b"2TXG"[..], 2, &b"2TXG"[..], g.len()));
        assert!(word(8) < word(16), "rows sorted by hash");
        let at = |row: usize| { let o = word(12 + 8 * row) as usize; &g[o..o + g[o..].iter().position(|&b| b == 0).unwrap()] };
        let first = if jenkins_hash("A") < jenkins_hash("B") { "one" } else { "two" };
        assert_eq!(at(0), first.as_bytes());
    }

    #[test]
    fn builds_a_resource() {
        let t = |tag, kind, n| Track { tag, kind, frames: [vec![0.0; n], vec![0.0; n]] };
        let ycd = build(&[t(0, 0, 3), t(MARKER_TAG, 0, 3), t(0, 1, 4), t(0, 2, 3)]).unwrap();
        assert_eq!(&ycd[..4], b"RSC7");
        assert_eq!(u32::from_le_bytes(ycd[4..8].try_into().unwrap()), 46);
    }
}
