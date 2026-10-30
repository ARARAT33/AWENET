//! Bilingual message catalog for the AWENET product UI.
//!
//! English and Armenian (hy) strings with deterministic key ordering and
//! English fallback for missing translations.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const FALLBACK_LOCALE: &str = "en";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    En,
    Hy,
}

impl Locale {
    pub fn code(&self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Hy => "hy",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEntry {
    pub en: String,
    pub hy: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalog {
    entries: BTreeMap<String, MessageEntry>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a key with its English and Armenian texts.
    pub fn add(&mut self, key: &str, en: &str, hy: &str) {
        self.entries.insert(
            key.to_string(),
            MessageEntry {
                en: en.to_string(),
                hy: hy.to_string(),
            },
        );
    }

    pub fn key_count(&self) -> usize {
        self.entries.len()
    }

    /// Returns the translation, falling back to English when the Armenian
    /// text is empty and to the key itself when the key is unknown.
    pub fn get<'a>(&'a self, locale: Locale, key: &'a str) -> &'a str {
        match self.entries.get(key) {
            Some(entry) => match locale {
                Locale::En => entry.en.as_str(),
                Locale::Hy if entry.hy.is_empty() => entry.en.as_str(),
                Locale::Hy => entry.hy.as_str(),
            },
            None => key,
        }
    }

    /// Renders a message, replacing named placeholders such as {score}.
    pub fn format(&self, locale: Locale, key: &str, args: &BTreeMap<&str, String>) -> String {
        let mut out = self.get(locale, key).to_string();
        for (name, value) in args {
            let placeholder = format!("{{{name}}}");
            if out.contains(&placeholder) {
                out = out.replace(&placeholder, value);
            }
        }
        out
    }
}

/// The built-in product catalog used by the desktop UI.
pub fn product_catalog() -> Catalog {
    let mut c = Catalog::new();
    c.add("welcome", "Welcome to AWENET", "Բարի գալուստ AWENET");
    c.add("nav_home", "Home", "Գլխավոր");
    c.add("nav_browser", "Browser", "Զննիչ");
    c.add("nav_messenger", "Messenger", "Նամականի");
    c.add("nav_storage", "Storage", "Պահեստ");
    c.add("nav_store", "Store", "Խանութ");
    c.add("nav_economy", "Economy", "Տնտեսություն");
    c.add("nav_settings", "Settings", "Կարգավորումներ");
    c.add("status_online", "Online", "Առցանց");
    c.add("status_offline", "Offline", "Անցանց");
    c.add("health_title", "Product health", "Ապրանքի առողջություն");
    c.add(
        "health_score",
        "Health score: {score}",
        "Առողջության միավոր՝ {score}",
    );
    c.add("peers_known", "Known peers", "Հայտնի հասակակիցներ");
    c.add("alerts_title", "Alerts", "Տագնապներ");
    c.add("notifications_title", "Notifications", "Ծանուցումներ");
    c.add(
        "backup_title",
        "Backup and restore",
        "Պահուստավորում և վերականգնում",
    );
    c.add("updates_title", "Software update", "Ծրագրային թարմացում");
    c.add("quota_title", "Quota usage", "Քվոտայի օգտագործում");
    c.add("theme_light", "Light theme", "Բաց տեսք");
    c.add("theme_dark", "Dark theme", "Մուգ տեսք");
    c.add("command_palette", "Command palette", "Հրամանների վահանակ");
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_both_locales() {
        let c = product_catalog();
        assert_eq!(c.get(Locale::En, "nav_home"), "Home");
        assert_eq!(c.get(Locale::Hy, "nav_home"), "Գլխավոր");
        assert!(c.key_count() >= 20);
    }

    #[test]
    fn falls_back_to_english_and_key() {
        let mut c = Catalog::new();
        c.add("only_en", "English only", "");
        assert_eq!(c.get(Locale::Hy, "only_en"), "English only");
        assert_eq!(c.get(Locale::En, "missing"), "missing");
    }

    #[test]
    fn formats_placeholders() {
        let c = product_catalog();
        let mut args = BTreeMap::new();
        args.insert("score", "87".to_string());
        assert_eq!(c.format(Locale::En, "health_score", &args), "Health score: 87");
        assert_eq!(
            c.format(Locale::Hy, "health_score", &args),
            "Առողջության միավոր՝ 87"
        );
        assert_eq!(c.format(Locale::En, "missing", &args), "missing");
    }

    #[test]
    fn serializes_to_json() {
        let c = product_catalog();
        let json = serde_json::to_string(&c).unwrap();
        let back: Catalog = serde_json::from_str(&json).unwrap();
        assert_eq!(back.key_count(), c.key_count());
    }
}
