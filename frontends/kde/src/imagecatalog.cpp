#include "imagecatalog.h"

#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonValue>
#include <QProcessEnvironment>
#include <QStandardPaths>
#include <QVariant>

namespace imagecatalog
{

namespace
{

// The manifest's own words for the search path the XFCE frontend uses:
// $FISHERMAN_IMAGES_PATH, then the XDG config override, then the system
// paths, then fisherman's installed data. The GNOME frontend checks the same
// user and system overrides for its copy of the file.
QStringList searchPaths(bool flatpak, const QProcessEnvironment &env)
{
    QStringList paths;
    const QString envPath = env.value(QStringLiteral("FISHERMAN_IMAGES_PATH"));
    if (!envPath.isEmpty())
        paths << envPath;

    const QString xdgConfig = env.value(
        QStringLiteral("XDG_CONFIG_HOME"),
        QStandardPaths::writableLocation(QStandardPaths::HomeLocation)
            + QStringLiteral("/.config"));
    paths << QDir(xdgConfig).filePath(QStringLiteral("tuna-installer/images.json"));

    // Inside a Flatpak the runtime's /etc is not the host's; the host side
    // is at /run/host/etc, the same prefix the GNOME RecipeLoader applies.
    // Both manifest names: tuna-installer (this/XFCE frontend) and
    // bootc-installer (the GNOME user step's live-ISO fallback).
    if (flatpak) {
        paths << QStringLiteral("/run/host/etc/tuna-installer/images.json");
        paths << QStringLiteral("/run/host/etc/bootc-installer/images.json");
    }
    paths << QStringLiteral("/etc/tuna-installer/images.json");
    paths << QStringLiteral("/etc/bootc-installer/images.json");
    paths << QStringLiteral("/app/share/fisherman/data/images.json");
    paths << QStringLiteral("/usr/share/fisherman/data/images.json");
    return paths;
}

// GNOME's alias pass: any string value starting with '@' names an entry of
// the manifest's top-level `aliases` map. Unknown aliases stay as-is so a
// later stage can report them, exactly as image.py does.
void resolveAliases(QJsonValue &value, const QJsonObject &aliases);
void resolveAliases(QJsonObject &obj, const QJsonObject &aliases)
{
    for (auto it = obj.begin(); it != obj.end(); ++it) {
        if (it.value().isString()) {
            const QString s = it.value().toString();
            if (s.startsWith(QLatin1Char('@'))) {
                const QJsonValue resolved = aliases.value(s.mid(1));
                if (!resolved.isUndefined())
                    it.value() = resolved;
            }
        } else if (it.value().isObject()) {
            QJsonObject child = it.value().toObject();
            resolveAliases(child, aliases);
            it.value() = child;
        } else if (it.value().isArray()) {
            // Index assignment, not a range-for: the loop variable would be
            // a copy, and resolving the copy resolves nothing.
            QJsonArray child = it.value().toArray();
            for (int i = 0; i < child.size(); ++i) {
                QJsonValue v = child.at(i);
                resolveAliases(v, aliases);
                child[i] = v;
            }
            it.value() = child;
        }
    }
}
void resolveAliases(QJsonValue &value, const QJsonObject &aliases)
{
    if (value.isObject()) {
        QJsonObject child = value.toObject();
        resolveAliases(child, aliases);
        value = child;
    } else if (value.isArray()) {
        QJsonArray child = value.toArray();
        for (int i = 0; i < child.size(); ++i) {
            QJsonValue v = child.at(i);
            resolveAliases(v, aliases);
            child[i] = v;
        }
        value = child;
    }
}

QString prettyName(const QString &name, const QString &imgref)
{
    // The GNOME _catalog_pretty_name rule: keep the catalog's name, derive
    // one from the ref only as a fallback.
    if (!name.trimmed().isEmpty())
        return name;
    const QString slug = imgref.split(QLatin1Char('/')).value(-1).split(QLatin1Char(':')).value(0);
    if (slug.isEmpty())
        return imgref;
    QStringList words = QString(slug).replace(QLatin1Char('_'), QLatin1Char('-')).split(QLatin1Char('-'));
    for (int i = 0; i < words.size(); ++i) {
        if (i == 0)
            words[i] = words[i].left(1).toUpper() + words[i].mid(1).toLower();
        else
            words[i] = words[i].toUpper();
    }
    return words.join(QLatin1Char(' '));
}

// The inherited context a group passes to its children: every per-image
// field the GNOME __build_node threads through, with the same field names
// the manifest uses.
struct Context {
    QString flatpaks; // inline JSON list, a URL string, or empty
    QVariantList carousel; // group slides, handed down like flatpaks
    QString icon;
    bool needsUserCreation = true; // see the header for why true, not false
    bool composefs = false;
    QString imageType = QStringLiteral("bootc");
    QString bootloader;
    QString imageFilesystem;
    QString flatpakVarPath;
    QString registry;
    QString registryName; // nearest registry-owning node's name (the product)
    QString defaultHostname;
    QStringList filesystems;
};

void collectLeaves(const QJsonObject &node, const QStringList &trail, const Context &ctx,
                   const QString &searchCtx, QList<Leaf> &out)
{
    Context child = ctx;
    // A group node carries the slides for all its leaves (Aurora's manifest
    // does); a leaf with its own keeps it. Inherited first, then the node's
    // own replaces it — the same rule as every other inherited field.
    if (node.contains(QStringLiteral("carousel"))) {
        child.carousel.clear();
        for (const QJsonValue &v : node.value(QStringLiteral("carousel")).toArray()) {
            if (v.isObject())
                child.carousel << QVariant(v.toObject().toVariantMap());
        }
    }
    if (node.contains(QStringLiteral("flatpaks"))) {
        const QJsonValue v = node.value(QStringLiteral("flatpaks"));
        child.flatpaks = v.isString() ? v.toString()
                                      : QString::fromUtf8(QJsonDocument(v.toArray()).toJson(QJsonDocument::Compact));
    }
    if (node.contains(QStringLiteral("icon")))
        child.icon = node.value(QStringLiteral("icon")).toString();
    if (node.contains(QStringLiteral("needs_user_creation")))
        child.needsUserCreation = node.value(QStringLiteral("needs_user_creation")).toBool(true);
    if (node.contains(QStringLiteral("composefs")))
        child.composefs = node.value(QStringLiteral("composefs")).toBool(false);
    if (node.contains(QStringLiteral("image_type")))
        child.imageType = node.value(QStringLiteral("image_type")).toString();
    if (node.contains(QStringLiteral("bootloader")))
        child.bootloader = node.value(QStringLiteral("bootloader")).toString();
    if (node.contains(QStringLiteral("filesystem")))
        child.imageFilesystem = node.value(QStringLiteral("filesystem")).toString();
    if (node.contains(QStringLiteral("flatpak_var_path")))
        child.flatpakVarPath = node.value(QStringLiteral("flatpak_var_path")).toString();
    if (node.contains(QStringLiteral("registry"))) {
        child.registry = node.value(QStringLiteral("registry")).toString();
        child.registryName = node.value(QStringLiteral("name")).toString();
    }
    if (node.contains(QStringLiteral("default_hostname")))
        child.defaultHostname = node.value(QStringLiteral("default_hostname")).toString();
    if (node.contains(QStringLiteral("filesystems"))) {
        child.filesystems.clear();
        for (const QJsonValue &v : node.value(QStringLiteral("filesystems")).toArray())
            child.filesystems << v.toString();
    }

    // A tag leaf without an imgref composes one from the in-scope registry,
    // as image.py does; without either it is not selectable and is skipped.
    QString imgref = node.value(QStringLiteral("imgref")).toString();
    if (imgref.isEmpty() && node.contains(QStringLiteral("tag")) && !child.registry.isEmpty())
        imgref = child.registry + QLatin1Char(':') + node.value(QStringLiteral("tag")).toString();

    // A node with an imgref is a selectable leaf, even with children —
    // image.py adds the leaf and returns without descending.
    if (!imgref.isEmpty()) {
        Leaf leaf;
        const QString name = node.value(QStringLiteral("name")).toString();
        // Group-qualified display name, so two groups' "Stable" rows differ.
        leaf.name = trail.isEmpty() ? name : trail.join(QStringLiteral(" — ")) + QStringLiteral(" — ") + name;
        leaf.imgref = imgref;
        leaf.desc = node.value(QStringLiteral("desc")).toString();
        // The GNOME search string: ancestors, name, description and ref.
        leaf.searchText = (searchCtx + QLatin1Char(' ') + name
                           + QLatin1Char(' ') + leaf.desc
                           + QLatin1Char(' ') + imgref).toLower();
        leaf.icon = node.value(QStringLiteral("icon")).toString(child.icon);
        leaf.flatpakSource = node.contains(QStringLiteral("flatpaks"))
            ? (node.value(QStringLiteral("flatpaks")).isString()
                   ? node.value(QStringLiteral("flatpaks")).toString()
                   : QString::fromUtf8(QJsonDocument(node.value(QStringLiteral("flatpaks")).toArray())
                                           .toJson(QJsonDocument::Compact)))
            : child.flatpaks;
        leaf.needsUserCreation = node.value(QStringLiteral("needs_user_creation")).toBool(child.needsUserCreation);
        leaf.composefs = node.value(QStringLiteral("composefs")).toBool(child.composefs);
        leaf.imageType = node.value(QStringLiteral("image_type")).toString(child.imageType);
        leaf.bootloader = node.value(QStringLiteral("bootloader")).toString(child.bootloader);
        leaf.imageFilesystem = node.value(QStringLiteral("filesystem")).toString(child.imageFilesystem);
        leaf.flatpakVarPath = node.value(QStringLiteral("flatpak_var_path")).toString(child.flatpakVarPath);
        leaf.defaultHostname = node.value(QStringLiteral("default_hostname")).toString(child.defaultHostname);
        leaf.filesystems = child.filesystems;
        if (node.contains(QStringLiteral("filesystems"))) {
            leaf.filesystems.clear();
            for (const QJsonValue &v : node.value(QStringLiteral("filesystems")).toArray())
                leaf.filesystems << v.toString();
        }
        // The GNOME pretty-name rule: a tag leaf is a channel, not the
        // product, so it is named for the registry-owning node.
        const QString identity = (!node.contains(QStringLiteral("imgref")) && !child.registryName.isEmpty())
            ? child.registryName
            : name;
        leaf.prettyName = prettyName(identity, imgref);
        leaf.carousel = child.carousel;
        out << leaf;
        return;
    }

    QStringList childTrail = trail;
    const QString nodeName = node.value(QStringLiteral("name")).toString();
    if (!nodeName.isEmpty())
        childTrail << nodeName;
    QString childSearch = searchCtx + QLatin1Char(' ') + nodeName;
    const QString extra = node.value(QStringLiteral("search_extra")).toString();
    if (!extra.isEmpty())
        childSearch += QLatin1Char(' ') + extra;
    for (const QJsonValue &v : node.value(QStringLiteral("children")).toArray()) {
        if (v.isObject())
            collectLeaves(v.toObject(), childTrail, child, childSearch, out);
    }
}

} // namespace

QStringList defaultSearchPaths(bool flatpak)
{
    return searchPaths(flatpak, QProcessEnvironment::systemEnvironment());
}

Catalog parse(const QByteArray &json)
{
    Catalog catalog;
    QJsonParseError err{};
    const QJsonDocument doc = QJsonDocument::fromJson(json, &err);
    if (err.error != QJsonParseError::NoError || !doc.isObject())
        return catalog;
    QJsonObject root = doc.object();
    const QJsonObject aliases = root.value(QStringLiteral("aliases")).toObject();
    if (!aliases.isEmpty())
        resolveAliases(root, aliases);
    catalog.defaultImage = root.value(QStringLiteral("default_image")).toString();
    // fallback_flatpaks may be a list or an @alias-resolved string; keep it simple.
    for (const QJsonValue &v : root.value(QStringLiteral("fallback_flatpaks")).toArray())
        catalog.fallbackFlatpaks << v.toString();
    const Context top;
    for (const QJsonValue &v : root.value(QStringLiteral("images")).toArray()) {
        if (v.isObject())
            collectLeaves(v.toObject(), {}, top, QString(), catalog.leaves);
    }
    return catalog;
}

Catalog load()
{
    const QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    const QString override = env.value(QString::fromLatin1(IMAGES_ENV));
    if (!override.isEmpty()) {
        QFile f(override);
        if (f.open(QIODevice::ReadOnly))
            return parse(f.readAll());
        return Catalog{};
    }
    // Flatpak detection without offline.h: the sentinel file. imagecatalog
    // stays dependency-free so the tests link only QtCore-adjacent units.
    const bool flatpak = QFile::exists(QStringLiteral("/.flatpak-info"));
    for (const QString &path : searchPaths(flatpak, env)) {
        QFile f(path);
        if (f.open(QIODevice::ReadOnly))
            return parse(f.readAll());
    }
    return Catalog{};
}

} // namespace imagecatalog
