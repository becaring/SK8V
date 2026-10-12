//! Preparing the board library with the bundled Skate 3 converter.
//!
//! The converter carries Skate 3 Rust Engine's own customiser tools. Two of
//! them run as converter tasks: the catalog indexer reads Create-a-Skater's
//! catalog from the player's disc, and the library builder converts parts
//! into meshes and textures. The library builder converts every model of the
//! catalog it is given, so it gets the catalog's skateboard parts only, plus
//! the default skater's parts its rig reference needs.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{LIBRARY_DIR, Part};

/// The converter tasks (paths inside the converter's bundled tools).
pub const CATALOG_TASK: &str = "tools/asset_pipeline/customisation_catalog.py";
pub const LIBRARY_TASK: &str = "tools/asset_pipeline/customisation_library.py";

/// The default skater's parts: `components` of
/// `tools/default_skater_retail_manifest.json` in the converter's tools.
/// The library builder takes its rig reference from these.
pub const DEFAULT_SKATER: [(&str, &str); 10] = [
    ("Arm", "af71f8ae8ef28d10"),
    ("Feet", "85b73da337acc9ce"),
    ("Hair", "bab72a34fa64163b"),
    ("Organ", "b278a4bfb3d6c4bc"),
    ("OuterTorso", "a3a732c5dd7e7e2c"),
    ("Pants", "d92b14a037b78985"),
    ("Rostral", "9461df2eb3d482dc"),
    ("SkateBoard", "a841a65d5c4dd593"),
    ("SkateTruck", "c6f01c93097b7fb5"),
    ("SkateWheel", "87d0fa0ea0366b23"),
];

/// Where the catalog indexer writes (outside the library folder: it never
/// replaces files it wrote before).
pub fn catalog_dir(assets: &Path) -> PathBuf {
    assets.join("private").join("boards-catalog")
}

pub fn library_dir(assets: &Path) -> PathBuf {
    assets.join(LIBRARY_DIR)
}

/// Arguments for the catalog task.
pub fn catalog_args(game: &Path, assets: &Path) -> Vec<String> {
    vec![
        "--game-root".into(),
        game.display().to_string(),
        "--output".into(),
        catalog_dir(assets).display().to_string(),
    ]
}

/// The library task's configuration file contents.
pub fn library_config(game: &Path, assets: &Path) -> Value {
    json!({
        "assets": assets.display().to_string(),
        "game_root": game.display().to_string(),
        "directory": library_dir(assets).display().to_string(),
        "library_index": "library.json",
    })
}

/// The skateboard part of a full catalog: every deck, truck and wheel, plus
/// the default skater's other parts, and nothing else.
pub fn board_catalog(full: &Value) -> Result<Value, String> {
    let mut catalog = full.clone();
    let components = catalog["components"]
        .as_array_mut()
        .ok_or("the Create-a-Skater catalog has no components")?;
    let mut kept = Vec::new();
    for mut component in components.drain(..) {
        let slot = component["slot"].as_str().unwrap_or("").to_owned();
        if Part::ALL.iter().any(|part| part.slot() == slot) {
            kept.push(component);
            continue;
        }
        let Some((_, asset)) = DEFAULT_SKATER.iter().find(|(s, _)| *s == slot) else {
            continue;
        };
        let models = component["models"].as_array_mut().ok_or("a catalog slot has no models")?;
        models.retain(|model| model["id"].as_str() == Some(asset));
        if models.len() != 1 {
            return Err(format!("the catalog lacks the default skater's {slot}"));
        }
        kept.push(component);
    }
    for (slot, asset) in DEFAULT_SKATER {
        let present = kept.iter().any(|c| {
            c["slot"] == slot
                && c["models"]
                    .as_array()
                    .is_some_and(|models| models.iter().any(|m| m["id"] == asset))
        });
        if !present {
            return Err(format!("the catalog lacks the default skater's {slot}"));
        }
    }
    *components = kept;
    Ok(catalog)
}

