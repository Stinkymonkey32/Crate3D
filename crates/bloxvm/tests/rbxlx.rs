//! Integration tests for loading `.rbxlx` place files.

use bloxvm::rbxlx::DataModel;
use bloxvm::value::Value;

const FIXTURE: &str = "tests/fixtures/minimal.rbxlx";
const MODERN_FIXTURE: &str = "tests/fixtures/modern.rbxlx";

fn load() -> DataModel {
    DataModel::parse_rbxlx_path(std::path::Path::new(FIXTURE)).expect("fixture should load")
}

fn load_modern() -> DataModel {
    DataModel::parse_rbxlx_path(std::path::Path::new(MODERN_FIXTURE)).expect("modern fixture should load")
}

#[test]
fn loads_all_instances() {
    let dm = load();
    assert_eq!(dm.instances.len(), 75);
}

#[test]
fn root_is_data_model() {
    let dm = load();
    let root = dm.root();
    assert_eq!(dm.instances[root].class, "DataModel");
    assert_eq!(dm.instances[root].name, "Game");
}

#[test]
fn workspace_service_and_hierarchy() {
    let dm = load();
    let root = dm.root();
    let ws = dm.get_service("Workspace").expect("workspace service");
    assert_eq!(dm.instances[ws].name, "Workspace");
    assert_eq!(dm.instances[ws].parent, Some(root));

    // Workspace children: Camera, Baseplate, Terrain, SpawnLocation.
    assert_eq!(dm.instances[ws].children.len(), 4);
    assert!(dm.find_first_child(ws, "Baseplate").is_some());
    assert!(dm.find_first_child(ws, "Terrain").is_some());
    assert!(dm.find_first_child(ws, "SpawnLocation").is_some());
}

#[test]
fn part_properties_parse() {
    let dm = load();
    let ws = dm.get_service("Workspace").unwrap();
    let plate = dm.find_first_child(ws, "Baseplate").unwrap();
    let inst = dm.instance(plate);

    assert_eq!(inst.class, "Part");
    assert!(inst.is_a("BasePart"));

    match inst.get_property("Size").unwrap() {
        Value::Vector3 { x, y, z } => assert!((*x - 2048.0).abs() < 1e-6 && (*y - 16.0).abs() < 1e-6 && (*z - 2048.0).abs() < 1e-6),
        other => panic!("expected Vector3, got {other:?}"),
    }

    match inst.get_property("Anchored").unwrap() {
        Value::Bool(true) => {}
        other => panic!("expected anchored true, got {other:?}"),
    }

    match inst.get_property("Material").unwrap() {
        Value::Token(256) => {}
        other => panic!("expected token 256, got {other:?}"),
    }

    match inst.get_property("CFrame").unwrap() {
        Value::CFrame { position, rotation } => {
            assert_eq!(*position, [0.0, -8.0, 0.0]);
            assert_eq!(*rotation, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        }
        other => panic!("expected CFrame, got {other:?}"),
    }

    // Studio serializes these `BasePart` properties under legacy names; the
    // loader canonicalizes them so lookups use the API names.
    match inst.get_property("Color").unwrap() {
        Value::Color3uint8 { r, g, b } => assert_eq!((*r, *g, *b), (91, 91, 91)),
        other => panic!("expected Color3uint8, got {other:?}"),
    }

    match inst.get_property("Shape").unwrap() {
        Value::Token(1) => {}
        other => panic!("expected token 1, got {other:?}"),
    }

    match inst.get_property("FormFactor").unwrap() {
        Value::Token(0) => {}
        other => panic!("expected token 0, got {other:?}"),
    }
}

#[test]
fn spawn_location_parses() {
    let dm = load();
    let ws = dm.get_service("Workspace").unwrap();
    let spawn = dm.find_first_child(ws, "SpawnLocation").unwrap();
    let inst = dm.instance(spawn);

    assert_eq!(inst.class, "SpawnLocation");
    assert!(inst.is_a("BasePart"));

    match inst.get_property("Size").unwrap() {
        Value::Vector3 { x, y, z } => assert!((*x - 12.0).abs() < 1e-6 && (*y - 1.0).abs() < 1e-6 && (*z - 12.0).abs() < 1e-6),
        other => panic!("expected Vector3, got {other:?}"),
    }

    match inst.get_property("CFrame").unwrap() {
        Value::CFrame { position, .. } => assert_eq!(*position, [0.0, 0.5, 0.0]),
        other => panic!("expected CFrame, got {other:?}"),
    }

    match inst.get_property("Shape").unwrap() {
        Value::Token(1) => {}
        other => panic!("expected token 1, got {other:?}"),
    }
}

#[test]
fn no_unsupported_properties_in_fixture() {
    let dm = load();
    let (count, unsupported) = dm.stats();
    assert_eq!(count, 75);
    assert_eq!(unsupported, 0, "fixture properties should all be decoded");
}

#[test]
fn find_first_descendant_search() {
    let dm = load();
    let root = dm.root();
    let spawn = dm.find_first_descendant(root, "SpawnLocation").expect("find descendant");
    assert_eq!(dm.instances[spawn].class, "SpawnLocation");
}

#[test]
fn missing_root_reports_error() {
    let err = DataModel::parse_rbxlx(b"<notroblox/>").unwrap_err();
    assert!(err.to_string().contains("roblox"));
}

#[test]
fn modern_loads_all_instances() {
    let dm = load_modern();
    // Workspace, BasePlate, SurfaceAppearance, Terrain + synthesized Game.
    assert_eq!(dm.instances.len(), 5);
}

#[test]
fn modern_root_is_synthesized_data_model() {
    let dm = load_modern();
    let root = dm.root();
    assert_eq!(dm.instances[root].class, "DataModel");
    assert_eq!(dm.instances[root].name, "Game");
    let ws = dm.get_service("Workspace").expect("workspace service");
    assert_eq!(dm.instances[ws].parent, Some(root));
}

#[test]
fn modern_nested_items_build_tree() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let plate = dm.find_first_child(ws, "BasePlate").unwrap();
    assert_eq!(dm.instances[plate].parent, Some(ws));
    let surface = dm.find_first_child(plate, "SurfaceAppearance").unwrap();
    assert_eq!(dm.instances[surface].parent, Some(plate));
    assert_eq!(dm.instances[plate].children, vec![surface]);
}

