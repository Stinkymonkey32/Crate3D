//! High-level queries over the generated API registry.

use crate::generated;
use crate::reflection::{ClassDef, DatatypeDef, EnumDef, GlobalDef, LibraryDef};

/// Look up a class definition by name (e.g. `"Part"`, `"Instance"`).
pub fn class_def(name: &str) -> Option<&'static ClassDef> {
    generated::class_def(name)
}

/// Look up an enum definition by name (e.g. `"PartType"`).
pub fn enum_def(name: &str) -> Option<&'static EnumDef> {
    generated::enum_def(name)
}

/// Look up a datatype definition by name (e.g. `"Vector3"`).
pub fn datatype_def(name: &str) -> Option<&'static DatatypeDef> {
    generated::datatype_def(name)
}

/// Look up a library by name (e.g. `"task"`).
pub fn library_def(name: &str) -> Option<&'static LibraryDef> {
    generated::library_def(name)
}

/// Look up a globals table by name (`"LuaGlobals"`, `"Roblox globals"`).
pub fn global_def(name: &str) -> Option<&'static GlobalDef> {
    generated::global_def(name)
}

/// Returns true if `class` is `ancestor` or inherits from it (transitively).
pub fn is_a(class: &str, ancestor: &str) -> bool {
    match class_def(class) {
        Some(def) => def.is_a(ancestor, class_def),
        None => false,
    }
}

/// All direct subclasses of `parent`, in definition order.
pub fn subclasses<'a>(parent: &'a str) -> impl Iterator<Item = &'static ClassDef> + 'a {
    generated::all_classes().filter(move |c| c.inherits == parent)
}

/// The full inheritance chain of `class`, from the class itself up to `Instance`.
pub fn inheritance_chain(class: &str) -> Vec<&'static str> {
    let mut chain = Vec::new();
    let mut cur = class_def(class);
    while let Some(c) = cur {
        chain.push(c.name);
        if c.inherits.is_empty() {
            break;
        }
        cur = class_def(c.inherits);
    }
    chain
}
