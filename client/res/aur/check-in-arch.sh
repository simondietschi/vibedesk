#!/bin/bash
# Baut das PKGBUILD im angegebenen Ordner in einem frischen Arch, prüft es mit
# namcap, installiert das Paket und sucht fehlende Bibliotheken. Läuft als root
# im Container archlinux:base-devel; makepkg selbst läuft als Benutzer mit der
# UID aus HOST_UID, damit die Dateien dem Runner gehören.
set -euo pipefail

dir=$1

pacman -Syu --noconfirm --needed namcap sudo
useradd -m -u "${HOST_UID:-1000}" builder
echo 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder

cd "$dir"
su builder -c "makepkg --printsrcinfo > .SRCINFO && makepkg -f --syncdeps --noconfirm"
cat .SRCINFO

namcap PKGBUILD ./*.pkg.tar.zst | tee namcap.txt || true
if grep -q ' E: ' namcap.txt; then
  echo "namcap meldet Fehler"
  exit 1
fi
rm namcap.txt

pacman -U --noconfirm ./*.pkg.tar.zst || true
pacman -Q "$(sed -n 's/^pkgname = //p' .SRCINFO)"
test -x /usr/bin/vibedesk
test -f /usr/lib/systemd/system/vibedesk.service
app=$(dirname "$(readlink -f /usr/bin/vibedesk)")
missing=$(LD_LIBRARY_PATH="$app/lib" ldd "$app/vibedesk" "$app"/lib/*.so \
  | grep "not found" | sort -u || true)
if [ -n "$missing" ]; then
  echo "Fehlende Bibliotheken:"
  echo "$missing"
  exit 1
fi
