//! Runtime instance tree.

use std::collections::BTreeMap;

use crate::api;
use crate::value::Value;

/// A single instance in the tree. Instances live in a [`DataModel`] arena and
/// refer to each other by stable numeric id.
#[derive(Debug, Clone)]
pub struct Instance {
    /// Stable id within the owning [`DataModel`].
    pub id: usize,
    /// The class name, e.g. `"Part"`, `"Script"`, `"DataModel"`.
    pub class: String,
    /// The `Name` property.
    pub name: String,
    /// All properties decoded from the file (including `Name`, but not
    /// `Parent`, which is stored structurally).
    pub properties: BTreeMap<String, Value>,
    /// Parent instance id, if any.
    pub parent: Option<usize>,
    /// Direct children, in file order.
    pub children: Vec<usize>,
}

impl Instance {
    pub fn get_property(&self, name: &str) -> Option<&Value> {
        self.properties.get(name)
    }

    /// `true` if this instance is `class` or inherits from it, per the
    /// generated class registry. Unknown classes fall back to exact matches.
    pub fn is_a(&self, class: &str) -> bool {
        if self.class == class {
            return true;
        }
        api::is_a(&self.class, class)
    }

    /// Walks up the parent chain looking for an instance that is `class`.
    pub fn find_first_ancestor_of_class(&self, data_model: &DataModel, class: &str) -> Option<usize> {
        let mut cur = self.parent;
        while let Some(id) = cur {
            let inst = &data_model.instances[id];
            if inst.is_a(class) {
                return Some(id);
            }
            cur = inst.parent;
        }
        None
    }
}

/// The root container holding every instance in the loaded place.
///
/// Instances are stored in a flat arena (`Vec`) so that the tree can be
/// borrowed and mutated without the borrow checker fighting over parent-child
/// back-references.
#[derive(Debug, Clone, Default)]
pub struct DataModel {
    pub instances: Vec<Instance>,
    /// Referent string -> instance id, for resolving `Ref` properties.
    pub by_referent: BTreeMap<String, usize>,
    /// Shared string md5 key -> payload bytes, from the file's
    /// `<SharedStrings>` block. `SharedString` properties reference these.
    pub shared_strings: BTreeMap<String, Vec<u8>>,
}

impl DataModel {
    /// Id of the root `DataModel` instance.
    pub fn root(&self) -> usize {
        self.instances
            .iter()
            .position(|i| i.class == "DataModel")
            .unwrap_or(0)
    }

    pub fn instance(&self, id: usize) -> &Instance {
        &self.instances[id]
    }

    pub fn instance_mut(&mut self, id: usize) -> &mut Instance {
        &mut self.instances[id]
    }

    /// Finds the first direct child of `id` named `name`.
    pub fn find_first_child(&self, id: usize, name: &str) -> Option<usize> {
        self.instances[id]
            .children
            .iter()
            .copied()
            .find(|&c| self.instances[c].name == name)
    }

    /// Depth-first search for a descendant of `id` named `name`.
    pub fn find_first_descendant(&self, id: usize, name: &str) -> Option<usize> {
        let mut stack: Vec<usize> = self.instances[id].children.clone();
        while let Some(cid) = stack.pop() {
            if self.instances[cid].name == name {
                return Some(cid);
            }
            stack.extend(self.instances[cid].children.iter().rev().copied());
        }
        None
    }

    /// Finds a service under the root `DataModel` by class or name, e.g.
    /// `"Workspace"`, `"Lighting"`, `"ReplicatedStorage"`.
    pub fn get_service(&self, name: &str) -> Option<usize> {
        let root = self.root();
        for &c in &self.instances[root].children {
            let inst = &self.instances[c];
            if inst.name == name || inst.class == name {
                return Some(c);
            }
        }
        None
    }

    /// Total number of instances, and how many properties hit the
    /// [`Value::Unsupported`] fallback.
    pub fn stats(&self) -> (usize, usize) {
        let unsupported = self
            .instances
            .iter()
            .flat_map(|i| i.properties.values())
            .filter(|v| matches!(v, Value::Unsupported { .. }))
            .count();
        (self.instances.len(), unsupported)
    }
}
