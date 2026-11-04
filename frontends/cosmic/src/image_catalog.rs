//! Image catalog: which bootc images can be installed, and whether the
//! installed system needs this frontend to create its first user.
//!
//! This mirrors `bootc_installer/defaults/image.py` (`needs_user_creation`
//! inheritance, manifest search order) and the XFCE `SourcePage` catalog
//! pattern (`frontends/xfce/.../core.py::load_catalog`, `ImageNode::leaves`),
//! adapted to Rust/libcosmic: the tree is plain data, and the wizard renders
//! it with cosmic widgets instead of GTK expander rows.
//!
//! Semantics (measured from `image.py::__build_node`):
//! - `needs_user_creation` is inherited from the nearest ancestor that sets
//!   it; the root default is `false` (the `needs_user_ctx=False` parameter
//!   default). A leaf without the key and without an ancestor that sets it
//!   does NOT require a user.
//! - A custom URL image never requires a user (`get_finals`, custom branch).
//! - The page is skipped when fewer than two leaves are selectable
//!   (`skip_screen`: `leaf_count <= 1`).
//!
//! The one deliberate seam: with no catalog at all (or no selection yet) this
//! frontend requires the account, as it always has — there is no image on
//! record to say otherwise. `account_required()` on the app encodes that;
//! only a *selected leaf* (or custom URL) can lift the requirement.

use serde_json::Value;

/// One installable image: a leaf of the manifest tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageLeaf {
    /// Display name (`name`, falling back to the imgref).
    pub name: String,
    /// Short description, when the manifest carries one.
    pub description: String,
    /// The bootc ref written to the recipe's `image`.
    pub imgref: String,
    /// Whether the installed system needs a user created for it.
    pub needs_user_creation: bool,
    /// Per-image tour slides (`carousel`), if the manifest carries any.
    /// Parsed by `crate::tour` only when this leaf is selected.
    pub carousel: Vec<Value>,
    /// Flatpaks installed with this image (inherited, as in `__build_node`).
    pub flatpaks: Vec<String>,
}

/// Where the manifest is looked for, in order. Mirrors the XFCE
/// `IMAGES_JSON_PATHS` plus `image.py`'s `/etc/bootc-installer/images.json`
/// override (with the Flatpak `/run/host/etc` prefix `image.py` applies, so
/// a live ISO's catalog is found from inside the sandbox).
pub fn manifest_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(p) = std::env::var("FISHERMAN_IMAGES_PATH") {
        if !p.is_empty() {
            paths.push(p);
        }
    }
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    for leaf in ["tuna-installer/images.json", "bootc-installer/images.json"] {
        paths.push(format!("{config_home}/{leaf}"));
    }
    // Inside a Flatpak sandbox /etc is the runtime's; the host's is visible
    // at /run/host/etc (image.py applies the same prefix).
    let etc = if std::path::Path::new("/.flatpak-info").exists() {
        "/run/host/etc"
    } else {
        "/etc"
    };
    for leaf in ["tuna-installer/images.json", "bootc-installer/images.json"] {
        paths.push(format!("{etc}/{leaf}"));
    }
    paths.push("/app/share/fisherman/data/images.json".into());
    paths.push("/usr/share/fisherman/data/images.json".into());
    paths
}

/// Recursively replace `@alias` strings with values from the manifest's
/// `aliases` table. Runs once at load time, as in `image.py::_resolve_aliases`:
/// unknown aliases are left alone so a later stage can report them.
fn resolve_aliases(value: &mut Value, aliases: &serde_json::Map<String, Value>) {
    match value {
        Value::String(s) if s.starts_with('@') => {
            if let Some(replacement) = aliases.get(&s[1..]) {
                *value = replacement.clone();
            }
        }
        Value::Array(items) => {
            for item in items {
                resolve_aliases(item, aliases);
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                resolve_aliases(v, aliases);
            }
        }
        _ => {}
    }
}

fn node_imgref(node: &serde_json::Map<String, Value>) -> String {
    if let Some(r) = node.get("imgref").and_then(|v| v.as_str()) {
        if !r.is_empty() {
            return r.to_string();
        }
    }
    // Older manifests split the ref into registry+tag (XFCE ImageNode).
    let registry = node.get("registry").and_then(|v| v.as_str()).unwrap_or("");
    let tag = node.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    if !registry.is_empty() && !tag.is_empty() {
        return format!("{registry}:{tag}");
    }
    String::new()
}

