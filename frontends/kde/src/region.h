#pragma once

// Region & language data for the welcome step's preflight section.
//
// The GNOME frontend collects a language, a keyboard layout and a timezone on
// dedicated steps (bootc_installer/defaults/language.py, keyboard.py,
// timezone.py) and shows them back on its confirm screen; its finals travel
// in the recipe dict, which fisherman silently ignores — its Recipe struct
// has no such fields, and this frontend's recipe stays field-identical to
// that struct (tests/unit/test_shared_recipe_schema.py pins it). So these
// values are wizard state, not recipe state: they are shown on the confirm
// screen, exactly where GNOME shows them, and not written anywhere else.
//
// Lists come from Qt and the platform, not from a bundled copy: languages
// from QLocale's own database, timezones from QTimeZone, keyboard layouts
// from the X server's evdev list (the host's when sandboxed) with a small
// built-in fallback. Keyboard *variants* are deliberately not offered: the
// GNOME list carries them per layout, but a first-install default layout is
// what the picker is for, and a variant picker doubles the UI for a value
// the installed desktop's own settings own afterwards.

#include <QList>
#include <QString>

namespace region
{

struct Language {
    QString id;      // "en", "de", ...
    QString display; // native name, "English", "Deutsch", ...
};
QList<Language> availableLanguages();
QString systemLanguage(); // id of the live session's language, "" when unknown

struct Keymap {
    QString id;          // "us", "de", ...
    QString description; // "English (US)", ...
};
QList<Keymap> availableKeyboardLayouts();
QString systemKeyboardLayout(); // "us": the GNOME fallback, detection owned by the desktop

QStringList availableTimezones(); // IANA ids, "America/New_York"
QString systemTimezone();         // the live session's id, "UTC" when unknown

// Pretty form of an IANA id for the pickers ("America/New_York" watches the
// underscore; GNOME shows city names from its own table, Qt has none).
QString timezoneDisplay(const QString &ianaId);

// Plain-POSIX TCP reachability: no QtNetwork link for one three-second
// probe. Mirrors conn_check.py's fallback (DNS on 8.8.8.8:53) rather than its
// portal check: the sandbox has the network share, and a portal answer says
// what the sandbox sees, not what fisherman on the host will.
bool hostReachable(const char *host, int port, int timeoutMs);
inline bool networkReachable()
{
    return hostReachable("8.8.8.8", 53, 3000);
}

} // namespace region
