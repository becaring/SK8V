use super::*;
use crate::glb::build::Writer;
use serde_json::json;

fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let data: Vec<u8> = (0..width * height).flat_map(|_| color).collect();
        writer.write_image_data(&data).unwrap();
    }
    out
}

fn translation(x: f32) -> [f32; 16] {
    [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., x, 0., 0., 1.]
}

/// A skinned GLB: joints ROOT, DECK, TRUCK; one triangle per named
/// material, the first on DECK, the second on TRUCK.
fn model(materials: &[&str], image: bool) -> Vec<u8> {
    let mut w = Writer::default();
    let inverse: Vec<f32> = [translation(0.), translation(-1.), translation(-2.)].concat();
    let ibm = w.floats(&inverse, "MAT4");
    let mut primitives = Vec::new();
    let mut gltf_materials = Vec::new();
    let mut images = Vec::new();
    let mut textures = Vec::new();
    for (i, name) in materials.iter().enumerate() {
        let bone = (i + 1) as u16;
        let pos = w.floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.], "VEC3");
        let nrm = w.floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.], "VEC3");
        let uv = w.floats(&[0., 0., 1., 0., 0., 1.], "VEC2");
        let joints = w.shorts(&[bone, 0, 0, 0, bone, 0, 0, 0, bone, 0, 0, 0], "VEC4");
        let weights = w.floats(&[1., 0., 0., 0., 1., 0., 0., 0., 2., 0., 0., 0.], "VEC4");
        let indices = w.ints(&[0, 1, 2]);
        primitives.push(json!({"attributes": {"POSITION": pos, "NORMAL": nrm, "TEXCOORD_0": uv,
            "JOINTS_0": joints, "WEIGHTS_0": weights}, "indices": indices, "material": i}));
        let mut material = json!({"name": name, "pbrMetallicRoughness": {}});
        if image {
            let view = w.image(&png(2, 2, [10 * i as u8, 20, 30, 255]));
            images.push(json!({"bufferView": view, "mimeType": "image/png"}));
            textures.push(json!({"source": images.len() - 1}));
            material["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": textures.len() - 1});
        }
        gltf_materials.push(material);
    }
    w.finish(json!({
        "asset": {"version": "2.0"},
        "nodes": [{"name": "ROOT"}, {"name": "DECK"}, {"name": "TRUCK"}, {"mesh": 0, "skin": 0}],
        "skins": [{"joints": [0, 1, 2], "inverseBindMatrices": ibm}],
        "meshes": [{"primitives": primitives}],
        "materials": gltf_materials,
        "images": images,
        "textures": textures,
    }))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skate-boards-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("private/boards/library")).unwrap();
    dir
}

const LIBRARY: &str = r#"{"version":3,"models":{
  "d1":{"slot":"SkateBoard","name":"Deck","flags":{},"materials":["m1","m2","m3"],"groups":[["m1","m2","m3"]],"scene":"private/boards/library/d1_v4.glb"},
  "d2":{"slot":"SkateBoard","name":"Long Deck","flags":{},"materials":["m4"],"groups":[["m4"]],"scene":"private/boards/library/d2_v4.glb"},
  "t1":{"slot":"SkateTruck","name":"Truck","flags":{},"materials":["m5","missing"],"groups":[["m5"]],"scene":"private/boards/library/t1_v4.glb"},
  "h1":{"slot":"Hair","name":"Hair","flags":{},"materials":["m1"],"groups":[["m1"]],"scene":"x.glb"}},
 "materials":{
  "m1":{"name":"Flames","diffuse":"private/boards/m1.png","tint":[1,1,1]},
  "m2":{"name":"Flames (Worn)","diffuse":"private/boards/m1.png"},
  "m3":{"name":"Anchor","diffuse":"private/boards/m3.png","tint":[0.5,1,1]},
  "m4":{"name":"Flames","diffuse":"private/boards/m1.png"},
  "m5":{"name":"Raw","diffuse":"private/boards/m1.png"}}}"#;

#[test]
fn brands_read_as_shops_show_them() {
    assert_eq!(brand_name("zoo_york"), "Zoo York");
    assert_eq!(brand_name("zoo york"), "Zoo York");
    assert_eq!(brand_name("blacklabel"), "Black Label");
    assert_eq!(brand_name("spitfire"), "Spitfire");
    assert_eq!(brand_name(""), "Unbranded");
}

