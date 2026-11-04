#pragma once

// The image catalog: which OS images the disk step offers, and what each
// choice implies for the rest of the wizard.
//
// The GNOME frontend reads the same file (bootc_installer/defaults/image.py:
// `data/images.json` bundled, `/etc/bootc-installer/images.json` and
// `$XDG_CONFIG_HOME` overrides) and the XFCE frontend resolves the same
// inheritance in `core.ImageNode`. This reader mirrors both: a tree of groups
// and leaves where every per-image field inherits from the nearest ancestor
// that sets it, a leaf may use `tag` + an in-scope `registry` instead of a
// full `imgref`, and `@alias` values resolve against the manifest's top-level
// `aliases` map before anything else reads it.
//
// The one decision this file makes on its own: a leaf without
// `needs_user_creation` defaults to TRUE. The GNOME tree defaults it to false
// at the top level, but its user step falls back to showing (true) whenever
// there is no usable image step, and the XFCE node defaults to true outright.
// Defaulting to true is the safe side — an image the catalog does not describe
// installs with an account step rather than with nobody to sign in as — and
// it matches the account step's standing rule (there is no catalog here that
// says an image creates its own first user, so the account is required).

#include <QByteArray>
#include <QList>
#include <QString>
#include <QStringList>
#include <QVariantList>
#include <QVariantMap>

namespace imagecatalog
{

// Test seam: a file holding an images.json manifest. When set, no search
// path is read. Every frontend honours an equivalent variable; the XFCE one
// is FISHERMAN_IMAGES_PATH, so this reuses that name.
inline constexpr const char *IMAGES_ENV = "FISHERMAN_IMAGES_PATH";

// One selectable image: a leaf of the manifest tree with every inherited
// field resolved.
struct Leaf {
    QString name;              // group-qualified display name ("Aurora DX — Stable")
    QString imgref;            // full OCI reference, tag+registry already composed
    QString desc;              // the leaf's `desc`, may be empty
    QString searchText;        // lowercased "group names + name + desc + imgref" for filtering
    QString icon;              // inherited `icon` spec, may be empty
    // Per-leaf flatpaks: a JSON list, a URL to fetch, or empty (fisherman's
    // fallback then applies — the same default the GNOME step starts from).
    QString flatpakSource;
    bool needsUserCreation = true;
    bool composefs = false;
    QString imageType;         // "bootc", "ostree", or "" (let fisherman decide)
    QString bootloader;        // "grub2", "systemd", or ""
    QString imageFilesystem;   // filesystem the image requires, or ""
    QString flatpakVarPath;    // override for the writable var path, or ""
    QString defaultHostname;   // suggested hostname, or ""
    QStringList filesystems;   // user-selectable filesystems, or empty
    QString prettyName;        // catalog identity for the confirm screen
    QVariantList carousel; // the leaf's `carousel` slides, as QVariantMaps
};

struct Catalog {
    QString defaultImage;
    QStringList fallbackFlatpaks;
    QList<Leaf> leaves;
};

// Where load() looks, in order. Mirrors the XFCE IMAGES_JSON_PATHS plus the
// GNOME system override, with the Flatpak host /etc first when sandboxed
// (the same prefix RecipeLoader applies to its own /etc reads).
QStringList defaultSearchPaths(bool flatpak);

// Parses one manifest document. Pure: no filesystem, no environment.
Catalog parse(const QByteArray &json);

// Loads the catalog: IMAGES_ENV first, then defaultSearchPaths(). A missing
// or unreadable manifest is an empty catalog, not an error — live media and
// branding-default installs proceed without one, as they do today.
Catalog load();

} // namespace imagecatalog
