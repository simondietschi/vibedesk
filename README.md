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

Upstream verwaltet `hbb_common` als Git-Submodule. Hier ist es ein normaler
Subtree, damit ein einfaches `git clone` ohne `--recursive` reicht. Client und
Server haben je eine eigene Kopie, weil upstream unterschiedliche Stände
fixiert sind.

## Server bauen und starten

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
`client/.github/workflows/` zeigen die genauen Schritte pro Plattform.

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

## Lizenz und Marke

Client und Server stehen unter der
[GNU Affero General Public License v3.0](LICENSE), und das gilt auch für diesen
Fork. Wer eine veränderte Version verbreitet oder als Netzwerkdienst anbietet,
muss den Quellcode unter derselben Lizenz zugänglich machen.

«RustDesk» ist eine Marke der Purslane Ltd. Name, Logo und Icons gehören nicht
zur AGPL-Lizenz. Vor einer eigenen Verteilung sollten sie durch
VibeDesk-Branding ersetzt werden.