#[test]
fn the_catalog_lists_finishes_by_part() {
    let library = Library::from_json(Path::new("/x"), LIBRARY).unwrap();
    let decks: Vec<String> = library.items(Part::Deck).iter().map(Item::title).collect();
    assert_eq!(decks, ["Anchor", "Flames (Deck)", "Flames (Long Deck)", "Flames (Worn)"]);
    let trucks = library.items(Part::Trucks);
    assert_eq!(trucks.len(), 1, "finishes the library lacks are left out");
    assert!(library.items(Part::Wheels).is_empty());
    let worn = &library.items(Part::Deck)[3];
    assert!(worn.worn && worn.label == "Flames");
    assert!(library.find(Part::Deck, &PartRef::new("d1", "m3")).is_some());
    let offered: Vec<String> = library.new_items(Part::Deck).iter().map(|i| i.title()).collect();
    assert_eq!(offered, ["Anchor", "Flames (Deck)", "Flames (Long Deck)"], "worn finishes are not for sale");
    assert_eq!(library.worn_of(Part::Deck, &PartRef::new("d1", "m1")), Some(PartRef::new("d1", "m2")));
    assert_eq!(library.worn_of(Part::Deck, &PartRef::new("d2", "m4")), None, "another model's Flames");
    assert_eq!(library.worn_of(Part::Deck, &PartRef::new("d1", "m3")), None, "no worn Anchor");
    assert_eq!(library.fresh_of(Part::Deck, &PartRef::new("d1", "m2")), PartRef::new("d1", "m1"));
    assert_eq!(library.fresh_of(Part::Deck, &PartRef::new("d1", "m3")), PartRef::new("d1", "m3"));
    assert!(library.find(Part::Trucks, &PartRef::new("d1", "m3")).is_none());
}

#[test]
fn choices_round_trip_through_text() {
    let mut choice = BoardChoice::default();
    assert_eq!(choice.to_text(), "");
    choice.set(Part::Deck, Some(PartRef::new("a841a65d5c4dd593", "0123abcd")));
    choice.set(Part::Wheels, Some(PartRef::new("87d0fa0ea0366b23", "ff")));
    let text = choice.to_text();
    assert_eq!(text, "a841a65d5c4dd593:0123abcd,,87d0fa0ea0366b23:ff");
    assert_eq!(BoardChoice::from_text(&text), choice);
    assert_eq!(BoardChoice::from_text("garbage"), BoardChoice::default());
    assert_eq!(BoardChoice::from_text("z-z:y y,,").get(Part::Deck), None, "not an id");
    // Painted parts are kept too (they were lost on every load: B71).
    let painted = BoardChoice::from_text("paint:paint_redm,c6f01c93097b7fb5:2c7f381700172b7a,paint:paint_goldx");
    assert_eq!(painted.get(Part::Deck), Some(&PartRef::new("paint", "paint_redm")));
    assert_eq!(painted.get(Part::Wheels), Some(&PartRef::new("paint", "paint_goldx")));
}

#[test]
fn boards_combine_chosen_and_default_parts() {
    let assets = temp("assemble");
    std::fs::write(
        assets.join("private/skater.glb"),
        model(&["Retail_SkateBoard", "Retail_SkateTruck", "Retail_Pants"], true),
    )
    .unwrap();
    std::fs::write(assets.join("private/boards/library/d1_v4.glb"), model(&["Retail_SkateBoard"], false)).unwrap();
    std::fs::write(assets.join("private/boards/m1.png"), png(4, 4, [200, 100, 50, 255])).unwrap();
    std::fs::write(assets.join("private/boards/m3.png"), png(4, 4, [200, 100, 50, 255])).unwrap();
    std::fs::write(library_path(&assets), LIBRARY).unwrap();

    let defaults = Board::default_board(&assets, 512).unwrap();
    assert_eq!(defaults.surfaces.len(), 2, "pants are not part of the board");
    assert_eq!(defaults.surfaces[0].material, "iw4l_skate/Retail_SkateBoard");
    assert_eq!(defaults.joints.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["DECK", "TRUCK"]);
    let truck = defaults.only(Part::Trucks);
    assert_eq!(truck.surfaces.len(), 1);
    assert_eq!(truck.joints[truck.surfaces[0].vertices[0].joints[0] as usize].name, "TRUCK");

    let library = Library::load(&assets).unwrap();
    let mut choice = BoardChoice::default();
    choice.set(Part::Deck, Some(PartRef::new("d1", "m3")));
    let board = assemble(Some(&library), &defaults, &choice, 2).unwrap();
    assert_eq!(board.surfaces.len(), 2);
    let deck = &board.surfaces[0];
    assert_eq!((deck.part, deck.material.as_str()), (Part::Deck, "iw4l_skate/board/m3"));
    let texture = &board.textures[deck.texture];
    assert_eq!((texture.width, texture.height), (2, 2), "shrunk to the limit");
    assert_eq!(texture.sample([0.1, 0.1]), [100, 100, 50, 255], "tinted");
    let truck = &board.surfaces[1];
    assert_eq!(board.joints[truck.vertices[0].joints[0] as usize].name, "TRUCK");
    assert_eq!(board.joints[deck.vertices[0].joints[0] as usize].inverse_bind[12], -1.0);
    assert!((deck.vertices[2].weights[0] - 1.0).abs() < 1e-6, "weights are normalised");

    let wheels = {
        let mut c = BoardChoice::default();
        c.set(Part::Wheels, Some(PartRef::new("w9", "m9")));
        c
    };
    assert!(assemble(Some(&library), &defaults, &wheels, 512).is_err());
    assert_eq!(assemble(None, &defaults, &BoardChoice::default(), 512).unwrap().surfaces.len(), 2);
    let _ = std::fs::remove_dir_all(&assets);
}

