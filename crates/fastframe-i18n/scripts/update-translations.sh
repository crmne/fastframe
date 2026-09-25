#!/usr/bin/env bash
# Extracts the translatable messages of an app into its gettext template,
# merges the template into every catalog, and checks each catalog with msgfmt.
#
# Run from the app's repository root. Requires GNU gettext tools with Rust
# support (xgettext 0.24 or later). Normal Cargo builds do not need them.
#
#   update-translations.sh --package ZapFast --domain zapfast \
#       --bugs 'https://github.com/crmne/zapfast/issues/new?template=translation.yml' \
#       [--keyword translated:2] [--fuzzy-matching] [--dir assets/i18n] [--check]
#
# --package         Project name written into the template header.
# --domain          Template name: <dir>/<domain>.pot.
# --bugs            Where translators report problems (msgid bugs address).
# --keyword         An extra xgettext keyword spec; repeatable.
# --fuzzy-matching  Let msgmerge guess translations for new messages. Off by
#                   default: the build drops fuzzy entries anyway, and a
#                   message left empty shows the English source.
# --dir             Where the template, the catalogs and POTFILES live.
# --check           Change nothing; fail if the template is out of date.
set -euo pipefail

usage() {
    sed -n '9,11p' "$0" | sed 's/^# *//' >&2
    exit 2
}

package= domain= bugs= dir=assets/i18n mode=update fuzzy=no
keywords=()
while [[ $# -gt 0 ]]; do
    case $1 in
        --package) package=${2:?}; shift 2 ;;
        --domain) domain=${2:?}; shift 2 ;;
        --bugs) bugs=${2:?}; shift 2 ;;
        --keyword) keywords+=("--keyword=${2:?}"); shift 2 ;;
        --fuzzy-matching) fuzzy=yes; shift ;;
        --dir) dir=${2:?}; shift 2 ;;
        --check) mode=check; shift ;;
        *) usage ;;
    esac
done
[[ -n $package && -n $domain && -n $bugs ]] || usage
[[ -f $dir/POTFILES ]] || { echo "$dir/POTFILES lists the sources to scan; it is missing" >&2; exit 2; }

template=$dir/$domain.pot
extracted=$(mktemp)
trap 'rm -f "$extracted"' EXIT
xgettext --language=Rust --from-code=UTF-8 \
    --keyword= --keyword=gettext:2 --keyword=ngettext:2,3 --keyword=pgettext:2c,3 \
    "${keywords[@]}" \
    --add-comments=Translators: \
    --flag=ngettext:2:rust-format --flag=ngettext:3:rust-format \
    --package-name="$package" --copyright-holder="$package contributors" \
    --msgid-bugs-address="$bugs" \
    --files-from="$dir/POTFILES" --output="$extracted"

if [[ $mode == check ]]; then
    # The extraction timestamp is the only nondeterministic header.
    diff -u <(sed '/^"POT-Creation-Date:/d' "$template") \
        <(sed '/^"POT-Creation-Date:/d' "$extracted")
else
    cp "$extracted" "$template"
    merge=(msgmerge --update --backup=none)
    [[ $fuzzy == yes ]] || merge+=(--no-fuzzy-matching)
    for catalog in "$dir"/*.po; do
        [[ -e $catalog ]] || continue
        "${merge[@]}" "$catalog" "$template"
    done
fi
for catalog in "$dir"/*.po; do
    [[ -e $catalog ]] || continue
    msgfmt --check --check-format --output-file=/dev/null "$catalog"
done
