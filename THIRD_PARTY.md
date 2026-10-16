# Third-party material

Qit uses gtk-rs, libadwaita-rs, GTK, libadwaita, libarchive and the Rust crates
recorded in Cargo.lock. Their licenses remain their own. Bundled native libraries
must be accompanied by the license files copied by the packaging script.

`data/mirrors.json` is derived from MirrorZ's public CERNET configuration and
`mirrorz-json-legacy` metadata; its source URLs and retrieval timestamp are embedded in the file. `tests/fixtures/*.xml` are
unmodified Qt repository metadata, used as parser test inputs. Qt and Qt Creator
are trademarks of their respective owners. Qit is independent of The Qt Company.

The independently implemented relocation logic was informed by the MIT-licensed
aqtinstall project (https://github.com/miurahr/aqtinstall), especially updater.py.
No Qt Installer Framework JavaScript is executed or bundled in Qit.

Qt SDKs and other downloaded tools are not part of the Qit application license.
Their component licenses apply separately.