#[test]
fn textures_sample_with_wrapping() {
    let texture = Texture {
        width: 2,
        height: 1,
        rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };
    assert_eq!(texture.sample([0.25, 0.5]), [1, 2, 3, 4]);
    assert_eq!(texture.sample([0.75, 0.5]), [5, 6, 7, 8]);
    assert_eq!(texture.sample([1.25, -0.5]), [1, 2, 3, 4]);
}

#[test]
fn a_deck_graphic_is_printed_over_the_blank_deck() {
    let base = Texture { width: 2, height: 1, rgba: vec![100, 60, 20, 255, 10, 10, 10, 255] };
    // Left: see-through, right: solid red.
    let mut graphic = Texture { width: 4, height: 2, rgba: [[0, 0, 0, 0], [0, 0, 0, 0], [255, 0, 0, 255], [255, 0, 0, 255]].repeat(2).concat() };
    graphic.print_over(&base);
    assert_eq!(&graphic.rgba[0..4], &[100, 60, 20, 255], "the wood shows through");
    assert_eq!(&graphic.rgba[8..12], &[255, 0, 0, 255], "the print stays");
    let mut solid = Texture { width: 1, height: 1, rgba: vec![1, 2, 3, 255] };
    solid.print_over(&base);
    assert_eq!(solid.rgba, vec![1, 2, 3, 255]);
}

#[test]
fn finishes_are_solid_unless_they_have_an_opacity_map() {
    let assets = temp("alpha");
    let dir = assets.join(crate::LIBRARY_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    // Half see-through pixels in both finishes.
    let image = png(2, 2, [200, 40, 40, 60]);
    std::fs::write(dir.join("plain.png"), &image).unwrap();
    std::fs::write(dir.join("cut.png"), &image).unwrap();
    let json = r#"{"models":{},"materials":{
        "plain":{"name":"Plain","diffuse":"private/boards/plain.png"},
        "cut":{"name":"Cut","diffuse":"private/boards/cut.png","alpha":true}}}"#;
    let library = Library::from_json(&assets, json).unwrap();
    let plain = library.texture("plain", 512).unwrap();
    assert!(plain.rgba.chunks(4).all(|p| p[3] == 255 && p[0] == 200), "solid, colour kept");
    let cut = library.texture("cut", 512).unwrap();
    assert!(cut.rgba.chunks(4).all(|p| p[3] == 0), "cut at a half");
}

#[test]
fn classes_get_boards_that_suit_their_weapons() {
    let item = |part: Part, asset: &str, label: &str| Item {
        part,
        asset: asset.into(),
        material: "0001".into(),
        label: label.into(),
        worn: false,
        finish: label.into(),
        brand: String::new(),
    };
    let items = |part: Part| match part {
        Part::Deck => vec![item(part, "d1", "Neon Green Logo"), item(part, "d2", "Black Night"), item(part, "d3", "Classic Blue")],
        Part::Trucks => vec![item(part, "t1", "Silver"), item(part, "t2", "Black")],
        Part::Wheels => vec![item(part, "w1", "White 52mm")],
    };
    let sniper = fitting_board(items, "cheytac_mp", "First Recon");
    assert_eq!(sniper.get(Part::Deck).unwrap().asset, "d2");
    assert_eq!(sniper.get(Part::Trucks).unwrap().asset, "t2");
    assert_eq!(sniper.get(Part::Wheels).unwrap().asset, "w1", "no dark wheels: one is picked anyway");
    let smg = fitting_board(items, "ump45_mp", "Grenadier");
    assert_eq!(smg.get(Part::Deck).unwrap().asset, "d1");
    assert_eq!(fitting_board(items, "m4_mp", "x"), fitting_board(items, "m4_mp", "x"), "the same every time");
}

