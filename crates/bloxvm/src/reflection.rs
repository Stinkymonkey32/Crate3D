//! Reflection metadata types shared by the generated API registry.
//!
//! The generated code in `generated/` populates these types with data parsed
//! from the Roblox Creator Documentation engine reference. These types are
//! hand-written and stable; the data is machine generated.

/// Security level of an API member, matching Roblox's security contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Security {
    None,
    PluginSecurity,
    RobloxScriptSecurity,
    RobloxEngineSecurity,
    LocalUserSecurity,
    NotAccessibleSecurity,
    /// Any security level not yet classified, kept as its raw string.
    Other(&'static str),
}

impl Security {
    pub fn parse(s: &'static str) -> Self {
        match s {
            "" | "None" => Security::None,
            "PluginSecurity" => Security::PluginSecurity,
            "RobloxScriptSecurity" => Security::RobloxScriptSecurity,
            "RobloxEngineSecurity" => Security::RobloxEngineSecurity,
            "LocalUserSecurity" => Security::LocalUserSecurity,
            "NotAccessibleSecurity" => Security::NotAccessibleSecurity,
            other => Security::Other(other),
        }
    }
}

/// Thread-safety contract of an API member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadSafety {
    ReadSafe,
    Unsafe,
    Safe,
    /// Any thread-safety level not yet classified, kept as its raw string.
    Unknown(&'static str),
}

impl ThreadSafety {
    pub fn parse(s: &'static str) -> Self {
        match s {
            "" => ThreadSafety::Unsafe,
            "ReadSafe" => ThreadSafety::ReadSafe,
            "Unsafe" => ThreadSafety::Unsafe,
            "Safe" => ThreadSafety::Safe,
            other => ThreadSafety::Unknown(other),
        }
    }
}

/// Whether a property is loadable/savable in serialized places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Serialization {
    pub can_load: bool,
    pub can_save: bool,
}

/// A function parameter or return value.
#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    pub name: &'static str,
    pub type_name: &'static str,
    pub default: Option<&'static str>,
}

/// A class or instance property.
#[derive(Debug, Clone, Copy)]
pub struct PropDef {
    pub name: &'static str,
    pub type_name: &'static str,
    pub read_security: Security,
    pub write_security: Security,
    pub thread_safety: ThreadSafety,
    pub tags: &'static [&'static str],
    pub serialization: Serialization,
    pub category: &'static str,
    pub deprecated: bool,
}

impl PropDef {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag)
    }
    pub fn is_read_only(&self) -> bool {
        self.has_tag("ReadOnly")
    }
    pub fn is_not_scriptable(&self) -> bool {
        self.has_tag("NotScriptable")
    }
}

/// A class method or callback.
#[derive(Debug, Clone, Copy)]
pub struct MethodDef {
    pub name: &'static str,
    pub params: &'static [ParamDef],
    pub returns: &'static [ParamDef],
    pub security: Security,
    pub thread_safety: ThreadSafety,
    pub tags: &'static [&'static str],
    pub deprecated: bool,
}

impl MethodDef {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag)
    }
    pub fn is_yielding(&self) -> bool {
        self.has_tag("Yields")
    }
}

/// A class event.
#[derive(Debug, Clone, Copy)]
pub struct EventDef {
    pub name: &'static str,
    pub params: &'static [ParamDef],
    pub security: Security,
    pub tags: &'static [&'static str],
    pub deprecated: bool,
}

impl EventDef {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag)
    }
}

/// A class definition from the Roblox engine reference.
#[derive(Debug, Clone, Copy)]
pub struct ClassDef {
    pub name: &'static str,
    /// The parent class, or empty string for `Instance`.
    pub inherits: &'static str,
    pub tags: &'static [&'static str],
    pub properties: &'static [PropDef],
    pub methods: &'static [MethodDef],
    pub events: &'static [EventDef],
    pub callbacks: &'static [MethodDef],
}

impl ClassDef {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag)
    }
    /// Whether `self` is `ancestor` or inherits from it.
    pub fn is_a(&self, ancestor: &str, lookup: impl Fn(&str) -> Option<&'static ClassDef>) -> bool {
        let mut cur = Some(*self);
        while let Some(c) = cur {
            if c.name == ancestor {
                return true;
            }
            if c.inherits.is_empty() {
                return false;
            }
            cur = lookup(c.inherits).copied();
        }
        false
    }
}

/// A single item of an enum.
#[derive(Debug, Clone, Copy)]
pub struct EnumItemDef {
    pub name: &'static str,
    pub value: i64,
    pub deprecated: bool,
}

/// An enum definition from the Roblox engine reference.
#[derive(Debug, Clone, Copy)]
pub struct EnumDef {
    pub name: &'static str,
    pub items: &'static [EnumItemDef],
}

/// A datatype property or constant (name + type only).
#[derive(Debug, Clone, Copy)]
pub struct DtPropDef {
    pub name: &'static str,
    pub type_name: &'static str,
}

/// A datatype constructor, method, or function.
#[derive(Debug, Clone, Copy)]
pub struct DtMethodDef {
    pub name: &'static str,
    pub params: &'static [ParamDef],
    pub returns: &'static [ParamDef],
    pub tags: &'static [&'static str],
    pub deprecated: bool,
}

/// An operator overload supported by a datatype.
#[derive(Debug, Clone, Copy)]
pub struct MathOpDef {
    pub operation: &'static str,
    pub type_a: &'static str,
    pub type_b: &'static str,
    pub return_type: &'static str,
    pub deprecated: bool,
}

/// A datatype definition from the Roblox engine reference.
#[derive(Debug, Clone, Copy)]
pub struct DatatypeDef {
    pub name: &'static str,
    pub constants: &'static [DtPropDef],
    pub properties: &'static [DtPropDef],
    pub constructors: &'static [DtMethodDef],
    pub methods: &'static [DtMethodDef],
    pub functions: &'static [DtMethodDef],
    pub math_operations: &'static [MathOpDef],
}

/// A library (e.g. `task`, `string`) or globals table definition.
#[derive(Debug, Clone, Copy)]
pub struct LibraryDef {
    pub name: &'static str,
    pub properties: &'static [DtPropDef],
    pub functions: &'static [MethodDef],
}

/// A globals table definition (e.g. `"Luau globals"`, `"Roblox globals"`).
/// Shares the same shape as a library.
pub type GlobalDef = LibraryDef;
