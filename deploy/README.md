# VibeDesk-Server installieren

Der Server besteht aus drei Diensten:

- **hbbs**, der ID-/Rendezvous-Server: Geräte melden sich hier an und finden sich gegenseitig.
- **hbbr**, der Relay-Server: Er leitet Verbindungen weiter, wenn keine direkte Verbindung möglich ist.
- **vibedesk-console**, die Verwaltungskonsole: Weboberfläche für Benutzer
  und Geräte. Sie ist optional, hbbs und hbbr laufen auch ohne sie.

Alle drei laufen normalerweise auf demselben Server. Ein kleiner VPS mit 1 CPU
und 1 GB RAM reicht für einige hundert Geräte. Bei vielen Relay-Verbindungen
ist die Bandbreite der Engpass.

## Ports

In der Firewall freigeben:

| Port  | Protokoll | Dienst | Zweck |
|-------|-----------|--------|-------|
| 21114 | TCP       | Konsole | Heartbeats der Clients, Weboberfläche |
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
docker compose exec console vibedesk-console create-admin simon
```

Der letzte Befehl legt das erste Admin-Konto der Konsole an und gibt dessen
Passwort aus.

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
install -m 755 hbbs hbbr rustdesk-utils vibedesk-console /usr/local/bin/
useradd --system --home-dir /var/lib/vibedesk --create-home --shell /usr/sbin/nologin vibedesk
install -m 644 systemd/*.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now vibedesk-hbbr vibedesk-hbbs vibedesk-console
cat /var/lib/vibedesk/id_ed25519.pub
sudo -u vibedesk vibedesk-console --db /var/lib/vibedesk/console.sqlite3 create-admin simon
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

## Konsole

Die Konsole ist unter `http://<server>:21114` erreichbar. Sie zeigt:

- **Geräte:** alle Geräte, die bei hbbs registriert sind, mit Online-Status,
  Gerätename, angemeldetem Benutzer, Betriebssystem, Version und Notizen.
  Die Clients senden dafür alle 15 Sekunden einen Heartbeat an Port 21114.
  Dazu muss am Client nichts eingestellt werden, er leitet die Adresse aus
  dem ID-Server ab.
- **Benutzer:** Konten anlegen, sperren, löschen, Passwort und Zwei-Faktor
  zurücksetzen. Rolle «Admin» darf alles, Rolle «Benutzer» nur das eigene
  Konto verwalten. Sperren, Löschen und Passwort zurücksetzen melden das
  Konto auch in allen Clients ab.
- **Adressbücher:** geteilte Adressbücher anlegen und Benutzern zuweisen,
  mit den Rechten «Lesen», «Lesen und schreiben» oder «Volle Kontrolle».
- **Gerätegruppen:** Geräte in Gruppen ordnen und Benutzern Zugriff auf
  Gruppen geben. Benutzer sehen im Client ihre Gruppen mit deren Geräten.
- **Zugriffsregeln:** Auf Wunsch dürfen nur angemeldete Benutzer verbinden,
  und nur zu erlaubten Geräten (siehe unten).
- **Protokoll:** wer wann von wo auf welches Gerät verbunden war (mit Art der
  Verbindung, Anmeldeweg und Dauer), Dateiübertragungen, Alarme der Geräte
  (z.B. viele falsche Passwörter) sowie Anmeldungen und Änderungen in der
  Konsole. Jede Ansicht lässt sich durchsuchen und als CSV exportieren.
- **Mein Konto:** Passwort ändern, Zwei-Faktor-Anmeldung mit einer
  Authenticator-App einrichten.

### Anmeldung im Client

Mit einem Konto der Konsole meldet man sich im Client an (Symbol oben
rechts, «Anmelden»). Ist Zwei-Faktor eingerichtet, fragt der Client nach dem
Code der Authenticator-App. Danach hat der Client:

- **Adressbuch:** das persönliche Adressbuch des Benutzers und alle geteilten
  Adressbücher, in denen er Mitglied ist. Einträge, Aliasse, Notizen und Tags
  werden auf dem Server gespeichert und auf allen Geräten des Benutzers
  synchronisiert. Im persönlichen Adressbuch merkt sich der Client
  gespeicherte Verbindungspasswörter (als Hash), in geteilten Adressbüchern
  kann ein gemeinsames Passwort pro Eintrag hinterlegt werden. Dieses liegt
  im Klartext in der Datenbank der Konsole.
- **Geräte:** die Geräte, auf denen der Benutzer angemeldet ist. Admins
  sehen alle Geräte, gruppiert nach Benutzer.

Die Konsole zeigt in der Geräteliste, welches Konto auf welchem Gerät
angemeldet ist. Eine Anmeldung im Client verfällt nach 90 Tagen ohne Nutzung.

**Wichtig:** Ohne weitere Einstellung schickt der Client das Passwort über
`http://<server>:21114`, also unverschlüsselt. Für den Betrieb über das
Internet die Konsole hinter HTTPS stellen (siehe unten) und im Client unter
Einstellungen, Netzwerk als API-Server `https://konsole.example.ch`
eintragen.