/// Writes the library builder's inputs: the board catalog and the native
/// database beside it.
pub fn stage_library(assets: &Path) -> Result<PathBuf, String> {
    let catalog = catalog_dir(assets);
    let library = library_dir(assets);
    std::fs::create_dir_all(&library).map_err(|error| format!("cannot create {}: {error}", library.display()))?;
    let read = |name: &str| {
        let path = catalog.join(name);
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))
    };
    let full: Value = serde_json::from_slice(&read("catalog.json")?).map_err(|error| error.to_string())?;
    let boards = board_catalog(&full)?;
    let write = |name: &str, data: &[u8]| {
        let path = library.join(name);
        std::fs::write(&path, data).map_err(|error| format!("cannot write {}: {error}", path.display()))
    };
    write("catalog.json", &serde_json::to_vec(&boards).map_err(|error| error.to_string())?)?;
    write("native.json", &read("native.json")?)?;
    Ok(library.join("config.json"))
}

/// Writes the library task's configuration; returns its path.
pub fn write_library_config(game: &Path, assets: &Path) -> Result<PathBuf, String> {
    let path = stage_library(assets)?;
    let data = serde_json::to_vec_pretty(&library_config(game, assets)).map_err(|error| error.to_string())?;
    std::fs::write(&path, data).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

/// Whether `game` is an extracted Skate 3 folder with the Create-a-Skater
/// archive the catalog comes from.
pub fn is_game_folder(game: &Path) -> bool {
    game.join("data").join("content").join("createacharacter.big").is_file()
}

/// Prepares the board library with `converter` (the bundled Skate 3
/// converter) from the extracted game at `game` into the Skate 3 data folder
/// `assets`. Progress lines go to `echo`. Parts converted by an earlier,
/// interrupted run are reused.
pub fn prepare(converter: &Path, game: &Path, assets: &Path, mut echo: impl FnMut(&str)) -> Result<usize, String> {
    if !is_game_folder(game) {
        return Err(format!(
            "{} has no data/content/createacharacter.big. Keep the Skate 3 data folder beside default.xex.",
            game.display()
        ));
    }
    echo("Reading Skate 3's skateboard catalog");
    let mut args = vec!["--task".to_owned(), CATALOG_TASK.to_owned()];
    args.extend(catalog_args(game, assets));
    run(converter, &args, &mut echo)?;
    let config = write_library_config(game, assets)?;
    echo("Converting decks, trucks and wheels (this takes a few minutes)");
    run(
        converter,
        &["--task".to_owned(), LIBRARY_TASK.to_owned(), config.display().to_string()],
        &mut echo,
    )?;
    let library = crate::Library::load(assets)?;
    let count: usize = Part::ALL.iter().map(|part| library.items(*part).len()).sum();
    if library.items(Part::Deck).is_empty() {
        return Err("the converter wrote a board library without decks".into());
    }
    echo(&format!("Skateboard catalog ready: {count} parts"));
    Ok(count)
}

/// Where the character library lives inside the Skate 3 data folder: every
/// Create-a-Skater part but the board's, for the GTA V skater.
pub const CHARACTER_DIR: &str = "private/character";

/// The character part of a full catalog: every slot but the board's.
/// The default skater's deck, trucks and wheels, as the converter's own
/// default skater (built into it) names them: it looks each one up in the
/// catalog before anything else, and stops when one is missing.
const DEFAULT_BOARD_ASSETS: [&str; 3] = ["a841a65d5c4dd593", "c6f01c93097b7fb5", "87d0fa0ea0366b23"];


/// The default skater's parts `catalog` has not got, as "slot asset".
pub fn default_skater_missing(catalog: &Value) -> Vec<String> {
    DEFAULT_SKATER
        .iter()
        .filter(|(slot, asset)| {
            !catalog["components"].as_array().into_iter().flatten().any(|component| {
                component["slot"].as_str() == Some(slot)
                    && component["models"].as_array().into_iter().flatten().any(|m| m["id"].as_str() == Some(asset))
            })
        })
        .map(|(slot, asset)| format!("{slot} {asset}"))
        .collect()
}

pub fn character_catalog(full: &Value) -> Result<Value, String> {
    let mut catalog = full.clone();
    let components = catalog["components"]
        .as_array_mut()
        .ok_or("the Create-a-Skater catalog has no components")?;
    // The board's parts are converted on their own (the board library):
    // here only the default skater's three stay, which the converter needs.
    for component in components.iter_mut() {
        let slot = component["slot"].as_str().unwrap_or("");
        if !Part::ALL.iter().any(|part| part.slot() == slot) {
            continue;
        }
        if let Some(models) = component["models"].as_array_mut() {
            let named = |m: &Value| m["id"].as_str().is_some_and(|id| DEFAULT_BOARD_ASSETS.contains(&id));
            // A catalog without them keeps the slot whole rather than lose
            // what the converter looks for.
            if models.iter().any(named) {
                models.retain(named);
            }
        }
    }
    let character = components.iter().any(|component| {
        let slot = component["slot"].as_str().unwrap_or("");
        !Part::ALL.iter().any(|part| part.slot() == slot)
    });
    if !character {
        return Err("the Create-a-Skater catalog has no character parts".into());
    }
    Ok(catalog)
}

/// How many parts (entries with a slot and an asset id) `value` lists.
fn count_parts(value: &Value) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| usize::from(item.get("slot").is_some() && item.get("asset_id").is_some()) + count_parts(item))
            .sum(),
        Value::Object(map) => map.values().map(count_parts).sum(),
        _ => 0,
    }
}

