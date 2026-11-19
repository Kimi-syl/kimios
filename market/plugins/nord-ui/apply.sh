#!/bin/sh
# nord-ui — example UI plugin entry (type=ui, runtime=native).
#
# UI plugins are applied by the host: kplugin.rs::apply_ui reads the
# manifest's ui.theme ("nord.ktheme") and loads the bundled 11-slot
# palette through the existing theme system. This native entry is a
# lightweight hook that reports what would be applied; it needs no
# Plugin API access and works fully offline.

DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
THEME="$DIR/nord.ktheme"

echo "nord-ui: applying Nord theme"
if [ -f "$THEME" ]; then
    echo "nord-ui: palette file $THEME"
    echo "nord-ui: slots (RRGGBB):"
    cat "$THEME"
else
    echo "nord-ui: WARNING bundled nord.ktheme not found" >&2
    exit 1
fi
exit 0
