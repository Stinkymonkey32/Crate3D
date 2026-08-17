//! Loader for `.rbxlx` (XML) place files.
//!
//! Supports both the modern (version 4+) format — nested `<Item>` elements,
//! child-element property values, a `<SharedStrings>` block — and the legacy
//! flat format with `Parent` `Ref` properties and comma-separated values.
//! Both are handled by the same code paths; the loader accepts whatever it
//! finds in each property.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use base64::Engine as _;
use roxmltree::{Document, Node};

use crate::instance::Instance;
use crate::value::{ColorSequenceKeypoint, NumberSequenceKeypoint, Value};

pub use crate::instance::DataModel;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug)]
pub enum Error {
    /// The input could not be parsed as XML.
    Xml(String),
    /// The document root was not a `<roblox>` element.
    MissingRoot,
    /// Failed to read the file.
    Io(std::io::Error),
    /// The file is not valid UTF-8.
    Utf8(std::str::Utf8Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Xml(msg) => write!(f, "XML parse error: {msg}"),
            Error::MissingRoot => write!(f, "not a Roblox place file (missing <roblox> root)"),
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Utf8(e) => write!(f, "file is not valid UTF-8: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl DataModel {
    /// Parses a `.rbxlx` place file from a byte slice.
    pub fn parse_rbxlx(bytes: &[u8]) -> Result<DataModel, Error> {
        let text = std::str::from_utf8(bytes).map_err(Error::Utf8)?;
        let doc = Document::parse(text).map_err(|e| Error::Xml(e.to_string()))?;
        let root = doc.root_element();
        if root.tag_name().name() != "roblox" {
            return Err(Error::MissingRoot);
        }

        let mut dm = DataModel::default();

        // The `<SharedStrings>` block holds deduplicated byte payloads keyed by
        // md5. Parse it first so `SharedString` properties can resolve during
        // item parsing regardless of where it appears in the file.
        for el in root.children().filter(Node::is_element) {
            if el.tag_name().name() == "SharedStrings" {
                for ss in el.children().filter(Node::is_element) {
                    if ss.tag_name().name() != "SharedString" {
                        continue;
                    }
                    if let Some(md5) = ss.attribute("md5") {
                        dm.shared_strings.insert(md5.to_string(), decode_b64(ss.text().unwrap_or("")));
                    }
                }
            }
        }

        // Legacy files are flat and use `Parent` `Ref` properties. Modern files
        // nest `<Item>` elements structurally. Both are handled here; any item
        // still without a structural parent falls back to its `Parent` ref.
        let mut legacy_parents: Vec<(usize, String)> = Vec::new();

        for item in root.children().filter(Node::is_element) {
            if item.tag_name().name() != "Item" {
                continue;
            }
            visit_item(item, &mut dm, &mut legacy_parents, None);
        }

        for (id, referent) in legacy_parents {
            if dm.instances[id].parent.is_none() {
                if let Some(&pid) = dm.by_referent.get(&referent) {
                    dm.instances[id].parent = Some(pid);
                    dm.instances[pid].children.push(id);
                }
            }
        }

        // Ensure there is a root DataModel instance so `root()` and services work.
        if !dm.instances.iter().any(|i| i.class == "DataModel") {
            let id = dm.instances.len();
            dm.instances.push(Instance {
                id,
                class: "DataModel".to_string(),
                name: "Game".to_string(),
                properties: Default::default(),
                parent: None,
                children: Vec::new(),
            });
            let orphans: Vec<usize> = (0..id).filter(|&i| dm.instances[i].parent.is_none()).collect();
            for child in orphans {
                dm.instances[child].parent = Some(id);
                dm.instances[id].children.push(child);
            }
        }

        Ok(dm)
    }

    /// Parses a `.rbxlx` place file from disk.
    pub fn parse_rbxlx_path(path: &Path) -> Result<DataModel, Error> {
        let bytes = std::fs::read(path).map_err(Error::Io)?;
        DataModel::parse_rbxlx(&bytes)
    }
}

/// Parses one `<Item>` and all of its structurally nested children.
///
/// `parent` is set when the item is nested inside another item (modern format).
/// Legacy `Parent` `Ref` properties are recorded in `legacy_parents` instead.
fn visit_item(item: Node, dm: &mut DataModel, legacy_parents: &mut Vec<(usize, String)>, parent: Option<usize>) {
    let class = item.attribute("class").unwrap_or("Instance").to_string();
    let referent = item
        .attribute("referent")
        .map(str::to_string)
        .unwrap_or_else(|| format!("RBX{}", dm.instances.len()));

    let id = dm.instances.len();
    let mut inst = Instance {
        id,
        class,
        name: String::new(),
        properties: Default::default(),
        parent,
        children: Vec::new(),
    };

    if let Some(props) = item.children().find(|c| c.is_element() && c.tag_name().name() == "Properties") {
        for prop in props.children().filter(Node::is_element) {
            let Some((name, value)) = parse_property(prop, &dm.shared_strings) else {
                continue;
            };
            if name == "Parent" {
                if let Value::Ref(r) = value {
                    legacy_parents.push((id, r));
                }
                continue;
            }
            if name == "Name" {
                if let Value::String(s) = &value {
                    inst.name = s.clone();
                }
            }
            inst.properties.insert(name, value);
        }
    }

    dm.by_referent.insert(referent, id);
    if let Some(pid) = parent {
        dm.instances[pid].children.push(id);
    }
    dm.instances.push(inst);

    for child in item.children().filter(Node::is_element) {
        if child.tag_name().name() == "Item" {
            visit_item(child, dm, legacy_parents, Some(id));
        }
    }
}

fn parse_property(node: Node, shared: &BTreeMap<String, Vec<u8>>) -> Option<(String, Value)> {
    let name = node.attribute("name")?;
    let tag = node.tag_name().name();
    let value = parse_value(tag, node, shared)?;
    Some((canonical_property_name(tag, name), value))
}

/// Roblox's XML serializer writes a few `BasePart` properties under legacy
/// names that don't match their canonical API names (`Size` → `size`,
/// `Shape` → `shape`, `FormFactor` → `formFactorRaw`, `Color` →
/// `Color3uint8`). Map them back so consumers can look properties up by the
/// canonical name.
fn canonical_property_name(tag: &str, name: &str) -> String {
    match (tag, name) {
        (_, "size") => "Size",
        (_, "shape") => "Shape",
        (_, "formFactorRaw") => "FormFactor",
        ("Color3uint8", "Color3uint8") => "Color",
        _ => name,
    }
    .to_string()
}

fn parse_value(tag: &str, node: Node, shared: &BTreeMap<String, Vec<u8>>) -> Option<Value> {
    let text = node.text().unwrap_or("");
    let url = node.attribute("url");
    let has_child = node.children().filter(Node::is_element).next().is_some();

    let value = match tag {
        "string" => Value::String(text.to_string()),
        "bool" => Value::Bool(matches!(text.trim(), "true")),
        "int" => Value::Int(parse_i64(text)? as i32),
        "int64" => Value::Int64(parse_i64(text)?),
        "float" => Value::Float(parse_f32(text)?),
        "double" => Value::Double(parse_f64(text)?),
        "BinaryString" => Value::BinaryString(decode_b64(text)),
        "ProtectedString" => Value::ProtectedString(text.to_string()),
        "Content" | "ContentId" => Value::Content(parse_content(node, url).unwrap_or_default()),
        "Ref" | "Object" => {
            let t = text.trim();
            if t == "null" || t.is_empty() {
                Value::Nil
            } else {
                Value::Ref(t.to_string())
            }
        }
        "Vector3" => {
            if has_child {
                let v = vec3_children(node)?;
                Value::Vector3 { x: v[0], y: v[1], z: v[2] }
            } else {
                let v = tokens(text);
                if v.len() < 3 {
                    return None;
                }
                Value::Vector3 {
                    x: parse_f32(v[0])?,
                    y: parse_f32(v[1])?,
                    z: parse_f32(v[2])?,
                }
            }
        }
        "Vector2" => {
            if has_child {
                let v = vec2_children(node)?;
                Value::Vector2 { x: v[0], y: v[1] }
            } else {
                let v = tokens(text);
                if v.len() < 2 {
                    return None;
                }
                Value::Vector2 {
                    x: parse_f32(v[0])?,
                    y: parse_f32(v[1])?,
                }
            }
        }
        "Vector3int16" => {
            if has_child {
                let v = vec3int16_children(node)?;
                Value::Vector3int16 { x: v[0], y: v[1], z: v[2] }
            } else {
                let v = tokens(text);
                if v.len() < 3 {
                    return None;
                }
                Value::Vector3int16 {
                    x: parse_i64(v[0])? as i16,
                    y: parse_i64(v[1])? as i16,
                    z: parse_i64(v[2])? as i16,
                }
            }
        }
        "Vector2int16" => {
            if has_child {
                let v = vec2int16_children(node)?;
                Value::Vector2int16 { x: v[0], y: v[1] }
            } else {
                let v = tokens(text);
                if v.len() < 2 {
                    return None;
                }
                Value::Vector2int16 {
                    x: parse_i64(v[0])? as i16,
                    y: parse_i64(v[1])? as i16,
                }
            }
        }
        "Color3" => {
            if has_child {
                let v = color3_children(node)?;
                Value::Color3 { r: v[0], g: v[1], b: v[2] }
            } else {
                let v = tokens(text);
                if v.len() < 3 {
                    return None;
                }
                Value::Color3 {
                    r: parse_f32(v[0])?,
                    g: parse_f32(v[1])?,
                    b: parse_f32(v[2])?,
                }
            }
        }
        "Color3uint8" => {
            let v = tokens(text);
            if v.len() >= 3 {
                Value::Color3uint8 {
                    r: parse_i64(v[0])? as u8,
                    g: parse_i64(v[1])? as u8,
                    b: parse_i64(v[2])? as u8,
                }
            } else if let Some(n) = parse_i64(text) {
                // Modern format packs RGB into the lower 24 bits (R high byte).
                Value::Color3uint8 {
                    r: ((n >> 16) & 0xff) as u8,
                    g: ((n >> 8) & 0xff) as u8,
                    b: (n & 0xff) as u8,
                }
            } else {
                return None;
            }
        }
        "CoordinateFrame" | "CFrame" => {
            if has_child {
                let v = cframe_children(node)?;
                Value::CFrame {
                    position: [v[0], v[1], v[2]],
                    rotation: v[3..12].try_into().ok()?,
                }
            } else {
                let v = tokens(text);
                if v.len() < 12 {
                    return None;
                }
                let nums: Vec<f64> = v.iter().map(|t| parse_f64(t)).collect::<Option<Vec<_>>>()?;
                let mut position = [0.0; 3];
                let mut rotation = [0.0; 9];
                position.copy_from_slice(&nums[0..3]);
                rotation.copy_from_slice(&nums[3..12]);
                Value::CFrame { position, rotation }
            }
        }
        "OptionalCoordinateFrame" | "OptionalCFrame" => {
            match node.children().filter(Node::is_element).next() {
                Some(c) => match cframe_children(c) {
                    Some(v) => Value::CFrame {
                        position: [v[0], v[1], v[2]],
                        rotation: v[3..12].try_into().ok()?,
                    },
                    None => Value::Nil,
                },
                None => Value::Nil,
            }
        }
        "UDim" => {
            if has_child {
                let s = child_f32(node, "S")?;
                let o = child_i64(node, "O")? as i32;
                Value::UDim { scale: s, offset: o }
            } else {
                let v = tokens(text);
                if v.len() < 2 {
                    return None;
                }
                Value::UDim {
                    scale: parse_f32(v[0])?,
                    offset: parse_i64(v[1])? as i32,
                }
            }
        }
        "UDim2" => {
            if has_child {
                Value::UDim2 {
                    x: [child_f32(node, "XS")?, child_f32(node, "XO")?],
                    y: [child_f32(node, "YS")?, child_f32(node, "YO")?],
                }
            } else {
                let v = tokens(text);
                if v.len() < 4 {
                    return None;
                }
                Value::UDim2 {
                    x: [parse_f32(v[0])?, parse_i64(v[1])? as f32],
                    y: [parse_f32(v[2])?, parse_i64(v[3])? as f32],
                }
            }
        }
        "Rect" | "Rect2D" => {
            if has_child {
                let min = child_node(node, "min").and_then(vec2_children)?;
                let max = child_node(node, "max").and_then(vec2_children)?;
                Value::Rect { min, max }
            } else {
                let v = tokens(text);
                if v.len() < 4 {
                    return None;
                }
                Value::Rect {
                    min: [parse_f32(v[0])?, parse_f32(v[1])?],
                    max: [parse_f32(v[2])?, parse_f32(v[3])?],
                }
            }
        }
        "NumberRange" => {
            let v = tokens(text);
            if v.len() < 2 {
                return None;
            }
            Value::NumberRange {
                min: parse_f32(v[0])?,
                max: parse_f32(v[1])?,
            }
        }
        "NumberSequence" => {
            let mut points = Vec::new();
            if has_child {
                for kp in node.children().filter(Node::is_element) {
                    let v = tokens(kp.text().unwrap_or(""));
                    if v.len() >= 3 {
                        points.push(NumberSequenceKeypoint {
                            time: parse_f32(v[0])?,
                            value: parse_f32(v[1])?,
                            envelope: parse_f32(v[2])?,
                        });
                    }
                }
            } else {
                for v in tokens(text).chunks(3) {
                    if v.len() >= 3 {
                        points.push(NumberSequenceKeypoint {
                            time: parse_f32(v[0])?,
                            value: parse_f32(v[1])?,
                            envelope: parse_f32(v[2])?,
                        });
                    }
                }
            }
            Value::NumberSequence(points)
        }
        "ColorSequence" => {
            let mut points = Vec::new();
            if has_child {
                for kp in node.children().filter(Node::is_element) {
                    let v = tokens(kp.text().unwrap_or(""));
                    if v.len() >= 5 {
                        points.push(ColorSequenceKeypoint {
                            time: parse_f32(v[0])?,
                            color: [parse_f32(v[1])?, parse_f32(v[2])?, parse_f32(v[3])?],
                            envelope: parse_f32(v[4])?,
                        });
                    }
                }
            } else {
                for v in tokens(text).chunks(5) {
                    if v.len() >= 5 {
                        points.push(ColorSequenceKeypoint {
                            time: parse_f32(v[0])?,
                            color: [parse_f32(v[1])?, parse_f32(v[2])?, parse_f32(v[3])?],
                            envelope: parse_f32(v[4])?,
                        });
                    }
                }
            }
            Value::ColorSequence(points)
        }
        "token" | "Enum" => {
            if let Some(n) = parse_i64(text) {
                Value::Token(n)
            } else {
                Value::Unsupported {
                    tag: tag.to_string(),
                    raw: text.to_string(),
                }
            }
        }
        "SecurityCapabilities" => Value::Token(parse_i64(text)?),
        "Axes" => {
            if has_child {
                let n = node.children().filter(Node::is_element).next().and_then(|c| parse_i64(c.text().unwrap_or("")))?;
                Value::Axes(n as u8)
            } else {
                Value::Axes(parse_mask(text)?)
            }
        }
        "Faces" => {
            if has_child {
                let n = node.children().filter(Node::is_element).next().and_then(|c| parse_i64(c.text().unwrap_or("")))?;
                Value::Faces(n as u8)
            } else {
                Value::Faces(parse_mask(text)?)
            }
        }
        "PhysicalProperties" => {
            if has_child {
                Value::PhysicalProperties {
                    custom: child_text(node, "CustomPhysics").map_or(false, |t| t == "true"),
                    density: child_f32(node, "Density")?,
                    friction: child_f32(node, "Friction")?,
                    elasticity: child_f32(node, "Elasticity")?,
                    friction_weight: child_f32(node, "FrictionWeight")?,
                    elasticity_weight: child_f32(node, "ElasticityWeight")?,
                }
            } else {
                let v = tokens(text);
                if v.len() < 5 {
                    return None;
                }
                let custom = v.get(5).map_or(false, |t| matches!(*t, "true"));
                Value::PhysicalProperties {
                    density: parse_f32(v[0])?,
                    friction: parse_f32(v[1])?,
                    elasticity: parse_f32(v[2])?,
                    friction_weight: parse_f32(v[3])?,
                    elasticity_weight: parse_f32(v[4])?,
                    custom,
                }
            }
        }
        "Ray" => {
            if has_child {
                let origin = child_node(node, "origin").and_then(vec3_children)?;
                let direction = child_node(node, "direction").and_then(vec3_children)?;
                let o = origin.map(|v| v as f64);
                let d = direction.map(|v| v as f64);
                Value::Ray { origin: o, direction: d }
            } else {
                let v = tokens(text);
                if v.len() < 6 {
                    return None;
                }
                let nums: Vec<f64> = v.iter().map(|t| parse_f64(t)).collect::<Option<Vec<_>>>()?;
                Value::Ray {
                    origin: [nums[0], nums[1], nums[2]],
                    direction: [nums[3], nums[4], nums[5]],
                }
            }
        }
        "Region3int16" => {
            let v = tokens(text);
            if v.len() < 6 {
                return None;
            }
            let nums: Vec<i64> = v.iter().map(|t| parse_i64(t)).collect::<Option<Vec<_>>>()?;
            Value::Region3int16 {
                min: [nums[0] as i32, nums[1] as i32, nums[2] as i32],
                max: [nums[3] as i32, nums[4] as i32, nums[5] as i32],
            }
        }
        "BrickColor" => Value::BrickColor {
            id: parse_i64(text).unwrap_or(0) as u16,
        },
        "Font" => {
            let family = child_node(node, "Family").and_then(|c| parse_content(c, c.attribute("url"))).unwrap_or_default();
            Value::Font {
                family,
                weight: child_text(node, "Weight").unwrap_or_else(|| "Normal".to_string()),
                style: child_text(node, "Style").unwrap_or_else(|| "Normal".to_string()),
                cached_face_id: child_node(node, "CachedFaceId").and_then(|c| parse_content(c, c.attribute("url"))).unwrap_or_default(),
            }
        }
        "DateTime" => Value::DateTime(parse_i64(text).unwrap_or(0)),
        "SharedString" | "NetAssetRef" => {
            let key = text.trim().to_string();
            let value = shared.get(&key).cloned().unwrap_or_else(|| decode_b64(text));
            Value::SharedString { key, value }
        }
        "UniqueId" | "UniqueIdGUID" => Value::UniqueId(text.trim().to_string()),
        "FilteredTags" => {
            let tags: Vec<String> = node
                .children()
                .filter(Node::is_element)
                .filter_map(|c| c.text())
                .map(str::to_string)
                .collect();
            Value::FilteredTags(tags)
        }
        // Script elements store their payload via a `url` attribute.
        "Script" => {
            if let Some(u) = url {
                Value::Content(u.to_string())
            } else {
                Value::ProtectedString(text.to_string())
            }
        }
        _ => Value::Unsupported {
            tag: tag.to_string(),
            raw: text.to_string(),
        },
    };
    Some(value)
}

/// Parses a `Content`/`ContentId` element. Modern files store the URI in a
/// child (`<uri>`, `<url>`); `null` means an empty (unset) content.
/// Legacy files use a `url` attribute or plain text.
fn parse_content(node: Node, url: Option<&str>) -> Option<String> {
    for c in node.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "uri" | "url" => return Some(c.text().unwrap_or("").to_string()),
            "null" => return Some(String::new()),
            "Ref" => return Some(c.text().unwrap_or("").to_string()),
            "binary" | "hash" => return Some(String::new()),
            _ => {}
        }
    }
    url.map(str::to_string)
        .or_else(|| Some(node.text().unwrap_or("").to_string()))
}