#[test]
fn modern_child_element_values_parse() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let plate = dm.find_first_child(ws, "BasePlate").unwrap();
    let inst = dm.instance(plate);

    match inst.get_property("Size").unwrap() {
        Value::Vector3 { x, y, z } => assert!((*x - 512.0).abs() < 1e-6 && (*y - 20.0).abs() < 1e-6 && (*z - 512.0).abs() < 1e-6),
        other => panic!("expected Vector3, got {other:?}"),
    }

    match inst.get_property("CFrame").unwrap() {
        Value::CFrame { position, rotation } => {
            assert_eq!(*position, [0.0, 0.0, 0.0]);
            assert_eq!(*rotation, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        }
        other => panic!("expected CFrame, got {other:?}"),
    }

    match inst.get_property("FaceButtonsSize").unwrap() {
        Value::UDim2 { x, y } => assert_eq!((*x, *y), ([0.0, 50.0], [0.0, 50.0])),
        other => panic!("expected UDim2, got {other:?}"),
    }

    match inst.get_property("PhysicalProperties").unwrap() {
        Value::PhysicalProperties { density, friction, elasticity, custom, .. } => {
            assert!((*density - 0.7).abs() < 1e-6);
            assert!((*friction - 0.3).abs() < 1e-6);
            assert!((*elasticity - 0.5).abs() < 1e-6);
            assert!(*custom);
        }
        other => panic!("expected PhysicalProperties, got {other:?}"),
    }

    match inst.get_property("Axe").unwrap() {
        Value::Axes(1) => {}
        other => panic!("expected Axes X, got {other:?}"),
    }

    match inst.get_property("Face").unwrap() {
        Value::Faces(5) => {}
        other => panic!("expected Faces Right|Back, got {other:?}"),
    }

    match inst.get_property("BrickColor").unwrap() {
        Value::Color3uint8 { r, g, b } => assert_eq!((*r, *g, *b), (96, 64, 32)),
        other => panic!("expected packed Color3uint8, got {other:?}"),
    }
}

#[test]
fn modern_content_uri_parses() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let plate = dm.find_first_child(ws, "BasePlate").unwrap();
    match dm.instance(plate).get_property("Texture").unwrap() {
        Value::Content(uri) => assert_eq!(*uri, "rbxassetid://1234567890"),
        other => panic!("expected Content, got {other:?}"),
    }
}

