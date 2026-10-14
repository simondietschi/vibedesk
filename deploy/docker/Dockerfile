# VibeDesk-Server: statische Binaries von hbbs, hbbr und Konsole, ohne
# Betriebssystem im Image.
# Die Binaries legt der Workflow vibedesk-server.yml pro Architektur unter
# bin/<TARGETARCH>/ ab (amd64, arm64, arm).
FROM scratch

ARG TARGETARCH
COPY bin/${TARGETARCH}/hbbs bin/${TARGETARCH}/hbbr bin/${TARGETARCH}/rustdesk-utils \
     bin/${TARGETARCH}/vibedesk-console /usr/bin/

ENV PATH=/usr/bin
# hbbs legt Schlüsselpaar und Datenbank im Arbeitsverzeichnis an.
WORKDIR /data
VOLUME /data

# hbbs: 21115 (NAT-Test), 21116 TCP/UDP (ID/Rendezvous), 21118 (WebSocket)
# hbbr: 21117 (Relay), 21119 (WebSocket)
# vibedesk-console: 21114 (Heartbeats der Clients, Weboberfläche)
EXPOSE 21114 21115 21116 21116/udp 21117 21118 21119

CMD ["hbbs"]
