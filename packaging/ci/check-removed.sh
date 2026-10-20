#!/bin/sh
# Fails if any file from a package's file list (one path per line, as printed
# by dpkg -L, rpm -ql or pacman -Qlq) still exists after uninstalling it.
# Directories are skipped: packages share them with others.
set -eu
left=0
while IFS= read -r path; do
    [ -n "$path" ] || continue
    if [ -e "$path" ] && [ ! -d "$path" ]; then
        echo "still present after uninstall: $path" >&2
        left=1
    fi
done <"$1"
[ $left -eq 0 ] && echo "uninstall removed every file the package installed"
exit $left