/// The named child element, if any.
fn child_node<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Text content of the named child element, if any.
fn child_text(node: Node, name: &str) -> Option<String> {
    child_node(node, name).and_then(|c| c.text().map(str::to_string))
}

fn child_f32(node: Node, name: &str) -> Option<f32> {
    child_text(node, name).and_then(|s| parse_f32(&s))
}

fn child_i64(node: Node, name: &str) -> Option<i64> {
    child_text(node, name).and_then(|s| parse_i64(&s))
}

fn vec2_children(node: Node) -> Option<[f32; 2]> {
    Some([child_f32(node, "X")?, child_f32(node, "Y")?])
}

fn vec3_children(node: Node) -> Option<[f32; 3]> {
    Some([child_f32(node, "X")?, child_f32(node, "Y")?, child_f32(node, "Z")?])
}

fn vec2int16_children(node: Node) -> Option<[i16; 2]> {
    Some([child_i64(node, "X")? as i16, child_i64(node, "Y")? as i16])
}

fn vec3int16_children(node: Node) -> Option<[i16; 3]> {
    Some([child_i64(node, "X")? as i16, child_i64(node, "Y")? as i16, child_i64(node, "Z")? as i16])
}

fn color3_children(node: Node) -> Option<[f32; 3]> {
    Some([child_f32(node, "R")?, child_f32(node, "G")?, child_f32(node, "B")?])
}

