//! Writes the classic R6 "noob" avatar as a `.rbxlx` file that Roblox Studio
//! can open (a Model with the six rig parts joined by Motor6D welds).
//!
//! Usage: `cargo run --example export_avatar -- [output.rbxlx]`

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "noob.rbxlx".to_string());
    std::fs::write(&out, bloxvm::rbxlx::write_avatar_rbxlx())?;
    println!("wrote {out}");
    Ok(())
}
