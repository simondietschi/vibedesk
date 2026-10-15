# AUR-Pakete

Zwei Vorlagen für das AUR, beide installieren dasselbe Programm und schliessen
sich gegenseitig aus:

| Paket | Ordner | Quelle |
|---|---|---|
| `vibedesk` | `vibedesk/` | baut den Client aus dem Quellcode (`vibedesk-<version>-src.tar.gz` aus dem Release) |
| `vibedesk-bin` | `vibedesk-bin/` | packt das fertige `.deb` aus dem Release um |

Das Quellpaket braucht beim Bauen Internet: `prepare()` installiert mit rustup
die Rust-Version, mit der der Client getestet ist, lädt die Rust-Crates und
Dart-Pakete und baut `flutter_rust_bridge_codegen`. Flutter selbst ist als
Quelle mit Prüfsumme eingetragen. Hardware-Video-Codecs (`hwcodec`) sind im
Quellpaket nicht enthalten, weil sie ein eigenes FFmpeg aus vcpkg brauchen;
VP8/VP9/AV1 kommen aus den Arch-Bibliotheken `libvpx` und `aom`.

Einen eigenen Server baut man beim Quellpaket so ein:

```sh
VIBEDESK_RENDEZVOUS_SERVER=desk.example.ch VIBEDESK_RS_PUB_KEY=... makepkg -si
```

## Prüfung im Build

`.github/workflows/vibedesk-build.yml` setzt bei jedem Build Version,
Release-Tag und Prüfsummen ein und ruft `check-in-arch.sh` auf. Das Skript
erzeugt `.SRCINFO`, baut das Paket mit `makepkg` in einem frischen Arch, prüft
es mit `namcap`, installiert es und sucht fehlende Bibliotheken. Der Release
enthält danach `vibedesk-<version>-aur.tar.gz` und
`vibedesk-bin-<version>-aur.tar.gz`, jeweils mit `PKGBUILD`, `.SRCINFO` und
Install-Skript.

## Im AUR veröffentlichen

Erst wenn der Release veröffentlicht ist (kein Entwurf), damit die Quellen ohne
Anmeldung heruntergeladen werden können. Vorher in der ersten Zeile des
`PKGBUILD` die eigene E-Mail-Adresse eintragen und `.SRCINFO` mit
`makepkg --printsrcinfo > .SRCINFO` neu erzeugen.

```sh
git clone ssh://aur@aur.archlinux.org/vibedesk.git
cd vibedesk
tar -xzf vibedesk-<version>-aur.tar.gz
git add PKGBUILD .SRCINFO vibedesk.install
git commit -m "Update to <version>"
git push
```

Für `vibedesk-bin` gleich, mit `vibedesk-bin.git` und `vibedesk-bin-<version>-aur.tar.gz`.