/// CFrame children: `X Y Z R00 R01 R02 R10 R11 R12 R20 R21 R22`.
fn cframe_children(node: Node) -> Option<[f64; 12]> {
    let mut out = [0.0; 12];
    out[0] = child_f64(node, "X")?;
    out[1] = child_f64(node, "Y")?;
    out[2] = child_f64(node, "Z")?;
    let rot = [
        "R00", "R01", "R02",
        "R10", "R11", "R12",
        "R20", "R21", "R22",
    ];
    for (i, name) in rot.iter().enumerate() {
        out[3 + i] = child_f64(node, name)?;
    }
    Some(out)
}

fn child_f64(node: Node, name: &str) -> Option<f64> {
    child_text(node, name).and_then(|s| parse_f64(&s))
}

/// Splits a serialized number list on commas and whitespace.
fn tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect()
}

fn parse_i64(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

fn parse_f64(s: &str) -> Option<f64> {
    let t = s.trim();
    match t.to_ascii_lowercase().as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" | "1.#inf" => return Some(f64::INFINITY),
        "-inf" | "-infinity" | "-1.#inf" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    if t.to_ascii_lowercase().contains("nan") || t.to_ascii_lowercase().contains("ind") {
        return Some(f64::NAN);
    }
    t.parse().ok()
}

fn parse_f32(s: &str) -> Option<f32> {
    parse_f64(s).map(|v| v as f32)
}

/// Parses `Axes`/`Faces` masks written as axis letters (`"X, Y"`) or
/// per-axis booleans (`"1, 0, 1"`).
fn parse_mask(s: &str) -> Option<u8> {
    let v = tokens(s);
    if v.is_empty() {
        return Some(0);
    }
    if v.iter().all(|t| matches!(*t, "0" | "1")) && v.len() <= 3 {
        let mut mask = 0u8;
        for (i, t) in v.iter().enumerate() {
            if *t == "1" {
                mask |= 1 << i;
            }
        }
        return Some(mask);
    }
    let mut mask = 0u8;
    for t in v {
        let c = t.chars().next()?.to_ascii_uppercase();
        mask |= match c {
            'X' => 1,
            'Y' => 2,
            'Z' => 4,
            _ => 0,
        };
    }
    Some(mask)
}

/// Decodes a base64 payload, ignoring any surrounding whitespace.
fn decode_b64(s: &str) -> Vec<u8> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    B64.decode(cleaned.as_bytes()).unwrap_or_default()
}

