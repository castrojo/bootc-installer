pkgdatadir = join_paths(get_option('prefix'), get_option('datadir'), meson.project_name())
coredir = join_paths(pkgdatadir, 'bootc_installer/core')

subdir('locales')

sources = [
  '__init__.py',
  'locale.py',
  'languages.py',
  'keymaps.py',
  'timezones.py',
  'disks.py',
  'system.py',
  'fisherman_probe.py',
  'fisherman_validate.py',
]

# The encryption-choices copy fisherman_validate.load_choices() reads beside
# its module: installed alongside, the same arrangement the XFCE frontend
# ships (frontends/xfce meson data), so the Flatpak and the source tree agree.
install_data(sources, install_dir: coredir)
install_data('encryption-choices.json', install_dir: coredir)
