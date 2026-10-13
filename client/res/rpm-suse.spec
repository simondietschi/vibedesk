Name:       vibedesk
Version:    1.1.9
Release:    0
Summary:    RPM package
License:    GPL-3.0
Requires:   gtk3 libxcb1 libXfixes3 alsa-utils libXtst6 libva2 gstreamer-plugins-base gstreamer-plugin-pipewire
Recommends: libayatana-appindicator3-1 xdotool

# https://docs.fedoraproject.org/en-US/packaging-guidelines/Scriptlets/

%description
The best open-source remote desktop client software, written in Rust.

%prep
# we have no source, so nothing here

%build
# we have no source, so nothing here

%global __python %{__python3}

%install
mkdir -p %{buildroot}/usr/bin/
mkdir -p %{buildroot}/usr/share/vibedesk/
mkdir -p %{buildroot}/usr/share/vibedesk/files/
mkdir -p %{buildroot}/usr/share/icons/hicolor/256x256/apps/
mkdir -p %{buildroot}/usr/share/icons/hicolor/scalable/apps/
install -m 755 $HBB/target/release/vibedesk %{buildroot}/usr/bin/vibedesk
install $HBB/libsciter-gtk.so %{buildroot}/usr/share/vibedesk/libsciter-gtk.so
install $HBB/res/vibedesk.service %{buildroot}/usr/share/vibedesk/files/
install $HBB/res/128x128@2x.png %{buildroot}/usr/share/icons/hicolor/256x256/apps/vibedesk.png
install $HBB/res/scalable.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/vibedesk.svg
install $HBB/res/vibedesk.desktop %{buildroot}/usr/share/vibedesk/files/
install $HBB/res/vibedesk-link.desktop %{buildroot}/usr/share/vibedesk/files/

%files
/usr/bin/vibedesk
/usr/share/vibedesk/libsciter-gtk.so
/usr/share/vibedesk/files/vibedesk.service
/usr/share/icons/hicolor/256x256/apps/vibedesk.png
/usr/share/icons/hicolor/scalable/apps/vibedesk.svg
/usr/share/vibedesk/files/vibedesk.desktop
/usr/share/vibedesk/files/vibedesk-link.desktop

%changelog
# let's skip this for now

%pre
# can do something for centos7
case "$1" in
  1)
    # for install
  ;;
  2)
    # for upgrade
    systemctl stop vibedesk || true
  ;;
esac

%post
cp /usr/share/vibedesk/files/vibedesk.service /etc/systemd/system/vibedesk.service
cp /usr/share/vibedesk/files/vibedesk.desktop /usr/share/applications/
cp /usr/share/vibedesk/files/vibedesk-link.desktop /usr/share/applications/
systemctl daemon-reload
systemctl enable vibedesk
systemctl start vibedesk
update-desktop-database

%preun
case "$1" in
  0)
    # for uninstall
    systemctl stop vibedesk || true
    systemctl disable vibedesk || true
    rm /etc/systemd/system/vibedesk.service || true
  ;;
  1)
    # for upgrade
  ;;
esac

%postun
case "$1" in
  0)
    # for uninstall
    rm /usr/share/applications/vibedesk.desktop || true
    rm /usr/share/applications/vibedesk-link.desktop || true
    update-desktop-database
  ;;
  1)
    # for upgrade
  ;;
esac
