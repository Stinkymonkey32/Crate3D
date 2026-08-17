//! Runtime values for instance properties, mirroring the data types found in
//! `.rbxlx` (XML) place files.

use std::fmt;

/// One keypoint of a `NumberSequence` property.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberSequenceKeypoint {
    pub time: f32,
    pub value: f32,
    pub envelope: f32,
}

/// One keypoint of a `ColorSequence` property.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorSequenceKeypoint {
    pub time: f32,
    pub color: [f32; 3],
    pub envelope: f32,
}

/// A property value loaded from a place file.
///
/// Unknown or exotic tags are captured losslessly in [`Value::Unsupported`] so
/// that loading a file never fails just because of an unrecognized property.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    String(String),
    /// Raw binary payload (base64 in `.rbxlx`).
    BinaryString(Vec<u8>),
    ProtectedString(String),
    /// A reference to an asset (`rbxasset://`, `rbxassetid://`, ...).
    Content(String),
    /// A referent reference to another instance in the file.
    Ref(String),
    Vector2 { x: f32, y: f32 },
    Vector3 { x: f32, y: f32, z: f32 },
    Vector2int16 { x: i16, y: i16 },
    Vector3int16 { x: i16, y: i16, z: i16 },
    Color3 { r: f32, g: f32, b: f32 },
    Color3uint8 { r: u8, g: u8, b: u8 },
    /// Position plus 3x3 rotation matrix (row-major).
    CFrame {
        position: [f64; 3],
        rotation: [f64; 9],
    },
    UDim { scale: f32, offset: i32 },
    /// (x scale, x offset), (y scale, y offset).
    UDim2 { x: [f32; 2], y: [f32; 2] },
    Rect { min: [f32; 2], max: [f32; 2] },
    NumberRange { min: f32, max: f32 },
    NumberSequence(Vec<NumberSequenceKeypoint>),
    ColorSequence(Vec<ColorSequenceKeypoint>),
    /// An enum serialized as its numeric value (`token` / `Enum`).
    Token(i64),
    /// A set of axes as a bitmask of X=1, Y=2, Z=4.
    Axes(u8),
    /// A set of faces as a bitmask of Right=1, Top=2, Back=4, Left=8,
    /// Bottom=16, Front=32.
    Faces(u8),
    PhysicalProperties {
        density: f32,
        friction: f32,
        elasticity: f32,
        friction_weight: f32,
        elasticity_weight: f32,
        custom: bool,
    },
    Ray {
        origin: [f64; 3],
        direction: [f64; 3],
    },
    Region3int16 {
        min: [i32; 3],
        max: [i32; 3],
    },
    BrickColor { id: u16 },
    Font {
        family: String,
        weight: String,
        style: String,
        cached_face_id: String,
    },
    /// Unix timestamp (seconds).
    DateTime(i64),
    SharedString {
        key: String,
        value: Vec<u8>,
    },
    /// UniqueId / UniqueIdGUID, kept as the raw serialized string.
    UniqueId(String),
    /// `FilteredTags` — a list of tag names.
    FilteredTags(Vec<String>),
    /// Any property type this loader does not understand yet, kept verbatim.
    Unsupported { tag: String, raw: String },
}

impl Value {
    /// Returns `true` if this value is the `Nil` variant.
    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// The `.rbxlx` element tag name this value was decoded from.
    pub fn tag_name(&self) -> &str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Int64(_) => "int64",
            Value::Float(_) => "float",
            Value::Double(_) => "double",
            Value::String(_) => "string",
            Value::BinaryString(_) => "BinaryString",
            Value::ProtectedString(_) => "ProtectedString",
            Value::Content(_) => "Content",
            Value::Ref(_) => "Ref",
            Value::Vector2 { .. } => "Vector2",
            Value::Vector3 { .. } => "Vector3",
            Value::Vector2int16 { .. } => "Vector2int16",
            Value::Vector3int16 { .. } => "Vector3int16",
            Value::Color3 { .. } => "Color3",
            Value::Color3uint8 { .. } => "Color3uint8",
            Value::CFrame { .. } => "CoordinateFrame",
            Value::UDim { .. } => "UDim",
            Value::UDim2 { .. } => "UDim2",
            Value::Rect { .. } => "Rect",
            Value::NumberRange { .. } => "NumberRange",
            Value::NumberSequence(_) => "NumberSequence",
            Value::ColorSequence(_) => "ColorSequence",
            Value::Token(_) => "token",
            Value::Axes(_) => "Axes",
            Value::Faces(_) => "Faces",
            Value::PhysicalProperties { .. } => "PhysicalProperties",
            Value::Ray { .. } => "Ray",
            Value::Region3int16 { .. } => "Region3int16",
            Value::BrickColor { .. } => "BrickColor",
            Value::Font { .. } => "Font",
            Value::DateTime(_) => "DateTime",
            Value::SharedString { .. } => "SharedString",
            Value::UniqueId(_) => "UniqueId",
            Value::FilteredTags(_) => "FilteredTags",
            Value::Unsupported { tag, .. } => tag,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Int64(i) => write!(f, "{i}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Double(v) => write!(f, "{v}"),
            Value::String(s) | Value::ProtectedString(s) | Value::UniqueId(s) => write!(f, "{s:?}"),
            Value::BinaryString(b) => write!(f, "<{} bytes>", b.len()),
            Value::Content(s) => write!(f, "{s}"),
            Value::Ref(r) => write!(f, "{r}"),
            Value::Vector2 { x, y } => write!(f, "{x}, {y}"),
            Value::Vector3 { x, y, z } => write!(f, "{x}, {y}, {z}"),
            Value::Vector2int16 { x, y } => write!(f, "{x}, {y}"),
            Value::Vector3int16 { x, y, z } => write!(f, "{x}, {y}, {z}"),
            Value::Color3 { r, g, b } => write!(f, "{r}, {g}, {b}"),
            Value::Color3uint8 { r, g, b } => write!(f, "{r}, {g}, {b}"),
            Value::CFrame { position, rotation } => write!(f, "{position:?}, {rotation:?}"),
            Value::UDim { scale, offset } => write!(f, "{scale}, {offset}"),
            Value::UDim2 { x, y } => write!(f, "{x:?}, {y:?}"),
            Value::Rect { min, max } => write!(f, "{min:?}, {max:?}"),
            Value::NumberRange { min, max } => write!(f, "{min}, {max}"),
            Value::NumberSequence(pts) => write!(f, "<{} keypoints>", pts.len()),
            Value::ColorSequence(pts) => write!(f, "<{} keypoints>", pts.len()),
            Value::Token(t) => write!(f, "{t}"),
            Value::Axes(mask) | Value::Faces(mask) => write!(f, "{mask:#x}"),
            Value::PhysicalProperties { .. } => write!(f, "PhysicalProperties"),
            Value::Ray { origin, direction } => write!(f, "{origin:?}, {direction:?}"),
            Value::Region3int16 { min, max } => write!(f, "{min:?}, {max:?}"),
            Value::BrickColor { id } => write!(f, "{id}"),
            Value::Font { family, .. } => write!(f, "{family}"),
            Value::DateTime(ts) => write!(f, "{ts}"),
            Value::SharedString { key, .. } => write!(f, "SharedString({key})"),
            Value::FilteredTags(tags) => write!(f, "{tags:?}"),
            Value::Unsupported { tag, raw } => write!(f, "<{tag}> {raw}"),
        }
    }
}