/// Serializes a classic R6 "noob" avatar as a `.rbxlx` document that Roblox
/// Studio can open: a `Model` holding the six rig parts joined by `Motor6D`
/// joints (using the classic template's `C0`/`C1` values, so the rig animates
/// normally in Roblox).
pub fn write_avatar_rbxlx() -> String {
    let parts: [(&str, [f32; 3], [f32; 3], [u8; 3]); 6] = [
        ("Torso", [0.0, 3.0, 0.0], [2.0, 2.0, 1.0], [0, 162, 255]),
        ("Head", [0.0, 4.5, 0.0], [1.0, 1.0, 1.0], [245, 205, 48]),
        ("Left Arm", [-1.5, 3.0, 0.0], [1.0, 2.0, 1.0], [245, 205, 48]),
        ("Right Arm", [1.5, 3.0, 0.0], [1.0, 2.0, 1.0], [245, 205, 48]),
        ("Left Leg", [-0.5, 1.0, 0.0], [1.0, 2.0, 1.0], [199, 210, 60]),
        ("Right Leg", [0.5, 1.0, 0.0], [1.0, 2.0, 1.0], [199, 210, 60]),
    ];
    // (name, part0 index, part1 index, C0, C1)
    let motors: [(&str, usize, usize, [f32; 3], [f32; 3]); 5] = [
        ("Neck", 0, 1, [0.0, 1.0, 0.0], [0.0, -0.5, 0.0]),
        ("Right Shoulder", 0, 3, [1.0, 0.5, 0.0], [-0.5, 0.5, 0.0]),
        ("Left Shoulder", 0, 2, [-1.0, 0.5, 0.0], [0.5, 0.5, 0.0]),
        ("Right Hip", 0, 5, [1.0, -1.0, 0.0], [-0.5, 1.0, 0.0]),
        ("Left Hip", 0, 4, [-1.0, -1.0, 0.0], [0.5, 1.0, 0.0]),
    ];

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <roblox xmlns:xmime=\"http://www.w3.org/2005/05/xmlmime\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:noNamespaceSchemaLocation=\"http://www.roblox.com/roblox.xsd\" version=\"4\">\n\
         <External>null</External>\n\
         <Item class=\"Workspace\" referent=\"RBX0\">\n\
         <Properties>\n\
         <string name=\"Name\">Workspace</string>\n\
         </Properties>\n\
         <Item class=\"Model\" referent=\"RBX1\">\n\
         <Properties>\n\
         <string name=\"Name\">Noob</string>\n\
         </Properties>\n",
    );

    for (i, (name, pos, size, color)) in parts.iter().enumerate() {
        let referent = format!("RBX{}", i + 2);
        out.push_str(&format!(
            "<Item class=\"Part\" referent=\"{referent}\">\n\
             <Properties>\n\
             <string name=\"Name\">{name}</string>\n\
             <CoordinateFrame name=\"CFrame\">{} {} {}, 1, 0, 0, 0, 1, 0, 0, 0, 1</CoordinateFrame>\n\
             <Vector3 name=\"Size\">{} {} {}</Vector3>\n\
             <Color3uint8 name=\"Color\">{} {} {}</Color3uint8>\n\
             <bool name=\"Anchored\">false</bool>\n\
             <token name=\"Shape\">1</token>\n\
             <token name=\"Material\">256</token>\n\
             <bool name=\"CanCollide\">true</bool>\n\
             </Properties>\n\
             </Item>\n",
            pos[0], pos[1], pos[2], size[0], size[1], size[2], color[0], color[1], color[2],
        ));
    }

    for (i, (name, p0, p1, c0, c1)) in motors.iter().enumerate() {
        let referent = format!("RBX{}", i + 8);
        let part0 = format!("RBX{}", p0 + 2);
        let part1 = format!("RBX{}", p1 + 2);
        out.push_str(&format!(
            "<Item class=\"Motor6D\" referent=\"{referent}\">\n\
             <Properties>\n\
             <string name=\"Name\">{name}</string>\n\
             <CoordinateFrame name=\"C0\">{} {} {}, 1, 0, 0, 0, -1, 0, 0, 0, -1</CoordinateFrame>\n\
             <CoordinateFrame name=\"C1\">{} {} {}, 1, 0, 0, 0, -1, 0, 0, 0, -1</CoordinateFrame>\n\
             <Object name=\"Part0\">{part0}</Object>\n\
             <Object name=\"Part1\">{part1}</Object>\n\
             <bool name=\"Anchored\">false</bool>\n\
             <bool name=\"Enabled\">true</bool>\n\
             </Properties>\n\
             </Item>\n",
            c0[0], c0[1], c0[2], c1[0], c1[1], c1[2],
        ));
    }

    out.push_str("</Item>\n</Item>\n</roblox>\n");
    out
}