#[test]
fn modern_optional_cframe_parses() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let plate = dm.find_first_child(ws, "BasePlate").unwrap();
    match dm.instance(plate).get_property("AlignPosition").unwrap() {
        Value::CFrame { position, .. } => assert_eq!(*position, [1.0, 2.0, 3.0]),
        other => panic!("expected CFrame, got {other:?}"),
    }
}

#[test]
fn modern_shared_strings_resolve() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let surface = dm.find_first_descendant(ws, "SurfaceAppearance").unwrap();
    match dm.instance(surface).get_property("ColorMap").unwrap() {
        Value::SharedString { key, value } => {
            assert_eq!(*key, "AAAAAAAAAAAAAAAAAAAAAAAA");
            assert_eq!(*value, b"fake texture data");
        }
        other => panic!("expected SharedString, got {other:?}"),
    }
    let terrain = dm.find_first_child(ws, "Terrain").unwrap();
    match dm.instance(terrain).get_property("Materials").unwrap() {
        Value::SharedString { value, .. } => assert_eq!(*value, b"fake texture data"),
        other => panic!("expected SharedString, got {other:?}"),
    }
}

#[test]
fn modern_font_parses() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let surface = dm.find_first_descendant(ws, "SurfaceAppearance").unwrap();
    match dm.instance(surface).get_property("Font").unwrap() {
        Value::Font { family, weight, style, .. } => {
            assert_eq!(*family, "rbxasset://fonts/families/BuilderSans");
            assert_eq!(*weight, "700");
            assert_eq!(*style, "Italic");
        }
        other => panic!("expected Font, got {other:?}"),
    }
}

#[test]
fn modern_rect_parses() {
    let dm = load_modern();
    let ws = dm.get_service("Workspace").unwrap();
    let terrain = dm.find_first_child(ws, "Terrain").unwrap();
    match dm.instance(terrain).get_property("DecalThickness").unwrap() {
        Value::Rect { min, max } => assert_eq!((*min, *max), ([0.0, 0.0], [1.0, 1.0])),
        other => panic!("expected Rect, got {other:?}"),
    }
}

#[test]
fn modern_no_unsupported_properties() {
    let dm = load_modern();
    let (count, unsupported) = dm.stats();
    assert_eq!(count, 5);
    assert_eq!(unsupported, 0, "modern fixture properties should all be decoded");
}

#[test]
fn avatar_export_roundtrips() {
    let xml = bloxvm::rbxlx::write_avatar_rbxlx();
    let dm = DataModel::parse_rbxlx(xml.as_bytes()).expect("exported avatar should parse back");
    assert_eq!(dm.instances.len(), 14, "synthesized Game + Workspace + Model + 6 parts + 5 Motor6Ds");

    let parts: Vec<_> = dm.instances.iter().filter(|i| i.class == "Part").collect();
    assert_eq!(parts.len(), 6);
    let names: Vec<&str> = parts.iter().map(|i| i.name.as_str()).collect();
    for expected in ["Head", "Torso", "Left Arm", "Right Arm", "Left Leg", "Right Leg"] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    let torso = parts.iter().find(|i| i.name == "Torso").unwrap();
    match torso.get_property("Size") {
        Some(Value::Vector3 { x, y, z }) => assert_eq!((*x, *y, *z), (2.0, 2.0, 1.0)),
        other => panic!("expected Size Vector3, got {other:?}"),
    }
    match torso.get_property("Color") {
        Some(Value::Color3uint8 { r, g, b }) => assert_eq!((*r, *g, *b), (0, 162, 255)),
        other => panic!("expected bright blue, got {other:?}"),
    }

    let motors: Vec<_> = dm.instances.iter().filter(|i| i.class == "Motor6D").collect();
    assert_eq!(motors.len(), 5);
    let neck = motors.iter().find(|i| i.name == "Neck").unwrap();
    match neck.get_property("Part1") {
        Some(Value::Ref(r)) => {
            let head = dm.instance(dm.by_referent[r.as_str()]);
            assert_eq!(head.name, "Head");
        }
        other => panic!("expected Part1 Ref, got {other:?}"),
    }
}
