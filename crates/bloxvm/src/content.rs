//! Asset downloading for `rbxassetid://` Content URIs and local `rbxasset://`
//! fallback resolution.
//!
//! Roblox places reference external textures via `Content` properties
//! (`rbxassetid://12345678`). This module scans a loaded [`DataModel`] for
//! those URIs, downloads them from the Roblox CDN, and caches them for the
//! renderer.
//!
//! Built-in Roblox paths (`rbxasset://textures/...`) cannot be downloaded from
//! the CDN. Instead, bundled copies shipped in `assets/textures/` are resolved
//! locally at compile time via [`include_bytes!`].

use std::collections::BTreeMap;
use std::io::Write;

use crate::instance::DataModel;
use crate::value::Value;

const ASSET_DELIVERY_URL: &str = "https://assetdelivery.roblox.com/v1/asset";

// ---------------------------------------------------------------------------
// Bundled local assets  (`rbxasset://textures/...`)
// ---------------------------------------------------------------------------

/// Bundled assets mapped by the path portion after `rbxasset://`.
/// Add new entries here as more Roblox built-in textures are needed.
fn local_asset_bytes(path: &str) -> Option<&'static [u8]> {
    match path.to_lowercase().as_str() {
        "textures/spawnlocation.png" => Some(include_bytes!("../assets/textures/SpawnLocation.png")),
        _ => None,
    }
}

/// Attempts to resolve a Content URI as a local (bundled) asset.
/// Returns `Some(bytes)` for `rbxasset://` URIs that have a bundled copy.
pub fn resolve_local_asset(uri: &str) -> Option<&'static [u8]> {
    let path = uri.strip_prefix("rbxasset://")?;
    local_asset_bytes(path)
}

/// Parsed representation of an `rbxassetid://` URI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetUri {
    pub scheme: String,
    pub id: u64,
}

/// Extracts an asset ID from a Content URI. Returns the numeric ID for
/// `rbxassetid://` schemes; returns `None` for other schemes or malformed
/// URIs.
pub fn parse_asset_id(uri: &str) -> Option<u64> {
    let uri = uri.trim();
    // Strip the scheme: "rbxassetid://12345678" → "12345678"
    let id_str = uri
        .strip_prefix("rbxassetid://")
        .or_else(|| uri.strip_prefix("rbxassetid:"))?;
    id_str.parse().ok()
}

/// Downloads a Roblox asset by ID from the asset delivery CDN.
///
/// Returns the raw response bytes (which may be DDS, mesh, audio, etc.).
/// The caller is responsible for interpreting the format.
pub fn download_asset(asset_id: u64) -> Result<Vec<u8>, String> {
    let url = format!("{ASSET_DELIVERY_URL}?id={asset_id}");

    let response = ureq::get(&url)
        .call()
        .map_err(|e| format!("HTTP request failed for asset {asset_id}: {e}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read response for asset {asset_id}: {e}"))?;

    Ok(bytes)
}

/// Scans a [`DataModel`] for `Value::Content` properties on
/// `SurfaceAppearance` children and `Part.Texture` properties of `BasePart`
/// instances, collects unique `rbxassetid://` URIs, downloads them, and
/// returns a map of URI → bytes.
///
/// Non-downloadable URIs (empty, `rbxasset://`, etc.) and download failures
/// are skipped with a warning. The caller can feed the returned map into the
/// texture pipeline.
pub fn resolve_surface_appearance_textures(dm: &DataModel) -> BTreeMap<String, Vec<u8>> {
    let mut uris: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for inst in &dm.instances {
        if !inst.is_a("BasePart") || inst.class == "Terrain" {
            continue;
        }

        // Scan children for downloadable Content texture URIs.
        for &child_id in &inst.children {
            let child = &dm.instances[child_id];
            let uri_opt = match child.class.as_str() {
                "SurfaceAppearance" => child.get_property("ColorMap"),
                "Texture" => child.get_property("Texture"),
                "Decal" => child.get_property("Texture"),
                _ => None,
            };
            if let Some(Value::Content(uri)) = uri_opt {
                if !uri.is_empty() && seen.insert(uri.clone()) {
                    uris.push(uri.clone());
                }
            }
        }

        // Also check the part's own Texture property.
        if let Some(Value::Content(uri)) = inst.get_property("Texture") {
            if !uri.is_empty() && seen.insert(uri.clone()) {
                uris.push(uri.clone());
            }
        }
    }

    if uris.is_empty() {
        return BTreeMap::new();
    }

    println!("resolving {} Content texture URI(s)...", uris.len());
    let mut cache = BTreeMap::new();
    let total = uris.len();
    for (i, uri) in uris.iter().enumerate() {
        // 1. Try local bundled asset first (rbxasset://textures/...).
        if let Some(bytes) = resolve_local_asset(uri) {
            let bar_width = 30;
            let filled = ((i + 1) * bar_width / total).max(1);
            let bar: String = "#".repeat(filled) + &"-".repeat(bar_width.saturating_sub(filled));
            eprint!("\r  [{bar}] {}/{} local: {uri}   ", i + 1, total);
            let _ = std::io::stderr().flush();
            cache.insert(uri.clone(), bytes.to_vec());
            continue;
        }

        // 2. Try downloading from the Roblox CDN (rbxassetid://...).
        if let Some(asset_id) = parse_asset_id(uri) {
            let bar_width = 30;
            let filled = ((i + 1) * bar_width / total).max(1);
            let bar: String = "#".repeat(filled) + &"-".repeat(bar_width.saturating_sub(filled));
            eprint!("\r  [{bar}] {}/{} rbxassetid://{asset_id}   ", i + 1, total);
            let _ = std::io::stderr().flush();
            match download_asset(asset_id) {
                Ok(bytes) => {
                    cache.insert(uri.clone(), bytes);
                }
                Err(e) => {
                    eprintln!("\n  warning: {e}");
                }
            }
        } else {
            eprintln!("  warning: skipping non-downloadable URI: {uri}");
        }
    }
    if total > 0 {
        let bar = "#".repeat(30);
        eprintln!("\r  [{bar}] {total}/{total} done.                    ");
    }
    cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rbxassetid() {
        assert_eq!(parse_asset_id("rbxassetid://12345678"), Some(12345678));
        assert_eq!(parse_asset_id("rbxassetid://0"), Some(0));
        assert_eq!(parse_asset_id("rbxassetid:12345"), Some(12345));
    }

    #[test]
    fn rejects_non_assetid_uris() {
        assert_eq!(parse_asset_id("rbxasset://textures/foo.png"), None);
        assert_eq!(parse_asset_id(""), None);
        assert_eq!(parse_asset_id("https://example.com"), None);
    }

    #[test]
    fn parses_rbxassetid_with_whitespace() {
        assert_eq!(parse_asset_id("  rbxassetid://42  "), Some(42));
    }
}
