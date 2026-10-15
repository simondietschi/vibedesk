# AUR-Paket `vibedesk-bin`

`PKGBUILD` und `vibedesk-bin.install` sind die Vorlage für ein AUR-Paket, das
das fertige `.deb` aus dem GitHub-Release umpackt. Gebaut wird beim Benutzer
nichts.

Der Linux-Job in `.github/workflows/vibedesk-build.yml` setzt bei jedem Build
Version, Release-Tag und Prüfsumme ein, erzeugt `.SRCINFO`, baut das Paket mit
`makepkg`, prüft es mit `namcap`, installiert es in einem frischen Arch und
sucht fehlende Bibliotheken. Das Ergebnis liegt als
`vibedesk-bin-<version>-aur.tar.gz` im Release.

## Im AUR veröffentlichen

Erst wenn der Release veröffentlicht ist (kein Entwurf), damit die Quelle ohne
Anmeldung heruntergeladen werden kann:

```sh
git clone ssh://aur@aur.archlinux.org/vibedesk-bin.git
cd vibedesk-bin
tar -xzf vibedesk-bin-<version>-aur.tar.gz
git add PKGBUILD .SRCINFO vibedesk-bin.install
git commit -m "Update to <version>"
git push
```

Vorher in der ersten Zeile des `PKGBUILD` die eigene E-Mail-Adresse eintragen.