/// Removes, anywhere in `value`, list entries that name a part (a `slot`
/// with an `asset_id`) the parts `catalog` does not have: the board's
/// parts, left out of it, and any the game lists but does not ship.
/// The parts (slot, asset id) `value` names that `catalog` has not got.
fn unknown_parts(value: &Value, catalog: &Value) -> Vec<(String, String)> {
    let mut trimmed = value.clone();
    drop_missing_parts(&mut trimmed, catalog);
    let mut all = Vec::new();
    collect_parts(value, &mut all);
    let mut kept = Vec::new();
    collect_parts(&trimmed, &mut kept);
    for k in &kept {
        if let Some(at) = all.iter().position(|a| a == k) {
            all.remove(at);
        }
    }
    all
}

fn collect_parts(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Array(items) => {
            for item in items {
                if let (Some(slot), Some(id)) = (item.get("slot").and_then(Value::as_str), item.get("asset_id")) {
                    out.push((slot.to_owned(), id.to_string()));
                }
                collect_parts(item, out);
            }
        }
        Value::Object(map) => map.values().for_each(|v| collect_parts(v, out)),
        _ => {}
    }
}

pub fn drop_missing_parts(value: &mut Value, catalog: &Value) {
    let known: std::collections::HashSet<(String, String)> = catalog["components"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|component| {
            let slot = component["slot"].as_str().unwrap_or("").to_owned();
            component["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |m| Some((slot.clone(), id_text(m.get("id")?)?)))
        })
        .collect();
    // As the converter compares them: "12" is not 12.
    fn id_text(v: &Value) -> Option<String> {
        match v {
            Value::String(_) | Value::Number(_) => serde_json::to_string(v).ok(),
            _ => None,
        }
    }
    fn walk(value: &mut Value, known: &std::collections::HashSet<(String, String)>) {
        match value {
            Value::Array(items) => {
                items.retain(|item| match (item.get("slot").and_then(Value::as_str), item.get("asset_id").and_then(id_text)) {
                    (Some(slot), Some(id)) => known.contains(&(slot.to_owned(), id)),
                    _ => true,
                });
                for item in items {
                    walk(item, known);
                }
            }
            Value::Object(map) => {
                for (_, item) in map.iter_mut() {
                    walk(item, known);
                }
            }
            _ => {}
        }
    }
    walk(value, &known);
}

/// Prepares the character library (every head, hair, top, pair of pants
/// and shoes of Create-a-Skater) with `converter` from the game at `game`
/// into `assets`, reusing the board catalog's index when it is there.
/// Returns the library folder.
pub fn prepare_character(converter: &Path, game: &Path, assets: &Path, mut echo: impl FnMut(&str)) -> Result<PathBuf, String> {
    if !is_game_folder(game) {
        return Err(format!(
            "{} has no data/content/createacharacter.big. Keep the Skate 3 data folder beside default.xex.",
            game.display()
        ));
    }
    let catalog = catalog_dir(assets);
    if !catalog.join("catalog.json").is_file() {
        echo("Reading Skate 3's Create-a-Skater catalog");
        let mut args = vec!["--task".to_owned(), CATALOG_TASK.to_owned()];
        args.extend(catalog_args(game, assets));
        run(converter, &args, &mut echo)?;
    }
    let library = assets.join(CHARACTER_DIR);
    std::fs::create_dir_all(&library).map_err(|error| format!("cannot create {}: {error}", library.display()))?;
    let read = |name: &str| {
        let path = catalog.join(name);
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))
    };
    let full: Value = serde_json::from_slice(&read("catalog.json")?).map_err(|error| error.to_string())?;
    let mut parts = character_catalog(&full)?;
    let write = |name: &str, data: &[u8]| {
        let path = library.join(name);
        std::fs::write(&path, data).map_err(|error| format!("cannot write {}: {error}", path.display()))
    };
    // The catalog's own outfits and presets name parts too, the board's
    // among them (left out of this catalog): those go as well, or the
    // converter looks for them in vain.
    let known = parts.clone();
    let before = count_parts(&parts);
    drop_missing_parts(&mut parts, &known);
    let dropped = before.saturating_sub(count_parts(&parts));
    if dropped > 0 {
        echo(&format!("Left out {dropped} part(s) the catalog's outfits name but it has not got"));
    }
    let missing = default_skater_missing(&parts);
    if !missing.is_empty() {
        return Err(format!(
            "Skate 3's catalog lacks part(s) of the default skater the converter starts from: {}. \
             Delete {} and run this again, so the catalog is read from the game anew.",
            missing.join(", "),
            catalog.display()
        ));
    }
    write("catalog.json", &serde_json::to_vec(&parts).map_err(|error| error.to_string())?)?;
    // Parts the default skater wears that the catalog has not got (the
    // game lists some it does not ship) go from its list, or the converter
    // looks for them in vain.
    // The game's own list is kept as native.original.json; both places the
    // converter may read native.json from get the trimmed list.
    let original = catalog.join("native.original.json");
    if !original.is_file() {
        std::fs::copy(catalog.join("native.json"), &original)
            .map_err(|error| format!("cannot keep {}: {error}", original.display()))?;
    }
    let mut native: Value = serde_json::from_slice(&read("native.original.json")?).map_err(|error| error.to_string())?;
    let before = count_parts(&native);
    drop_missing_parts(&mut native, &parts);
    let dropped = before.saturating_sub(count_parts(&native));
    if dropped > 0 {
        echo(&format!("Left out {dropped} part(s) of the default skater that Skate 3's catalog does not have"));
    }
    let trimmed = serde_json::to_vec(&native).map_err(|error| error.to_string())?;
    write("native.json", &trimmed)?;
    std::fs::write(catalog.join("native.json"), &trimmed)
        .map_err(|error| format!("cannot write {}: {error}", catalog.join("native.json").display()))?;
    let config = json!({
        "assets": assets.display().to_string(),
        "game_root": game.display().to_string(),
        "directory": library.display().to_string(),
        "library_index": "library.json",
    });
    write("config.json", &serde_json::to_vec_pretty(&config).map_err(|error| error.to_string())?)?;
    echo("Converting heads, hair, clothes and shoes (this takes a while)");
    if let Err(error) = run(
        converter,
        &["--task".to_owned(), LIBRARY_TASK.to_owned(), library.join("config.json").display().to_string()],
        &mut echo,
    ) {
        // Which parts were named but not found, for the report.
        let mut missing = Vec::new();
        for dir in [&library, &catalog] {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json")
                    && let Ok(bytes) = std::fs::read(&path)
                    && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                {
                    for (slot, id) in unknown_parts(&value, &parts) {
                        missing.push(format!("{} in {}", format!("{slot} {id}"), path.display()));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        for line in missing.iter().take(12) {
            echo(&format!("  named but not in the catalog: {line}"));
        }
        return Err(error);
    }
    if !library.join("library.json").is_file() {
        return Err("the converter wrote no character library".into());
    }
    Ok(library)
}

/// Runs the converter, echoing its output; fails with its last error line.
fn run(converter: &Path, args: &[String], echo: &mut impl FnMut(&str)) -> Result<(), String> {
    let mut command = std::process::Command::new(converter);
    command.args(args);
    run_command(command, &converter.display().to_string(), echo)
}

/// Runs one of the Skate 3 Rust Engine's own tools (`task`, a path inside
/// `engine`, its source tree) with `python` (numpy and Pillow installed), in
/// place and read-only, as SK8V runs them: no converter of the 2010 Rust
/// Rewrite Mashup needed.
fn run_python(python: &Path, engine: &Path, task: &str, args: &[String], echo: &mut impl FnMut(&str)) -> Result<(), String> {
    let mut command = std::process::Command::new(python);
    command
        .arg("-B")
        .arg(engine.join(task))
        .args(args)
        .current_dir(engine)
        .env("PYTHONPATH", engine)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    run_command(command, &python.display().to_string(), echo)
}

/// Prepares the board library as [`prepare`] does, with the engine's tools
/// run by `python` from its source tree `engine` (see [`run_python`]).
pub fn prepare_with_python(python: &Path, engine: &Path, game: &Path, assets: &Path, mut echo: impl FnMut(&str)) -> Result<usize, String> {
    if !is_game_folder(game) {
        return Err(format!(
            "{} has no data/content/createacharacter.big. Keep the Skate 3 data folder beside default.xex.",
            game.display()
        ));
    }
    if !engine.join(CATALOG_TASK).is_file() {
        return Err(format!("{} is not the Skate 3 Rust Engine's source (no {CATALOG_TASK})", engine.display()));
    }
    echo("Reading Skate 3's skateboard catalog");
    run_python(python, engine, CATALOG_TASK, &catalog_args(game, assets), &mut echo)?;
    let config = write_library_config(game, assets)?;
    echo("Converting decks, trucks and wheels (this takes a few minutes)");
    run_python(python, engine, LIBRARY_TASK, &[config.display().to_string()], &mut echo)?;
    let library = crate::Library::load(assets)?;
    let count: usize = Part::ALL.iter().map(|part| library.items(*part).len()).sum();
    if library.items(Part::Deck).is_empty() {
        return Err("the engine's tools wrote a board library without decks".into());
    }
    echo(&format!("Skateboard catalog ready: {count} parts"));
    Ok(count)
}

/// Runs `command`, echoing its output; fails with its last error line.
fn run_command(mut command: std::process::Command, name: &str, echo: &mut impl FnMut(&str)) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start {name}: {error}"))?;
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        stderr
            .map(|stderr| BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let mut last = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            echo(&line);
            if !line.trim().is_empty() {
                last = Some(line);
            }
        }
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let errors = errors.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    for line in &errors {
        echo(line);
    }
    let reason = errors
        .iter()
        .rev()
        .chain(last.iter())
        .find(|line| !line.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| format!("the converter stopped ({status})"));
    Err(reason.trim().trim_start_matches("ERROR: ").to_owned())
}

#[cfg(test)]
mod missing_part_tests {
    use super::*;

    #[test]
    fn parts_the_catalog_lacks_leave_the_default_skater() {
        let catalog = json!({"components": [{"slot": "Head", "models": [{"id": "h1"}, {"id": 7}]}]});
        let mut native = json!({"skater": {"components": [
            {"slot": "Head", "asset_id": "h1"},
            {"slot": "Head", "asset_id": 7},
            {"slot": "Head", "asset_id": "7"},
            {"slot": "SkateBoard", "asset_id": "d1"},
            {"slot": "Head", "asset_id": "gone"},
            {"other": 1}
        ]}});
        drop_missing_parts(&mut native, &catalog);
        let left = native["skater"]["components"].as_array().unwrap();
        assert_eq!(left.len(), 3, "{left:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> Value {
        json!({"id": id, "name": id, "flags": {}, "lods": []})
    }

    #[test]
    fn keeps_board_parts_and_the_default_skater_only() {
        let mut components = vec![
            json!({"slot": "SkateBoard", "models": [model("aa"), model("bb")]}),
            json!({"slot": "Misc", "models": [model("cc")]}),
            json!({"slot": "Hat", "models": [model("dd")]}),
        ];
        for (slot, asset) in DEFAULT_SKATER {
            if slot == "SkateBoard" {
                components[0]["models"].as_array_mut().unwrap().push(model(asset));
                continue;
            }
            components.push(json!({"slot": slot, "models": [model(asset), model("ee")]}));
        }
        let full = json!({"version": 1, "materials": {}, "components": components});
        let boards = board_catalog(&full).unwrap();
        let slots: Vec<&str> = boards["components"].as_array().unwrap().iter().map(|c| c["slot"].as_str().unwrap()).collect();
        assert!(!slots.contains(&"Misc") && !slots.contains(&"Hat"));
        assert_eq!(slots.len(), 10);
        let deck = &boards["components"][0];
        assert_eq!(deck["models"].as_array().unwrap().len(), 3, "every deck is kept");
        let arm = boards["components"].as_array().unwrap().iter().find(|c| c["slot"] == "Arm").unwrap();
        assert_eq!(arm["models"].as_array().unwrap().len(), 1, "only the default arm");
        assert_eq!(boards["materials"], json!({}));
    }

    #[test]
    fn the_character_catalog_keeps_only_the_default_board() {
        let full = json!({"materials": {}, "components": [
            {"slot": "SkateBoard", "models": [model("aa"), model("a841a65d5c4dd593")]},
            {"slot": "Hair", "models": [model("bb"), model("cc")]},
            {"slot": "Pants", "models": [model("dd")]},
        ]});
        let parts = character_catalog(&full).unwrap();
        let slots: Vec<&str> = parts["components"].as_array().unwrap().iter().map(|c| c["slot"].as_str().unwrap()).collect();
        assert_eq!(slots, ["SkateBoard", "Hair", "Pants"]);
        let decks: Vec<&str> = parts["components"][0]["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(decks, ["a841a65d5c4dd593"], "only the default skater's deck");
        assert!(character_catalog(&json!({"components": [{"slot": "SkateWheel", "models": []}]})).is_err());
        let missing = default_skater_missing(&parts);
        assert!(missing.contains(&"Arm af71f8ae8ef28d10".to_owned()) && !missing.iter().any(|m| m.starts_with("SkateBoard")));
    }

    #[test]
    fn a_catalog_without_the_default_skater_is_refused() {
        let full = json!({"components": [{"slot": "SkateBoard", "models": [model("aa")]}]});
        assert!(board_catalog(&full).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn runs_both_converter_tasks() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("skate-boards-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        let assets = root.join("assets");
        std::fs::create_dir_all(game.join("data/content")).unwrap();
        std::fs::write(game.join("data/content/createacharacter.big"), b"").unwrap();
        std::fs::create_dir_all(&assets).unwrap();
        let mut components: Vec<Value> = DEFAULT_SKATER
            .iter()
            .map(|(slot, asset)| json!({"slot": slot, "models": [model(asset)]}))
            .collect();
        components.push(json!({"slot": "Hat", "models": [model("aa")]}));
        let catalog = json!({"materials": {}, "components": components}).to_string();
        let library = r#"{"models":{"d":{"slot":"SkateBoard","name":"Deck","materials":["m"],"scene":"x"}},"materials":{"m":{"name":"Flames","diffuse":"m.png"}}}"#;
        let script = format!(
            "#!/bin/sh\nif [ \"$2\" = \"{CATALOG_TASK}\" ]; then mkdir -p \"$6\"; printf '%s' '{catalog}' > \"$6/catalog.json\"; echo '{{}}' > \"$6/native.json\"; echo CAC_CATALOG_READY; exit 0; fi\n\
             dir=$(dirname \"$3\"); test -f \"$dir/catalog.json\" || exit 3; grep -q Hat \"$dir/catalog.json\" && exit 4; printf '%s' '{library}' > \"$dir/library.json\"; echo LIBRARY_READY\n"
        );
        let converter = root.join("convert.sh");
        std::fs::write(&converter, script).unwrap();
        std::fs::set_permissions(&converter, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut lines = Vec::new();
        let count = prepare(&converter, &game, &assets, |line| lines.push(line.to_owned())).unwrap();
        assert_eq!(count, 1);
        assert!(lines.iter().any(|l| l == "LIBRARY_READY"), "{lines:?}");
        assert!(crate::Library::exists(&assets));

        let failing = root.join("fail.sh");
        std::fs::write(&failing, "#!/bin/sh\necho working\necho 'ValueError: no catalog' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(prepare(&failing, &game, &assets, |_| {}).unwrap_err(), "ValueError: no catalog");
        assert!(prepare(&converter, &root, &assets, |_| {}).is_err(), "not a game folder");
        let _ = std::fs::remove_dir_all(&root);
    }
}