#[test]
fn the_catalogs_own_outfits_lose_parts_it_has_not_got() {
    let mut catalog = serde_json::json!({
        "components": [{"slot": "Head", "models": [{"id": "h1"}]}],
        "presets": [{"name": "default", "components": [{"slot": "Head", "asset_id": "h1"}, {"slot": "SkateBoard", "asset_id": "d1"}]}]
    });
    let known = catalog.clone();
    crate::convert::drop_missing_parts(&mut catalog, &known);
    assert_eq!(catalog["presets"][0]["components"].as_array().unwrap().len(), 1);
    assert_eq!(catalog["components"].as_array().unwrap().len(), 1, "the catalog's own parts stay");
}

#[test]
fn board_textures_have_names_in_gta() {
    assert_eq!(gta_texture(Part::Wheels, "iw4l_skate/board/00004ACC03E38817"), ("skate3_board_wheels".into(), "00004acc03e38817".into()));
    assert_eq!(gta_texture(Part::Deck, "iw4l_skate/SkateBoard Deck.1"), ("skate3_board_default".into(), "default_skateboard_deck_1".into()));
}

#[test]
fn cut_out_finishes_show_the_part_under_them_not_the_world() {
    let assets = temp("solid");
    let dir = assets.join(crate::LIBRARY_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cut.png"), png(2, 2, [200, 40, 40, 60])).unwrap();
    let json = r#"{"models":{},"materials":{
        "cut":{"name":"Cut","diffuse":"private/boards/cut.png","alpha":true}}}"#;
    let library = Library::from_json(&assets, json).unwrap();
    for part in Part::ALL {
        let texture = library.solid_texture(part, "cut", 512).unwrap();
        assert!(texture.rgba.chunks(4).all(|p| p[3] == 255), "{part:?} is solid");
    }
    let deck = library.solid_texture(Part::Deck, "cut", 512).unwrap();
    assert_eq!(&deck.rgba[..3], &[214, 178, 132], "bare maple under a deck's cut-out");
}

#[test]
#[ignore = "reads the real board library (SKATE_ASSETS) and the player's imported decks"]
fn imported_decks_join_the_catalog_with_their_maker() {
    let assets = std::path::PathBuf::from(std::env::var("SKATE_ASSETS").expect("SKATE_ASSETS = the board library's assets folder"));
    let library = Library::load(&assets).unwrap();
    let decks = library.new_items(Part::Deck);
    let custom: Vec<_> = decks.iter().filter(|i| i.material.starts_with("c057")).map(|i| (i.label.clone(), i.brand.clone())).collect();
    eprintln!("{custom:?}");
    assert!(!custom.is_empty());
    assert!(custom.iter().all(|(_, b)| !b.is_empty()));
    assert!(library.texture_path(&decks.iter().find(|i| i.material.starts_with("c057")).unwrap().material).is_some());
}

#[test]
fn skates_three_normal_map_layouts_come_out_in_gtas() {
    use crate::{Texture, normal_from_skate};
    // Red/green with blue: kept (z rebuilt from x and y).
    let rgb = normal_from_skate(Texture { width: 1, height: 1, rgba: vec![128, 200, 180, 255] });
    assert_eq!(&rgb.rgba[..2], &[128, 200]);
    assert!(rgb.rgba[2] > 200);
    // Blue left blank (255 or 128): z rebuilt the same.
    let blank = normal_from_skate(Texture { width: 1, height: 1, rgba: vec![128, 200, 128, 255] });
    assert_eq!(blank.rgba, rgb.rgba);
    // Grey, x in alpha: x moves to red.
    let grey = normal_from_skate(Texture { width: 1, height: 1, rgba: vec![150, 150, 150, 60] });
    assert_eq!(&grey.rgba[..2], &[60, 150]);
    // Flat stays flat.
    let flat = normal_from_skate(Texture { width: 1, height: 1, rgba: vec![127, 127, 127, 127] });
    assert!(flat.rgba[0].abs_diff(127) <= 1 && flat.rgba[1].abs_diff(127) <= 1 && flat.rgba[2] >= 254);
}
