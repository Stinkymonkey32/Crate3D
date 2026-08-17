//! Loads a `.rbxlx` place file and prints a summary of the instance tree
//! plus any property types that fell back to `Value::Unsupported`.
//!
//! Usage: `cargo run --example load_place -- path/to/file.rbxlx`

use std::collections::BTreeMap;
use std::env;
use std::path::Path;

use bloxvm::rbxlx::DataModel;
use bloxvm::value::Value;

fn main() {
    let path = env::args().nth(1).expect("usage: load_place <file.rbxlx>");
    let dm = DataModel::parse_rbxlx_path(Path::new(&path))
        .unwrap_or_else(|e| panic!("failed to load {path}: {e}"));

    let (count, unsupported) = dm.stats();
    println!("loaded {count} instances, {unsupported} unsupported properties");

    let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
    for inst in &dm.instances {
        for prop in inst.properties.values() {
            if let Value::Unsupported { tag, .. } = prop {
                *by_tag.entry(tag.clone()).or_default() += 1;
            }
        }
    }
    if by_tag.is_empty() {
        println!("all property types decoded");
    } else {
        println!("unsupported property tags:");
        for (tag, n) in &by_tag {
            println!("  {tag}: {n}");
        }
    }

    let root = dm.root();
    println!(
        "root: {} ({} children)",
        dm.instances[root].name,
        dm.instances[root].children.len()
    );
    for &child in &dm.instances[root].children {
        let c = dm.instance(child);
        println!("  - {} ({}) [{} children]", c.name, c.class, c.children.len());
    }
}
