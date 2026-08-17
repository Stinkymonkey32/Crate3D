//! BloxVM — a Roblox emulator written in Rust.
//!
//! The API surface is generated from the Roblox Creator Documentation
//! engine reference by `tools/codegen/codegen.py` into `generated/`.

pub mod generated;
pub mod reflection;

/// High-level queries over the generated API registry.
pub mod api;

/// Runtime instance tree.
pub mod instance;

/// Property value types.
pub mod value;

/// `.rbxlx` (XML) place-file loader.
pub mod rbxlx;

/// DDS texture parsing and DXT decompression.
pub mod texture;

/// OpenGL renderer: scene building, camera, and draw calls.
pub mod render;

/// Rapier 3D physics: world building, the player character, and the avatar.
pub mod physics;

pub use generated::*;
pub use reflection::*;
