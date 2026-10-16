# Interface translations

`en.json` and `zh-CN.json` are embedded in the GTK executable at build time.
No locale files need to be installed beside Qit. Keys are the English source
messages; both catalogs must contain the same keys with nonempty translations.
When changing a message, update both catalogs and its call site together.

Use `tr("Message")` for static text and `tr_args` for sentences with named
placeholders such as `{count}` and `{size}`. Translate the whole sentence and
reorder placeholders as needed, but preserve their names. Inserted values are
plain text and are never reinterpreted as placeholders or markup.

The language follows `glib::language_names()` at startup. Simplified Chinese
locales (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`) are supported. Traditional Chinese is
not yet translated and uses English; other unsupported languages try subsequent
system preferences, then English. Missing messages also fall back to English.
Saved legacy `language` settings do not override the system language.

To add another locale, add its JSON catalog and register it in
`crates/qit-gtk/src/i18n.rs`, including locale matching and catalog tests.
Upstream component names/descriptions, licenses and raw technical diagnostics
are intentionally not translated by these catalogs. CLI protocol output is
unchanged.

Validation:

```sh
cargo test -p qit-gtk i18n::tests
GDK_BACKEND=x11 GTK_A11Y=none QIT_TEST_LANGUAGE=zh_CN \
  python3 scripts/run-test-session.py xvfb-run -a \
  -s '-screen 0 1280x1024x24' python3 tests/ui-smoke.py
```

Use `QIT_TEST_LANGUAGE=en` to exercise the English interface. Tests check locale
fallback, matching catalog keys/placeholders, and both GTK interaction paths.
