//! Embedded UI catalogs. Package metadata, licenses and raw diagnostics stay upstream.
use std::{
    collections::BTreeMap,
    sync::{LazyLock, OnceLock},
};

type Catalog = BTreeMap<String, String>;
static EN: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../data/i18n/en.json")).expect("English catalog")
});
static ZH_CN: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../data/i18n/zh-CN.json"))
        .expect("Simplified Chinese catalog")
});
static LANGUAGE: OnceLock<Language> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    English,
    SimplifiedChinese,
}

fn select(languages: impl IntoIterator<Item = impl AsRef<str>>) -> Language {
    for locale in languages {
        let normalized = locale
            .as_ref()
            .split(['.', '@'])
            .next()
            .unwrap_or("")
            .replace('_', "-")
            .to_ascii_lowercase();
        if normalized == "c"
            || normalized == "posix"
            || normalized == "en"
            || normalized.starts_with("en-")
        {
            return Language::English;
        }
        if normalized == "zh"
            || normalized == "zh-cn"
            || normalized == "zh-sg"
            || normalized == "zh-hans"
            || normalized.starts_with("zh-hans-")
        {
            return Language::SimplifiedChinese;
        }
        // Do not let GLib's subsequent generic "zh" fallback turn traditional
        // Chinese into simplified Chinese when that translation is unavailable.
        if normalized.starts_with("zh-") {
            return Language::English;
        }
    }
    Language::English
}

pub fn init() {
    LANGUAGE.get_or_init(|| select(gtk::glib::language_names()));
}
fn translate(language: Language, message: &str) -> &str {
    let catalog = match language {
        Language::English => &*EN,
        Language::SimplifiedChinese => &*ZH_CN,
    };
    catalog
        .get(message)
        .or_else(|| EN.get(message))
        .map(String::as_str)
        .unwrap_or(message)
}
pub fn message(message: &str) -> &str {
    init();
    translate(*LANGUAGE.get().unwrap(), message)
}
pub fn tr(key: &'static str) -> &'static str {
    message(key)
}
pub fn phase(key: &str) -> &str {
    match key {
        "request" => tr("Connecting"),
        "download" => tr("Downloading"),
        "cached" => tr("Using cached download"),
        "extract" => tr("Extracting"),
        "configure" => tr("Configuring"),
        "record" => tr("Recording installation"),
        "complete" => tr("Complete"),
        "uninstall" => tr("Uninstalling"),
        _ => key,
    }
}

pub fn plan_note(note: &str) -> String {
    if let Some(component) = note.strip_prefix("Optional recommendation: ") {
        tr_args(
            "Optional recommendation: {component}",
            &[("component", component)],
        )
    } else {
        message(note).to_owned()
    }
}

fn interpolate(pattern: &str, args: &[(&str, &str)]) -> String {
    let mut rest = pattern;
    let mut output = String::new();
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}').map(|n| start + n) else {
            break;
        };
        output.push_str(&rest[..start]);
        let key = &rest[start + 1..end];
        output.push_str(
            args.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| *value)
                .unwrap_or(&rest[start..=end]),
        );
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
}
pub fn tr_args(message: &'static str, args: &[(&str, &str)]) -> String {
    interpolate(tr(message), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn locale_preferences_and_fallbacks() {
        assert_eq!(
            select(["zh_CN.UTF-8", "zh", "C"]),
            Language::SimplifiedChinese
        );
        assert_eq!(select(["zh-Hans-CN"]), Language::SimplifiedChinese);
        assert_eq!(select(["fr_FR", "en_US", "zh_CN"]), Language::English);
        assert_eq!(select(["fr_FR", "zh_CN"]), Language::SimplifiedChinese);
        assert_eq!(select(["zh_TW", "zh", "C"]), Language::English);
        assert_eq!(select(["C.UTF-8"]), Language::English);
        assert_eq!(select(["de_DE"]), Language::English);
    }
    #[test]
    fn catalogs_have_matching_keys_and_placeholders() {
        assert_eq!(
            EN.keys().collect::<Vec<_>>(),
            ZH_CN.keys().collect::<Vec<_>>()
        );
        fn placeholders(s: &str) -> BTreeSet<&str> {
            s.split('{')
                .skip(1)
                .filter_map(|part| part.split_once('}').map(|(name, _)| name))
                .collect()
        }
        for (key, en) in &*EN {
            assert!(!ZH_CN[key].is_empty(), "{key}");
            assert_eq!(placeholders(en), placeholders(&ZH_CN[key]), "{key}");
        }
        assert_eq!(
            translate(Language::SimplifiedChinese, "Install Qt"),
            "安装 Qt"
        );
        assert_eq!(
            translate(Language::SimplifiedChinese, "unknown message"),
            "unknown message"
        );
    }
    #[test]
    fn substitutions_are_reorderable_and_not_recursive() {
        assert_eq!(
            interpolate("{size} / {count}", &[("count", "3"), ("size", "{count}")]),
            "{count} / 3"
        );
    }
}