/// Flatten one manifest node into leaves, inheriting `needs_user_creation`,
/// the carousel and the flatpak list from the nearest ancestor that defines
/// each (`__build_node` inherits every per-image field the same way).
fn collect_leaves(
    node: &Value,
    needs_user_ctx: bool,
    carousel_ctx: &[Value],
    flatpaks_ctx: &[String],
    out: &mut Vec<ImageLeaf>,
) {
    let Some(obj) = node.as_object() else { return };
    let needs_user = obj
        .get("needs_user_creation")
        .and_then(|v| v.as_bool())
        .unwrap_or(needs_user_ctx);
    let carousel = obj
        .get("carousel")
        .and_then(|v| v.as_array())
        .map(|a| a.to_vec())
        .unwrap_or_else(|| carousel_ctx.to_vec());
    let flatpaks = obj
        .get("flatpaks")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(|| flatpaks_ctx.to_vec());
    if let Some(children) = obj.get("children").and_then(|v| v.as_array()) {
        for child in children {
            collect_leaves(child, needs_user, &carousel, &flatpaks, out);
        }
    }
    let imgref = node_imgref(obj);
    if imgref.is_empty() {
        return;
    }
    let has_children = obj
        .get("children")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    // A leaf offers an image and has no children of its own (XFCE is_leaf).
    if has_children {
        return;
    }
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&imgref)
        .to_string();
    let description = obj
        .get("description")
        .or_else(|| obj.get("desc"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    out.push(ImageLeaf {
        name,
        description,
        imgref,
        needs_user_creation: needs_user,
        carousel,
        flatpaks,
    });
}

/// The parsed catalog: the default ref, the fallback flatpaks, and the
/// selectable leaves in manifest order.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub default_image: String,
    pub fallback_flatpaks: Vec<String>,
    pub leaves: Vec<ImageLeaf>,
}

/// Parse a manifest document. Pure: the search order lives in
/// [`manifest_paths`], IO in [`load_catalog`].
pub fn parse_manifest(text: &str) -> Result<Catalog, String> {
    let mut root: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid images.json: {e}"))?;
    let aliases = root
        .get("aliases")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    resolve_aliases(&mut root, &aliases);
    let default_image = root
        .get("default_image")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let fallback_flatpaks: Vec<String> = root
        .get("fallback_flatpaks")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut leaves = Vec::new();
    if let Some(images) = root.get("images").and_then(|v| v.as_array()) {
        for node in images {
            collect_leaves(node, false, &[], &fallback_flatpaks, &mut leaves);
        }
    }
    Ok(Catalog {
        default_image,
        fallback_flatpaks,
        leaves,
    })
}

/// Load the first manifest found on [`manifest_paths`]. `None` when no file
/// exists or none parses — the wizard then falls back to the branding
/// default image with the account required.
pub fn load_catalog() -> Option<Catalog> {
    for path in manifest_paths() {
        let text = std::fs::read_to_string(&path).ok()?;
        match parse_manifest(&text) {
            Ok(catalog) => return Some(catalog),
            Err(e) => {
                tracing::warn!("ignoring unparsable catalog {path}: {e}");
            }
        }
    }
    None
}

impl Catalog {
    /// Index of the leaf matching the default ref, if any (mirrors
    /// `__select_default`).
    pub fn default_index(&self) -> Option<usize> {
        if self.default_image.is_empty() {
            return None;
        }
        self.leaves
            .iter()
            .position(|l| l.imgref == self.default_image)
    }

    /// Whether the image page earns its place: two or more selectable
    /// leaves (`skip_screen` in image.py).
    pub fn has_choice(&self) -> bool {
        self.leaves.len() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "default_image": "ghcr.io/example/desktop:stable",
        "fallback_flatpaks": ["org.mozilla.firefox"],
        "aliases": {"stable_tag": "stable"},
        "images": [
            {"name": "Group", "needs_user_creation": true, "children": [
                {"name": "Desktop", "imgref": "ghcr.io/example/desktop:stable"},
                {"name": "Minimal", "registry": "ghcr.io/example/minimal", "tag": "@stable_tag",
                 "description": "No frills", "needs_user_creation": false,
                 "carousel": [{"title": "Hi", "description": "Welcome", "image": "a.png"}]}
            ]},
            {"name": "Server", "imgref": "ghcr.io/example/server:stable"}
        ]
    }"#;

    #[test]
    fn leaves_inherit_needs_user_from_the_nearest_ancestor() {
        let catalog = parse_manifest(MANIFEST).unwrap();
        assert_eq!(catalog.leaves.len(), 3);
        // Inherited from Group.
        assert!(catalog.leaves[0].needs_user_creation);
        // Overridden on the leaf itself.
        assert!(!catalog.leaves[1].needs_user_creation);
        // No ancestor sets it: the image.py root default, false.
        assert!(!catalog.leaves[2].needs_user_creation);
    }

