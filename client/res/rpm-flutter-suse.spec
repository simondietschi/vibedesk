Name:       vibedesk
Version:    1.5.1
Release:    0
Summary:    RPM package
License:    GPL-3.0
URL:        https://vibedesk.com
Vendor:     vibedesk <info@vibedesk.com>
Requires:   gtk3 libxcb1 libXfixes3 alsa-utils libXtst6 libva2 gstreamer-plugins-base gstreamer-plugin-pipewire
Recommends: libayatana-appindicator3-1 xdotool
Provides:   libdesktop_drop_plugin.so()(64bit), libdesktop_multi_window_plugin.so()(64bit), libfile_selector_linux_plugin.so()(64bit), libflutter_custom_cursor_plugin.so()(64bit), libflutter_linux_gtk.so()(64bit), libscreen_retriever_plugin.so()(64bit), libtray_manager_plugin.so()(64bit), liburl_launcher_linux_plugin.so()(64bit), libwindow_manager_plugin.so()(64bit), libwindow_size_plugin.so()(64bit), libtexture_rgba_renderer_plugin.so()(64bit)

# https://docs.fedoraproject.org/en-US/packaging-guidelines/Scriptlets/

%description
The best open-source remote desktop client software, written in Rust.

%prep
# we have no source, so nothing here

%build
# we have no source, so nothing here

# %global __python %{__python3}

%install

mkdir -p "%{buildroot}/usr/share/vibedesk" && cp -r ${HBB}/flutter/build/linux/x64/release/bundle/* -t "%{buildroot}/usr/share/vibedesk"
mkdir -p "%{buildroot}/usr/bin"
install -Dm 644 $HBB/res/vibedesk.service -t "%{buildroot}/usr/share/vibedesk/files"
install -Dm 644 $HBB/res/vibedesk.desktop -t "%{buildroot}/usr/share/vibedesk/files"
install -Dm 644 $HBB/res/vibedesk-link.desktop -t "%{buildroot}/usr/share/vibedesk/files"
install -Dm 644 $HBB/res/128x128@2x.png "%{buildroot}/usr/share/icons/hicolor/256x256/apps/vibedesk.png"
install -Dm 644 $HBB/res/scalable.svg "%{buildroot}/usr/share/icons/hicolor/scalable/apps/vibedesk.svg"

%files
/usr/share/vibedesk/*
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
ln -sf /usr/share/vibedesk/vibedesk /usr/bin/vibedesk
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
    rm /usr/bin/vibedesk || true
    rmdir /usr/lib/vibedesk || true
    rmdir /usr/local/vibedesk || true
    rmdir /usr/share/vibedesk || true
    rm /usr/share/applications/vibedesk.desktop || true
    rm /usr/share/applications/vibedesk-link.desktop || true
    update-desktop-database
  ;;
  1)
    # for upgrade
    rmdir /usr/lib/vibedesk || true
    rmdir /usr/local/vibedesk || true
  ;;
esac
