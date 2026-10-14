# Arbeitsweise mit Branches

VibeDesk folgt dem GitHub Flow: Es gibt einen stabilen Hauptbranch, und jede
Änderung entsteht auf einem kurzlebigen Branch, der per Pull Request
zurückfliesst.

## Branches

| Branch | Zweck |
|--------|-------|
| `main` | Stabiler Stand. Wird nie direkt bearbeitet, nur über Pull Requests. Releases werden von hier getaggt. |
| `feature/<thema>` | Neue Funktion, z.B. `feature/server-pakete` |
| `fix/<thema>` | Fehlerkorrektur |
| `docs/<thema>` | Nur Dokumentation |
| `chore/<thema>` | Wartung: CI, Abhängigkeiten, Upstream-Sync (`chore/upstream-sync-2026-10`) |

Ein Branch behandelt genau ein Thema und lebt nur, bis sein Pull Request
gemergt ist. Danach wird er gelöscht.

## Ablauf

1. Vom aktuellen `main` abzweigen:
   `git switch main && git pull && git switch -c feature/<thema>`
2. Committen und pushen. Commit-Nachrichten im Imperativ, eine Änderung pro
   Commit.
3. Pull Request nach `main` öffnen. Die CI muss grün sein.
4. Mit «Squash and merge» zusammenführen, damit `main` pro Pull Request
   genau einen Commit erhält. Ausnahme: Upstream-Syncs mit «Create a merge
   commit», sonst geht die Subtree-Historie verloren.
5. Branch löschen.

Ist `main` inzwischen weitergezogen, holt man ihn mit
`git merge origin/main` in den eigenen Branch. Kein Rebase auf Branches,
an denen mehrere arbeiten.

## Releases

Releases entstehen nur aus `main`, über Tags:

| Tag | Ergebnis |
|-----|----------|
| `v1.2.3` | Clients für alle Plattformen, Release-Entwurf |
| `server-v1.2.3` | Server-Pakete und Docker-Image, Release-Entwurf |

## GitHub-Einstellungen

Diese Einstellungen kann nur der Repo-Inhaber setzen:

- **Settings, General, Default branch:** `main`
- **Settings, General, Pull Requests:** nur «Allow squash merging» und
  «Allow merge commits» aktiv lassen, «Automatically delete head branches»
  einschalten
- **Settings, Rules, Rulesets:** für `main` eine Regel anlegen mit
  «Restrict deletions», «Block force pushes», «Require a pull request
  before merging» und «Require status checks to pass»
