# TuneMesh for Fedora. Builds offline from two sources: the
# source tarball and a cargo-vendor tarball of the crates.io dependencies
# (packaging/rpm/build-rpm.sh makes both, and so does the release workflow).
# Library dependencies come from rpm's automatic soname requirements.

# The release profile carries no debug info, so there is nothing to split.
%global debug_package %{nil}

Name:           tunemesh
Version:        0.1.0
Release:        1%{?dist}
Summary:        Music player for a Syncthing-shared library
# TuneMesh is MIT; the rest covers the statically linked Rust crates.
License:        MIT AND Apache-2.0 AND BSD-3-Clause AND CC0-1.0 AND CDLA-Permissive-2.0 AND ISC AND MPL-2.0 AND Unicode-3.0
URL:            https://github.com/brainage04/TuneMesh
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo >= 1.85
BuildRequires:  rust >= 1.85
BuildRequires:  gcc
BuildRequires:  pkgconfig(gtk4) >= 4.14
BuildRequires:  pkgconfig(libadwaita-1) >= 1.5
BuildRequires:  pkgconfig(mpv)
BuildRequires:  /usr/bin/glib-compile-resources
Requires:       /usr/bin/ffmpeg
Requires:       /usr/bin/ffprobe
Requires:       hicolor-icon-theme

%description
TuneMesh plays a music library that lives in one Syncthing folder shared
between your devices, and downloads new songs into it with yt-dlp
(tunemesh-worker, also available as the user service tunemesh-worker).

%prep
%autosetup -n %{name}-%{version} -a 1
mkdir -p .cargo
cat > .cargo/config.toml <<EOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "$PWD/vendor"
EOF

%build
export CARGO_HOME=$PWD/.cargo
cargo build --release --frozen --manifest-path linux/Cargo.toml -p tunemesh -p tunemesh-worker

%install
packaging/linux/install.sh --destdir %{buildroot} --prefix %{_prefix}
rm -rf %{buildroot}%{_datadir}/licenses

%files
%license LICENSE packaging/licenses/THIRD-PARTY-RUST.txt
%{_bindir}/tunemesh
%{_bindir}/tunemesh-worker
%{_datadir}/applications/io.github.brainage04.TuneMesh.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.github.brainage04.TuneMesh.svg
%{_datadir}/metainfo/io.github.brainage04.TuneMesh.metainfo.xml
%{_prefix}/lib/systemd/user/tunemesh-worker.service

%changelog
* Mon Oct 19 2026 brainage04 <brainage04@users.noreply.github.com> - 0.1.0-1
- First package
