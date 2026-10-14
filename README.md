# VibeDesk

VibeDesk ist ein Fork der freien Open-Source-Versionen von
[RustDesk](https://github.com/rustdesk/rustdesk) (Client) und
[rustdesk-server](https://github.com/rustdesk/rustdesk-server) (Server),
zusammengeführt in einem Monorepo.

## Struktur

| Pfad                      | Inhalt                                             | Upstream |
|---------------------------|----------------------------------------------------|----------|
| `client/`                 | Desktop- und Mobile-Client (Rust + Flutter)        | `rustdesk/rustdesk` |
| `client/libs/hbb_common/` | Gemeinsame Bibliothek, Stand gemäss Client-Upstream | `rustdesk/hbb_common` |
| `server/`                 | ID-/Rendezvous-Server `hbbs` und Relay `hbbr`      | `rustdesk/rustdesk-server` |
| `server/libs/hbb_common/` | Gemeinsame Bibliothek, Stand gemäss Server-Upstream | `rustdesk/hbb_common` |
| `scripts/`                | Hilfsskripte (Upstream-Sync)                       | |
| `console/`                | Verwaltungskonsole (Benutzer, Geräte)              | |
| `deploy/`                 | Docker-Image, Compose-Datei, systemd-Units         | |

Upstream verwaltet `hbb_common` als Git-Submodule. Hier ist es ein normaler
Subtree, damit ein einfaches `git clone` ohne `--recursive` reicht. Client und
Server haben je eine eigene Kopie, weil upstream unterschiedliche Stände
fixiert sind.

## Server installieren

Fertige Server-Pakete (statische Linux-Binaries für x86_64, aarch64 und
armv7 mit systemd-Units, inklusive Verwaltungskonsole) und das Docker-Image
`ghcr.io/simondietschi/vibedesk-server` baut der Workflow
`.github/workflows/vibedesk-server.yml`. Die Installation steht in
[`deploy/README.md`](deploy/README.md).

## Verwaltungskonsole

`console/` enthält eine eigene Weboberfläche für den Server, keinen Nachbau
von RustDesk Pro. Sie läuft auf Port 21114, wo die Clients ohnehin ihre
Heartbeats hinschicken, und bietet:

- Geräteübersicht mit Online-Status, Gerätename, Benutzer, System und Notizen
- Benutzerverwaltung mit den Rollen Admin und Benutzer
- Anmeldung mit Passwort und optionaler Zwei-Faktor-Anmeldung (TOTP)

Installation und HTTPS-Einrichtung: [`deploy/README.md`](deploy/README.md#konsole).
Entwickeln: `cd console && cargo run -- create-admin admin && cargo run`.

## Server selbst bauen

```sh
cd server
cargo build --release
./target/release/hbbs   # ID-/Rendezvous-Server: TCP 21115, 21116, UDP 21116, WS 21118
./target/release/hbbr   # Relay-Server: TCP 21117, WS 21119
```

Beim ersten Start erzeugt `hbbs` ein Schlüsselpaar (`id_ed25519`,
`id_ed25519.pub`) im Arbeitsverzeichnis. Den öffentlichen Schlüssel trägt man
im Client zusammen mit der Serveradresse ein.

Der freie Server kennt keine Begrenzung der Anzahl Geräte oder gleichzeitiger
Verbindungen. Der Relay hat aber Bandbreitenlimits, die sich über
Umgebungsvariablen anpassen lassen (Werte in Mbit/s):

| Variable                | Standard | Bedeutung |
|-------------------------|----------|-----------|
| `TOTAL_BANDWIDTH`       | 1024     | Gesamtbandbreite des Relays |
| `SINGLE_BANDWIDTH`      | 128      | Bandbreite pro Verbindung |
| `LIMIT_SPEED`           | 32       | Bandbreite für gedrosselte Verbindungen |
| `DOWNGRADE_THRESHOLD`   | 0.66     | Auslastungsschwelle für die Drosselung |
| `DOWNGRADE_START_CHECK` | 1800     | Sekunden bis zur ersten Drosselungsprüfung |

Weitere Optionen: `./target/release/hbbs --help` und `server/README.md`.

## Client bauen

Der Client braucht vcpkg (libvpx, libyuv, opus, aom) und für die Oberfläche
Flutter. Die Anleitung steht in `client/README.md`, die CI-Workflows in
`client/.github/workflows/` zeigen die genauen Schritte pro Plattform. Unter
Linux geht es auch ohne vcpkg mit den Systembibliotheken
(`--features linux-pkg-config`).

VibeDesk bringt keinen öffentlichen Server mit. Den eigenen `hbbs` und dessen
öffentlichen Schlüssel kann man beim Bauen fest einbauen, dann muss im Client
nichts konfiguriert werden:

```sh
export VIBEDESK_RENDEZVOUS_SERVER=desk.example.ch
export VIBEDESK_RS_PUB_KEY="$(cat /pfad/zu/id_ed25519.pub)"
export VIBEDESK_API_SERVER=https://konsole.example.ch   # optional
cargo build --release --features flutter
```

Ohne diese Variablen trägt man Server und Schlüssel im Client unter
Einstellungen, Netzwerk, ID/Relay-Server ein. Der API-Server, über den die
Anmeldung und die Adressbücher laufen, wird ohne `VIBEDESK_API_SERVER`
automatisch als `http://<server>:21114` abgeleitet. Das ist unverschlüsselt.
Läuft die Konsole hinter HTTPS (siehe `deploy/README.md`), gehört deren
Adresse in `VIBEDESK_API_SERVER`. Ein im Client eingetragener API-Server hat
weiterhin Vorrang.

## Eigene Builds per GitHub Actions

Der Workflow `.github/workflows/vibedesk-build.yml` baut fertige Clients mit
deinem Server fest eingebaut:

| Plattform | Ergebnis |
|-----------|----------|
| Linux x86_64 | `vibedesk-<version>-x86_64.deb` |
| Windows x64 | `vibedesk-<version>-x86_64.exe` (selbstentpackend) und `.msi` |
| Android | `vibedesk-<version>-universal.apk` (arm64, arm, x86_64) |
| macOS | `vibedesk-<version>-x86_64.dmg` (Intel) und `-aarch64.dmg` (Apple Silicon) |
| iOS | `vibedesk-<version>-unsigned.ipa` |

Server und Schlüssel einmalig als Repository-Variablen hinterlegen
(GitHub: Settings, Secrets and variables, Actions, Variables):

- `VIBEDESK_RENDEZVOUS_SERVER`, z.B. `desk.example.ch`
- `VIBEDESK_RS_PUB_KEY`, Inhalt von `id_ed25519.pub` des Servers
- `VIBEDESK_API_SERVER`, optional, z.B. `https://konsole.example.ch`

Starten lässt sich der Build unter Actions, «VibeDesk Build», «Run workflow».
Dort kann man Server und Schlüssel für einen einzelnen Lauf auch direkt
eingeben. Die Pakete liegen danach als Artefakte beim Lauf. Ein Git-Tag `v*`
(z.B. `v1.5.1`) oder das Feld «Release-Tag» beim manuellen Start legt
zusätzlich einen Release-Entwurf mit Prüfsummen (`SHA256SUMS`) an. Öffentlich
wird er erst, wenn man ihn unter Releases veröffentlicht. Erst dann erstellt
GitHub auch den Tag.

Das Windows-Paket enthält wie upstream den Treiber für virtuelle Bildschirme,
den Druckertreiber für «Drucken auf entferntem Drucker» und das
Hilfsprogramm für den Privatsphäre-Modus. Die Treiber werden aus den
Upstream-Releases geladen und gegen feste Prüfsummen geprüft. Sie sind von
RustDesk signiert und tragen deshalb intern deren Namen.

Der öffentliche Schlüssel ist kein Geheimnis, er darf als normale Variable
gespeichert werden.

macOS-Apps sind nur ad-hoc signiert. Beim ersten Start muss man sie deshalb
per Rechtsklick, «Öffnen» freigeben. Für eine Signatur mit Apple-Zertifikat
trägt man in Xcode unter Signing die eigene Team-ID ein. Die iOS-App ist
unsigniert und muss vor der Installation mit einem eigenen Apple-Konto
signiert werden (z.B. mit Xcode oder AltStore).

Die Android-APK wird ohne weitere Einstellungen mit einem Debug-Schlüssel
signiert und lässt sich so installieren. Für Updates über einen eigenen
Schlüssel hinterlegt man diese Secrets: `ANDROID_SIGNING_KEY` (Keystore als
Base64), `ANDROID_ALIAS`, `ANDROID_KEY_STORE_PASSWORD` und
`ANDROID_KEY_PASSWORD`.

## Branding

Die Quelldateien für Logo und Icons liegen in `branding/` (`icon.svg` farbig,
`mark.svg` einfarbig für Tray und Statusleiste). Daraus erzeugt
`python3 branding/generate.py` alle Icon-Grössen für Desktop, Android, iOS,
macOS und Windows (benötigt `rsvg-convert`, ImageMagick und Pillow).

Intern läuft VibeDesk im «Custom Client»-Modus des Upstream-Codes: Der
App-Name ist `VibeDesk`, Texte mit «RustDesk» werden zur Laufzeit ersetzt und
der Update-Check gegen die RustDesk-Server ist abgeschaltet.

## Upstream-Updates übernehmen

```sh
scripts/sync-upstream.sh          # Client und Server
scripts/sync-upstream.sh client   # nur Client
scripts/sync-upstream.sh server   # nur Server
```

Das Skript holt den aktuellen `master` per `git subtree pull --squash` und zieht
`hbb_common` auf den Commit nach, den upstream fixiert hat. Ändert upstream
den Submodule-Zeiger, kann beim Client- oder Server-Pull ein Konflikt auf
`*/libs/hbb_common` entstehen. Diesen löst man mit
`git rm --cached <prefix>/libs/hbb_common`, behält das Verzeichnis und
committet. Danach das Skript erneut starten.

## Datenschutz

VibeDesk sendet von sich aus keine Daten an Dritte. Den täglichen
Update-Check von `hbbs` gegen `api.rustdesk.com`, der dabei einen
Geräte-Fingerabdruck mitschickt, haben wir entfernt. Verbindungen laufen über
den ID- und Relay-Server, den du selbst betreibst oder im Client einträgst.
Welche Daten dort anfallen (Geräte-ID, IP-Adressen, Verbindungszeitpunkte),
bestimmt der Betreiber dieses Servers.

## Lizenz und Marke

Client und Server stehen unter der
[GNU Affero General Public License v3.0](LICENSE), und das gilt auch für diesen
Fork. Wer eine veränderte Version verbreitet oder als Netzwerkdienst anbietet,
muss den Quellcode unter derselben Lizenz zugänglich machen.

«RustDesk» ist eine Marke der Purslane Ltd. Name, Logo und Icons gehören nicht
zur AGPL-Lizenz und wurden deshalb durch VibeDesk-Branding ersetzt. Die
Copyright-Hinweise der ursprünglichen Autoren bleiben gemäss AGPL erhalten.
