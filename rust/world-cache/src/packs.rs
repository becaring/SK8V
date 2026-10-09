//! Which archives a patch DLC pack actually mounts.
//!
//! A pack's `dlc.rpf` can carry map archives the game never loads: an archive
//! is live only when a content change set that `setup2.xml` runs lists it in
//! `filesToEnable` (`content.xml`). patchday1ng's `platform:/patch_1/...`
//! overlays and patchday2ng/27ng's `fwy_04.rpf` are shipped but never enabled;
//! applying them anyway replaced La Mesa freeway collision with a 48-triangle
//! stub (the hole found by the material-recovery collision audit).
//!
//! Layer directories hold the pack's `content.xml` and `setup2.xml` beside the
//! extracted archives. A layer without them (`update.rpf`) is applied whole.
use std::collections::HashSet;
use std::path::Path;

/// Top-level archive paths (`x64/levels/gta5/.../name.rpf`, lower case) the
/// pack enables, or `None` when the layer is not a DLC pack.
pub fn enabled(layer: &Path) -> Option<HashSet<String>> {
    let content = std::fs::read_to_string(layer.join("content.xml")).ok()?;
    let setup = std::fs::read_to_string(layer.join("setup2.xml")).ok()?;
    Some(parse(&content, &setup))
}

fn parse(content: &str, setup: &str) -> HashSet<String> {
    let run: HashSet<&str> = tag_texts(setup, "Item").into_iter().collect();
    let mut out = HashSet::new();
    for set in content.split("<changeSetName>").skip(1) {
        let name = set.split("</changeSetName>").next().unwrap_or("").trim();
        if !run.contains(name) {
            continue;
        }
        let Some(files) = set
            .split("<filesToEnable>")
            .nth(1)
            .and_then(|f| f.split("</filesToEnable>").next())
        else {
            continue;
        };
        for item in tag_texts(&strip_comments(files), "Item") {
            let path = item.split_once(":/").map_or(item, |(_, p)| p);
            out.insert(path.replace("%PLATFORM%", "x64").to_ascii_lowercase());
        }
    }
    out
}

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find("-->").map_or("", |j| &rest[i + j + 3..]);
    }
    out.push_str(rest);
    out
}

fn tag_texts<'a>(s: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    s.split(open.as_str())
        .skip(1)
        .filter_map(|t| t.split(close.as_str()).next())
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.contains('<'))
        .collect()
}

/// Whether `file` (somewhere under `layer`) is inside an archive the pack
/// mounts. Files outside any archive are never streamed from a pack.
///
/// `update.rpf`'s `dlc_patch/mp*/` patches multiplayer packs whose map
/// archives only `GROUP_MAP` change sets enable, which run on entering
/// Online (`ON_ENTER_MP`), never in Story Mode. Applied anyway, mpheist's
/// `hei_vb_*` copies doubled Vespucci's collision (collision audit,
/// 2026-10-05). The few `mp*` archives enabled at startup are prop and
/// vehicle assets, not map placements.
pub fn allows(layer: &Path, enabled: &Option<HashSet<String>>, file: &Path) -> bool {
    let rel = file.strip_prefix(layer).unwrap_or(file);
    let rel = rel.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if rel.contains("dlc_patch/mp") {
        return false;
    }
    let Some(set) = enabled else { return true };
    match rel.find(".rpf/") {
        Some(i) => set.contains(&rel[..i + 4]),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT: &str = r#"<dataFiles><Item><filename>platform:/patch_1/levels/gta5/_citye/downtown_01/fwy_04.rpf</filename></Item></dataFiles>
<contentChangeSets>
 <Item><changeSetName>CCS_MAP</changeSetName><filesToEnable>
  <Item>dlc_patchDay2NG:/%PLATFORM%/levels/gta5/_citye/downtown_01/DT1_15.rpf</Item>
  <!-- <Item>dlc_patchDay2NG:/%PLATFORM%/levels/gta5/_citye/downtown_01/dt1_16.rpf</Item> -->
 </filesToEnable></Item>
 <Item><changeSetName>CCS_UNUSED</changeSetName><filesToEnable>
  <Item>dlc_patchDay2NG:/%PLATFORM%/levels/gta5/_citye/downtown_01/fwy_04.rpf</Item>
 </filesToEnable></Item>
</contentChangeSets>"#;
    const SETUP: &str = "<contentChangeSetGroups><Item><NameHash>GROUP_UPDATE_STREAMING</NameHash><ContentChangeSets><Item>CCS_MAP</Item></ContentChangeSets></Item></contentChangeSetGroups>";

    #[test]
    fn only_archives_of_running_change_sets_are_live() {
        let set = Some(parse(CONTENT, SETUP));
        let layer = Path::new("L");
        let live = Path::new("L/x64/levels/gta5/_citye/downtown_01/dt1_15.rpf/dt1_15_0.ybn");
        let dead = Path::new("L/x64/levels/gta5/_citye/downtown_01/fwy_04.rpf/fwy_04_0.ybn");
        let commented = Path::new("L/x64/levels/gta5/_citye/downtown_01/dt1_16.rpf/a.ybn");
        assert!(allows(layer, &set, live));
        assert!(!allows(layer, &set, dead));
        assert!(!allows(layer, &set, commented));
        assert!(allows(layer, &None, dead));
        let mp = Path::new("L/dlc_patch/mpheist/x64/levels/gta5/_cityw/venice_01/vb_34.rpf/hei_vb_34_1.ybn");
        let sp = Path::new("L/dlc_patch/patchday10ng/x64/levels/gta5/a.rpf/a.ybn");
        assert!(!allows(layer, &None, mp));
        assert!(allows(layer, &None, sp));
    }
}
