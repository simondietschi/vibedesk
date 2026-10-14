# VibeDesk-Server installieren

Der Server besteht aus zwei Diensten:

- **hbbs**, der ID-/Rendezvous-Server: Geräte melden sich hier an und finden sich gegenseitig.
- **hbbr**, der Relay-Server: Er leitet Verbindungen weiter, wenn keine direkte Verbindung möglich ist.

Beide laufen normalerweise auf demselben Server. Ein kleiner VPS mit 1 CPU
und 1 GB RAM reicht für einige hundert Geräte. Bei vielen Relay-Verbindungen
ist die Bandbreite der Engpass.

## Ports

In der Firewall freigeben:

| Port  | Protokoll | Dienst | Zweck |
|-------|-----------|--------|-------|
| 21115 | TCP       | hbbs   | NAT-Test |
| 21116 | TCP + UDP | hbbs   | ID/Rendezvous |
| 21117 | TCP       | hbbr   | Relay |
| 21118 | TCP       | hbbs   | WebSocket (Web-Client) |
| 21119 | TCP       | hbbr   | WebSocket (Web-Client) |

## Variante A: Docker

```sh
mkdir vibedesk && cd vibedesk
curl -O https://raw.githubusercontent.com/simondietschi/vibedesk/main/deploy/docker-compose.yml
docker compose up -d
cat data/id_ed25519.pub
```

Das Image `ghcr.io/simondietschi/vibedesk-server` gibt es für amd64, arm64
und armv7. Tags:

| Tag | Inhalt |
|-----|--------|
| `latest` | neuster Release |
| `1.2.3` | bestimmter Release (Tag `server-v1.2.3`) |
| `edge` | aktueller Stand von `main`, ungetestet |

Das Image ist nach dem ersten Bau privat. Damit der Server es ohne Anmeldung
laden kann, unter GitHub, Packages, `vibedesk-server`, Package settings die
Sichtbarkeit auf «Public» stellen. Alternativ auf dem Server
`docker login ghcr.io` mit einem Token mit Recht `read:packages` ausführen.

## Variante B: systemd

Aus dem Release das Archiv für die eigene Architektur laden
(`x86_64`, `aarch64` oder `armv7`) und als root ausführen:

```sh
tar xzf vibedesk-server-*-linux-x86_64.tar.gz
cd vibedesk-server-*-linux-x86_64
install -m 755 hbbs hbbr rustdesk-utils /usr/local/bin/
useradd --system --home-dir /var/lib/vibedesk --create-home --shell /usr/sbin/nologin vibedesk
install -m 644 systemd/*.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now vibedesk-hbbr vibedesk-hbbs
cat /var/lib/vibedesk/id_ed25519.pub
```

Die Binaries sind statisch gelinkt und laufen auf jeder Linux-Distribution.

## Nach dem ersten Start

`hbbs` erzeugt beim ersten Start ein Schlüsselpaar (`id_ed25519` und
`id_ed25519.pub`) im Datenverzeichnis. Den Inhalt von `id_ed25519.pub`
braucht der Client:

- **Eigener Client-Build:** als Repository-Variable `VIBEDESK_RS_PUB_KEY`
  hinterlegen, zusammen mit `VIBEDESK_RENDEZVOUS_SERVER` (siehe Haupt-README).
- **Manuell:** im Client unter Einstellungen, Netzwerk, ID/Relay-Server
  eintragen.

Den privaten Schlüssel `id_ed25519` sichern. Geht er verloren, müssen alle
Clients den neuen öffentlichen Schlüssel erhalten.

Läuft `hbbr` auf demselben Server, muss man den Relay nirgends angeben. Die
Clients verwenden dann automatisch die Adresse von `hbbs`. Steht der Relay auf
einem anderen Server, startet man `hbbs -r relay.example.ch`.

## Nur eigene Clients zulassen

Mit der Option `-k _` lassen `hbbs` und `hbbr` nur Clients zu, die den
öffentlichen Schlüssel dieses Servers kennen. In `docker-compose.yml` sind die
passenden Zeilen vorbereitet, für systemd die `ExecStart`-Zeilen um `-k _`
ergänzen (`systemctl edit --full vibedesk-hbbs`).