### Zugriffsregeln

Unter «Gerätegruppen» lässt sich der Zugriff von «Offen» auf «Nur nach
Regeln» umstellen. Dann vermittelt hbbs eine Verbindung nur, wenn der
steuernde Client mit einem Konto angemeldet ist, und zwar:

- **Admins** zu allen Geräten,
- **Benutzer** zu den Geräten, auf denen sie selbst angemeldet sind, und zu
  den Geräten der Gruppen, auf die sie Zugriff haben.

Alle anderen Verbindungsversuche lehnt hbbs mit einer Meldung im Client ab,
auch über den Relay. Das Passwort des Zielgeräts braucht es weiterhin.

Dafür muss hbbs die Datenbank der Konsole lesen können
(`VIBEDESK_CONSOLE_DB`, in `docker-compose.yml` und der systemd-Unit
vorbereitet). Ist sie nicht lesbar, gelten keine Regeln, und hbbs schreibt
eine Warnung ins Log.

Grenzen: Direkte Verbindungen per IP-Adresse im lokalen Netz («Direkter
IP-Zugriff» im Client) laufen nicht über hbbs und werden darum nicht
geprüft. Bestehende Verbindungen bleiben beim Umschalten bestehen.

### Protokoll

Die Einträge im Protokoll meldet das gesteuerte Gerät selbst, sobald es die
Konsole als API-Server kennt (automatisch, wie bei den Heartbeats). Die
Konsole nimmt nur Meldungen von Geräten an, die bei hbbs registriert sind,
und erkennt doppelt gesendete Meldungen. Eine Notiz, die jemand am Ende einer
Sitzung im steuernden Client eingibt, erscheint in der Spalte «Notiz».

Das Protokoll enthält personenbezogene Daten (Namen, IP-Adressen,
Dateinamen). Die Aufbewahrungsdauer lässt sich mit
`VIBEDESK_CONSOLE_LOG_DAYS` festlegen.

Die Konsole prüft jeden Heartbeat gegen die Gerätedatenbank von hbbs: Nur
ein Gerät, dessen ID und interne UUID dort registriert sind, kann Daten
melden. Dafür braucht die Konsole Lesezugriff auf `db_v2.sqlite3`
(`VIBEDESK_HBBS_DB`). Das Verzeichnis darf nicht schreibgeschützt eingehängt
sein, weil SQLite neben der Datei Sperrdateien braucht. Die Konsole selbst
öffnet die Datei nur lesend.

Nach fünf falschen Anmeldungen sperrt die Konsole Benutzername und
IP-Adresse für 15 Minuten. Passwort vergessen:

```sh
docker compose exec console vibedesk-console reset-password simon
# oder ohne Docker:
sudo -u vibedesk vibedesk-console --db /var/lib/vibedesk/console.sqlite3 reset-password simon
```

### Einstellungen

| Variable | Standard | Bedeutung |
|----------|----------|-----------|
| `VIBEDESK_CONSOLE_LISTEN` | `0.0.0.0:21114` | Adresse und Port |
| `VIBEDESK_CONSOLE_DB` | `console.sqlite3` | Datenbank der Konsole |
| `VIBEDESK_HBBS_DB` | leer | Gerätedatenbank von hbbs, nur lesend |
| `VIBEDESK_CONSOLE_SECURE_COOKIES` | `false` | Anmelde-Cookie nur über HTTPS senden |
| `VIBEDESK_CONSOLE_TRUST_PROXY` | `false` | Client-Adresse aus `X-Forwarded-For` lesen |
| `VIBEDESK_CONSOLE_LOG_DAYS` | `365` | Protokolleinträge nach so vielen Tagen löschen |

### HTTPS

Port 21114 spricht nur HTTP. Für die Heartbeats der Clients reicht das, für
die Anmeldung an der Konsole über das Internet nicht. Mit
[Caddy](https://caddyserver.com) als Proxy bekommt die Weboberfläche ein
Let's-Encrypt-Zertifikat, und auf Port 21114 bleiben nur die Client-Endpunkte
offen.

Konsole intern auf Port 21180 umstellen (in `docker-compose.yml` bzw. der
systemd-Unit sind die Zeilen vorbereitet):

```
VIBEDESK_CONSOLE_LISTEN=127.0.0.1:21180
VIBEDESK_CONSOLE_SECURE_COOKIES=true
VIBEDESK_CONSOLE_TRUST_PROXY=true
```

`/etc/caddy/Caddyfile`:

```
konsole.example.ch {
	reverse_proxy 127.0.0.1:21180
}

http://:21114 {
	handle /api/* {
		reverse_proxy 127.0.0.1:21180
	}
	handle {
		respond 404
	}
}
```

Für `konsole.example.ch` braucht es einen DNS-Eintrag auf den Server und
offene Ports 80 und 443.