    #[test]
    fn aliases_resolve_before_leaves_are_read() {
        let catalog = parse_manifest(MANIFEST).unwrap();
        assert_eq!(catalog.leaves[1].imgref, "ghcr.io/example/minimal:stable");
    }

    #[test]
    fn flatpaks_inherit_with_fallback_at_the_root() {
        // get_finals: the leaf's list, else the manifest fallback.
        let catalog = parse_manifest(
            r#"{"fallback_flatpaks": ["org.example.Fallback"],
                "images": [
                    {"name": "Plain", "imgref": "x:1"},
                    {"name": "Group", "flatpaks": ["org.example.Group"], "children": [
                        {"name": "Child", "imgref": "x:2"},
                        {"name": "Own", "imgref": "x:3", "flatpaks": ["org.example.Own"]}
                    ]}
                ]}"#,
        )
        .unwrap();
        let by_name = |n: &str| {
            catalog
                .leaves
                .iter()
                .find(|l| l.name == n)
                .unwrap()
                .flatpaks
                .clone()
        };
        assert_eq!(by_name("Plain"), vec!["org.example.Fallback"]);
        assert_eq!(by_name("Child"), vec!["org.example.Group"]);
        assert_eq!(by_name("Own"), vec!["org.example.Own"]);
    }

    #[test]
    fn only_a_whole_string_is_an_alias() {
        // image.py replaces a string that STARTS WITH @; an @ in the
        // middle of a longer value is literal and survives loading.
        let catalog = parse_manifest(
            r#"{"aliases": {"t": "stable"},
                "images": [{"name": "A", "imgref": "x:@t"},
                           {"name": "B", "imgref": "@t"}]}"#,
        )
        .unwrap();
        assert_eq!(catalog.leaves[0].imgref, "x:@t");
        assert_eq!(catalog.leaves[1].imgref, "stable");
    }

    #[test]
    fn carousel_and_description_ride_along() {
        let catalog = parse_manifest(MANIFEST).unwrap();
        assert_eq!(catalog.leaves[1].description, "No frills");
        assert_eq!(catalog.leaves[1].carousel.len(), 1);
        assert!(catalog.leaves[0].carousel.is_empty());
    }

    #[test]
    fn default_and_fallbacks_are_reported() {
        let catalog = parse_manifest(MANIFEST).unwrap();
        assert_eq!(catalog.default_image, "ghcr.io/example/desktop:stable");
        assert_eq!(catalog.fallback_flatpaks, vec!["org.mozilla.firefox"]);
        assert_eq!(catalog.default_index(), Some(0));
    }

    #[test]
    fn registry_plus_tag_leaves_are_understood() {
        let catalog = parse_manifest(
            r#"{"images": [{"name": "Old", "registry": "ghcr.io/example/old", "tag": "lts"}]}"#,
        )
        .unwrap();
        assert_eq!(catalog.leaves.len(), 1);
        assert_eq!(catalog.leaves[0].imgref, "ghcr.io/example/old:lts");
    }

    #[test]
    fn one_leaf_is_no_choice() {
        let one = parse_manifest(r#"{"images": [{"name": "Only", "imgref": "x:y"}]}"#).unwrap();
        assert!(!one.has_choice());
        let two = parse_manifest(
            r#"{"images": [{"name": "A", "imgref": "x:y"}, {"name": "B", "imgref": "x:z"}]}"#,
        )
        .unwrap();
        assert!(two.has_choice());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_manifest("not json").is_err());
        // A wrongly-shaped tree is an empty catalog, not an error: the
        // wizard falls back to the branding default with the account
        // required, instead of failing to start.
        assert!(parse_manifest(r#"{"images": {}}"#)
            .unwrap()
            .leaves
            .is_empty());
    }

    #[test]
    fn load_reads_the_first_existing_manifest() {
        let dir = std::env::temp_dir().join(format!("tuna-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("images.json");
        std::fs::write(&path, MANIFEST).unwrap();
        // SAFETY: single-threaded test; read back immediately.
        unsafe { std::env::set_var("FISHERMAN_IMAGES_PATH", &path) };
        let catalog = load_catalog().expect("manifest");
        assert_eq!(catalog.leaves.len(), 3);
        unsafe { std::env::remove_var("FISHERMAN_IMAGES_PATH") };
        std::fs::remove_dir_all(&dir).ok();
    }
}
