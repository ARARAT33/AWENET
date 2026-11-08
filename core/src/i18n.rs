//! Bilingual message catalog for the AWENET product UI.
//!
//! English and Armenian (hy) strings with deterministic key ordering and
//! English fallback for missing translations. The catalog is the single
//! source of truth for the desktop economy screens (AWENETwork, ONECOIN
//! wallet, ONECOIN Market, Founder Control Center); the node serves it to
//! the UI through `GET /api/i18n/messages`.

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

    /// Parses a locale code (`en`, `hy`); anything else is rejected.
    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim() {
            "en" => Some(Locale::En),
            "hy" => Some(Locale::Hy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEntry {
    pub en: String,
    pub hy: String,
}

impl MessageEntry {
    fn new(en: &str, hy: &str) -> Self {
        MessageEntry {
            en: en.to_string(),
            hy: hy.to_string(),
        }
    }
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
        let entry = MessageEntry::new(en, hy);
        self.entries.insert(key.to_string(), entry);
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

    /// Returns the flat key-to-text map for `locale`, applying the same
    /// fallback rules as [`Catalog::get`]. Unknown keys are omitted, so a
    /// caller can distinguish "known message" from "raw identifier".
    pub fn messages(&self, locale: Locale) -> BTreeMap<String, String> {
        self.entries
            .iter()
            .map(|(key, entry)| {
                let text = match locale {
                    Locale::En => entry.en.as_str(),
                    Locale::Hy if entry.hy.is_empty() => entry.en.as_str(),
                    Locale::Hy => entry.hy.as_str(),
                };
                (key.clone(), text.to_string())
            })
            .collect()
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
    c.add("lang_toggle", "Language", "Լեզու");

    // --- Economy screen statuses, categories and levels -------------------
    c.add("status_published", "Published", "Հրապարակված");
    c.add("status_closed", "Closed", "Փակված");
    c.add("status_cancelled", "Cancelled", "Չեղարկված");
    c.add("status_submitted", "Submitted", "Ներկայացված");
    c.add("status_approved", "Approved", "Հաստատված");
    c.add("status_rejected", "Rejected", "Մերժված");
    c.add("status_revising", "Revising", "Վերանայման ենթակա");
    c.add("status_held", "Held in escrow", "Պահված էսքրոում");
    c.add("status_released", "Released", "Փոխանցված");
    c.add("status_refunded", "Refunded", "Վերադարձված");
    c.add("status_disputed", "Disputed", "Վիճարկված");
    c.add("status_settled", "Settled", "Կարգավորված");
    c.add("status_expired", "Expired", "Ժամկետանց");
    c.add("cat_engineering", "Engineering", "Ծրագրավորում");
    c.add("cat_design", "Design", "Դիզայն");
    c.add("cat_content", "Content", "Բովանդակություն");
    c.add("cat_network", "Network", "Ցանց");
    c.add("cat_community", "Community", "Համայնք");
    c.add("cx_beginner", "Beginner", "Սկսնակ");
    c.add("cx_intermediate", "Intermediate", "Միջին");
    c.add("cx_advanced", "Advanced", "Բարձր");
    c.add("cx_expert", "Expert", "Փորձագետ");
    c.add("kind_digital", "Digital", "Թվային");
    c.add("kind_service", "Service", "Ծառայություն");
    c.add("kind_physical", "Physical", "Ֆիզիկական");
    c.add("level_beginner", "Beginner", "Սկսնակ");
    c.add("level_contributor", "Contributor", "Մասնակից");
    c.add(
        "level_advanced",
        "Advanced Contributor",
        "Փորձառյալ մասնակից",
    );
    c.add("level_expert", "Expert", "Փորձագետ");
    c.add("level_master", "Master Contributor", "Ավագ մասնակից");

    // --- AWENETwork participant board -------------------------------------
    c.add(
        "awk_hero_title",
        "Official work platform",
        "Պաշտոնական աշխատանքային հարթակ",
    );
    c.add(
        "awk_hero_sub",
        "Founder-published tasks for the AWENET network. Accept a task, deliver real work, and the founder reviews every submission before ONECOIN is minted.",
        "Հիմնադրի հրապարակած առաջադրանքները AWENET ցանցի համար: Ընդունիր առաջադրանքը, կատարիր իրական աշխատանք, և հիմնադիրը ստուգում է ամեն ներկայացումը՝ մինչև ONECOIN-ի թողարկումը:",
    );
    c.add("awk_tab_tasks", "Tasks", "Առաջադրանքներ");
    c.add("awk_tab_mywork", "My Work", "Իմ աշխատանքները");
    c.add("awk_tab_suggest", "Suggest a task", "Առաջարկել առաջադրանք");
    c.add("awk_page_title", "AWENETwork", "AWENETwork");
    c.add("awk_board_title", "Task board", "Առաջադրանքների վահանակ");
    c.add(
        "awk_filter_all_categories",
        "All categories",
        "Բոլոր կատեգորիաները",
    );
    c.add("awk_filter_all_levels", "All levels", "Բոլոր մակարդակները");
    c.add("awk_filter_any_status", "Any status", "Ցանկացած կարգավիճակ");
    c.add(
        "awk_filter_min_reward",
        "Min reward (ONECOIN)",
        "Նվազագույն պարգև (ONECOIN)",
    );
    c.add("awk_apply", "Apply filters", "Կիրառել զտիչները");
    c.add(
        "awk_loading_tasks",
        "Loading tasks…",
        "Առաջադրանքները բեռնվում են…",
    );
    c.add(
        "awk_mywork_title",
        "My work profile",
        "Իմ աշխատանքային պրոֆիլը",
    );
    c.add(
        "awk_loading_profile",
        "Loading profile…",
        "Պրոֆիլը բեռնվում է…",
    );
    c.add(
        "awk_suggest_notice",
        "Suggestions are community proposals. They never become official tasks until the founder reviews and publishes them. No reward is promised before publication.",
        "Առաջարկները համայնքի նախաձեռնություններ են: Դրանք երբեք պաշտոնական առաջադրանք չեն դառնում, քանի դեռ հիմնադիրը դրանք չի ստուգել ու հրապարակել: Հրապարակումից առաջ ոչ մի պարգև չի խոստացվում:",
    );
    c.add("awk_title_ph", "Title (required)", "Վերնագիր (պարտադիր)");
    c.add(
        "awk_suggest_reward_ph",
        "Suggested reward (ONECOIN)",
        "Առաջարկվող պարգև (ONECOIN)",
    );
    c.add(
        "awk_deadline_ph",
        "Deadline (unix seconds, optional)",
        "Վերջնաժամկետ (unix վայրկյան, ըստ ցանկության)",
    );
    c.add(
        "awk_desc_ph",
        "Detailed description (required)",
        "Մանրամասն նկարագրություն (պարտադիր)",
    );
    c.add(
        "awk_outcome_ph",
        "Expected outcome (required)",
        "Սպասվող արդյունք (պարտադիր)",
    );
    c.add(
        "awk_criteria_ph",
        "Acceptance criteria, one per line (at least one required)",
        "Ընդունման չափանիշներ՝ մեկական տող վրա (առնվազն մեկը պարտադիր)",
    );
    c.add(
        "awk_suggest_submit",
        "Submit suggestion",
        "Ուղարկել առաջարկը",
    );
    c.add(
        "awk_suggest_state",
        "Awaiting founder approval — nothing is official until published.",
        "Սպասում է հիմնադրի հաստատմանը՝ ոչինչ պաշտոնական չէ, մինչև հրապարակվի:",
    );
    c.add(
        "awk_empty_board",
        "No tasks match these filters yet. The founder publishes official tasks here.",
        "Այս զտիչներին դեռ ոչ մի առաջադրանք չի համապատասխանում: Հիմնադիրը պաշտոնական առաջադրանքները հրապարակում է այստեղ:",
    );
    c.add(
        "awk_board_unavailable",
        "Task board unavailable: {message}",
        "Առաջադրանքների վահանակն անհասանելի է՝ {message}",
    );
    c.add(
        "awk_contributors",
        "{count} contributor(s)",
        "{count} մասնակից",
    );
    c.add(
        "awk_deadline_short",
        "deadline {date}",
        "վերջնաժամկետ {date}",
    );
    c.add("awk_open_task", "Open task", "Բացել առաջադրանքը");
    c.add("awk_phase", "Phase {n}: {name}", "Փուլ {n}՝ {name}");
    c.add(
        "awk_task_eyebrow",
        "AWENETwork / Task #{id}",
        "AWENETwork / Առաջադրանք #{id}",
    );
    c.add("awk_details", "Details", "Մանրամասներ");
    c.add("awk_category", "Category", "Կատեգորիա");
    c.add("awk_complexity", "Complexity", "Բարդություն");
    c.add("awk_status", "Status", "Կարգավիճակ");
    c.add("awk_reward", "Reward", "Պարգև");
    c.add("awk_deadline_label", "Deadline", "Վերջնաժամկետ");
    c.add("awk_team", "Team work", "Թիմային աշխատանք");
    c.add("awk_team_allowed", "allowed", "թույլատրվում է");
    c.add("awk_team_solo", "solo", "անհատական");
    c.add("awk_contributors_label", "Contributors", "Մասնակիցներ");
    c.add("awk_outcome", "Expected outcome", "Սպասվող արդյունք");
    c.add(
        "awk_criteria",
        "Acceptance criteria",
        "Ընդունման չափանիշներ",
    );
    c.add("awk_phases", "Phases", "Փուլեր");
    c.add("awk_participate", "Participate", "Մասնակցել");
    c.add(
        "awk_participate_notice",
        "Accept the task first, then submit evidence of your work. The founder personally reviews every submission; the reward is paid only after approval.",
        "Նախ ընդունիր առաջադրանքը, ապա ներկայացրու աշխատանքիդ ապացույցները: Հիմնադիրն անձամբ է ստուգում ամեն ներկայացումը, պարգևը վճարվում է միայն հաստատումից հետո:",
    );
    c.add("awk_accept", "Accept this task", "Ընդունել այս առաջադրանքը");
    c.add(
        "awk_accept_ok",
        "Task accepted. You can now work and submit.",
        "Առաջադրանքն ընդունված է: Այժմ կարող ես աշխատել և ներկայացնել:",
    );
    c.add(
        "awk_accept_fail",
        "Accept failed: {message}",
        "Ընդունումը ձախողվեց՝ {message}",
    );
    c.add("awk_submit_work", "Submit work", "Ներկայացնել աշխատանքը");
    c.add(
        "awk_evidence_ph",
        "Evidence: one link, file reference or proof per line (at least 1, max 16, 512 chars each)",
        "Ապացույցներ՝ մեկ հղում, ֆայլի հղում կամ ապացույց մեկ տողում (առնվազն 1, առավելագույնը 16, յուրաքանչյուրը 512 նշան)",
    );
    c.add(
        "awk_note_ph",
        "Note for the reviewer (optional)",
        "Գրառում ստուգողի համար (ըստ ցանկության)",
    );
    c.add("awk_submit", "Submit for review", "Ուղարկել ստուգման");
    c.add(
        "awk_evidence_required",
        "Add at least one line of evidence.",
        "Ավելացրու առնվազն մեկ տող ապացույց:",
    );
    c.add(
        "awk_submit_ok",
        "Submitted (#{id}). Status: awaiting founder review. No reward is paid before approval.",
        "Ներկայացված է (#{id}): Կարգավիճակը՝ սպասում է հիմնադրի ստուգմանը: Հաստատումից առաջ պարգև չի վճարվում:",
    );
    c.add(
        "awk_submit_fail",
        "Submit failed: {message}",
        "Ներկայացումը ձախողվեց՝ {message}",
    );
    c.add("awk_back", "← Back to the task board", "← Վերադարձ վահանակ");
    c.add(
        "awk_task_unavailable",
        "Task unavailable: {message}",
        "Առաջադրանքն անհասանելի է՝ {message}",
    );
    c.add("awk_aweid", "AWE ID", "AWE ID");
    c.add("awk_level", "Level", "Մակարդակ");
    c.add(
        "awk_accepted_tasks",
        "Accepted tasks",
        "Ընդունված առաջադրանքներ",
    );
    c.add("awk_approved", "Approved", "Հաստատված");
    c.add("awk_rejected", "Rejected", "Մերժված");
    c.add("awk_revising", "Revising", "Վերանայման ենթակա");
    c.add("awk_earned", "Earned", "Վաստակված");
    c.add("awk_my_submissions", "My submissions", "Իմ ներկայացումները");
    c.add(
        "awk_empty_submissions",
        "No submissions yet. Accept a task from the board and submit your work.",
        "Դեռ ներկայացումներ չկան: Ընդունիր առաջադրանք վահանակից և ներկայացրու աշխատանքդ:",
    );
    c.add("awk_task_ref", "task #{id}", "առաջադրանք #{id}");
    c.add("awk_paid", "paid {amount}", "վճարված {amount}");
    c.add(
        "awk_founder_review",
        "Founder review:",
        "Հիմնադրի ստուգում՝",
    );
    c.add(
        "awk_resub_ph",
        "New evidence, one per line",
        "Նոր ապացույցներ՝ մեկական տող վրա",
    );
    c.add("awk_resubmit", "Resubmit", "Կրկին ներկայացնել");
    c.add(
        "awk_resubmit_fail",
        "Resubmit failed: {message}",
        "Կրկին ներկայացումը ձախողվեց՝ {message}",
    );
    c.add(
        "awk_profile_unavailable",
        "Profile unavailable: {message}",
        "Պրոֆիլն անհասանելի է՝ {message}",
    );
    c.add(
        "awk_suggest_ok",
        "Suggestion #{id} recorded. It becomes an official task only if the founder approves and publishes it.",
        "Առաջարկ #{id} գրանցված է: Այն պաշտոնական առաջադրանք է դառնում միայն այն դեպքում, եթե հիմնադիրը հաստատի և հրապարակի:",
    );
    c.add(
        "awk_suggest_fail",
        "Suggestion rejected: {message}",
        "Առաջարկը մերժվեց՝ {message}",
    );

    // --- ONECOIN Market (AWENET Store) ------------------------------------
    c.add("mkt_hero_title", "AWENET Store", "AWENET խանութ");
    c.add(
        "mkt_hero_sub",
        "Products and services priced in ONECOIN. Buyer payments are held in escrow and released to the seller only after delivery confirmation; sale revenue is a transfer, never new issuance.",
        "ONECOIN-ով գնակշռված ապրանքներ և ծառայություններ: Գնորդի վճարումները պահվում են էսքրոում և վաճառողին են փոխանցվում միայն առաքման հաստատումից հետո, վաճառքի եկամուտը փոխանցում է, ոչ թե նոր թողարկում:",
    );
    c.add("mkt_page_title", "ONECOIN Market", "ONECOIN շուկա");
    c.add("mkt_tab_browse", "Browse", "Դիտել");
    c.add("mkt_tab_sell", "Sell", "Վաճառել");
    c.add("mkt_tab_orders", "My orders", "Իմ պատվերները");
    c.add("mkt_catalog", "Catalog", "Կատալոգ");
    c.add("mkt_search_ph", "Search products", "Որոնել ապրանքներ");
    c.add("mkt_all_kinds", "All kinds", "Բոլոր տեսակները");
    c.add("mkt_category_ph", "Category", "Կատեգորիա");
    c.add("mkt_search", "Search", "Որոնել");
    c.add(
        "mkt_loading_catalog",
        "Loading catalog…",
        "Կատալոգը բեռնվում է…",
    );
    c.add("mkt_seller_title", "My seller page", "Իմ վաճառողի էջը");
    c.add(
        "mkt_title_ph",
        "Product title (required)",
        "Ապրանքի անվանում (պարտադիր)",
    );
    c.add(
        "mkt_category_opt",
        "Category (optional)",
        "Կատեգորիա (ըստ ցանկության)",
    );
    c.add(
        "mkt_price_ph",
        "Price (ONECOIN, decimal)",
        "Գին (ONECOIN, տասնորդական)",
    );
    c.add(
        "mkt_stock_ph",
        "Stock (empty = unlimited)",
        "Պահեստ (դատարկ = անսահմանափակ)",
    );
    c.add(
        "mkt_desc_ph",
        "Description (optional)",
        "Նկարագրություն (ըստ ցանկության)",
    );
    c.add("mkt_list", "List product", "Ավելացնել ապրանքը");
    c.add(
        "mkt_orders_notice",
        "As a buyer you confirm delivery to release the escrow, or open a dispute for the founder to settle. Digital goods and services are delivered directly between people; the escrow only guards the payment.",
        "Որպես գնորդ՝ դու հաստատում ես առաքումը՝ էսքրոուն ազատելու համար, կամ վեճ ես բացում՝ հիմնադրի կարգավորման համար: Թվային ապրանքներն ու ծառայությունները մարդիկ իրար միջև են փոխանցում, էսքրոուն պահպանում է միայն վճարումը:",
    );
    c.add(
        "mkt_loading_orders",
        "Loading orders…",
        "Պատվերները բեռնվում են…",
    );
    c.add(
        "mkt_empty_products",
        "No products match. The catalog fills as sellers list products.",
        "Ոչ մի ապրանք չի համապատասխանում: Կատալոգը լրանում է, երբ վաճառողները ապրանքներ են ավելացնում:",
    );
    c.add(
        "mkt_catalog_unavailable",
        "Catalog unavailable: {message}",
        "Կատալոգն անհասանելի է՝ {message}",
    );
    c.add("mkt_general", "general", "ընդհանուր");
    c.add("mkt_unlimited", "unlimited stock", "անսահմանափակ պահեստ");
    c.add("mkt_unlimited_short", "unlimited", "անսահմանափակ");
    c.add("mkt_left", "{count} left", "մնացել է {count}");
    c.add("mkt_sold_out", "sold out", "սպառված է");
    c.add("mkt_sold", "{count} sold", "վաճառված {count}");
    c.add("mkt_buy", "Buy with ONECOIN", "Գնել ONECOIN-ով");
    c.add(
        "mkt_order_ok",
        "Order #{id} funded ({amount}). Held in escrow until you confirm delivery.",
        "Պատվեր #{id} ֆինանսավորված է ({amount}): Պահվում է էսքրոում մինչև առաքման հաստատումը:",
    );
    c.add(
        "mkt_buy_fail",
        "Purchase failed: {message}",
        "Գնումը ձախողվեց՝ {message}",
    );
    c.add(
        "mkt_price_required",
        "A price in ONECOIN is required.",
        "ONECOIN-ով գինը պարտադիր է:",
    );
    c.add(
        "mkt_listed",
        "Listed as product #{id}. It is now visible in the catalog.",
        "Ավելացված է որպես ապրանք #{id}: Այժմ երևում է կատալոգում:",
    );
    c.add(
        "mkt_list_fail",
        "Listing failed: {message}",
        "Ավելացումը ձախողվեց՝ {message}",
    );
    c.add("mkt_stat_products", "Products", "Ապրանքներ");
    c.add("mkt_stat_active", "Active: {count}", "Ակտիվ՝ {count}");
    c.add("mkt_stat_sold", "Sold units", "Վաճառված միավորներ");
    c.add(
        "mkt_stat_escrow",
        "Escrow releases on confirmation",
        "Էսքրոուն ազատվում է հաստատումից հետո",
    );
    c.add("mkt_active", "active", "ակտիվ");
    c.add("mkt_inactive", "inactive", "ոչ ակտիվ");
    c.add(
        "mkt_new_price_ph",
        "New price (ONECOIN)",
        "Նոր գին (ONECOIN)",
    );
    c.add(
        "mkt_new_stock_ph",
        "New stock (empty = unlimited)",
        "Նոր պահեստ (դատարկ = անսահմանափակ)",
    );
    c.add("mkt_save", "Save changes", "Պահպանել փոփոխությունները");
    c.add("mkt_deactivate", "Deactivate", "Ոչ ակտիվացնել");
    c.add(
        "mkt_empty_listings",
        "You have no listings yet. Products you list appear in the catalog immediately.",
        "Դեռ ապրանքներ չունես: Ավելացրած ապրանքներդ անմիջապես հայտնվում են կատալոգում:",
    );
    c.add(
        "mkt_update_fail",
        "Update failed: {message}",
        "Թարմացումը ձախողվեց՝ {message}",
    );
    c.add(
        "mkt_deactivate_fail",
        "Deactivation failed: {message}",
        "Ոչ ակտիվացումը ձախողվեց՝ {message}",
    );
    c.add(
        "mkt_seller_unavailable",
        "Seller page unavailable: {message}",
        "Վաճառողի էջն անհասանելի է՝ {message}",
    );
    c.add("mkt_role", "you are the {role}", "դու {role} ես");
    c.add("mkt_role_buyer", "buyer", "գնորդ");
    c.add("mkt_role_seller", "seller", "վաճառող");
    c.add("mkt_fee", "fee {amount}", "միջնորդավճար {amount}");
    c.add("mkt_buyer", "buyer", "գնորդ");
    c.add("mkt_seller", "seller", "վաճառող");
    c.add("mkt_dispute", "Dispute:", "Վեճ՝");
    c.add(
        "mkt_awaiting_ruling",
        "awaiting founder ruling",
        "սպասում է հիմնադրի որոշմանը",
    );
    c.add(
        "mkt_confirm",
        "Confirm delivery (release escrow)",
        "Հաստատել առաքումը (ազատել էսքրոուն)",
    );
    c.add("mkt_open_dispute", "Open dispute", "Բացել վեճ");
    c.add(
        "mkt_confirm_ok",
        "Escrow released. Seller received {amount}.",
        "Էսքրոուն ազատված է: Վաճառողը ստացավ {amount}:",
    );
    c.add(
        "mkt_confirm_fail",
        "Confirm failed: {message}",
        "Հաստատումը ձախողվեց՝ {message}",
    );
    c.add(
        "mkt_dispute_prompt",
        "Describe the problem with this order:",
        "Նկարագրիր այս պատվերի խնդիրը՝",
    );
    c.add(
        "mkt_dispute_ok",
        "Dispute opened. The founder will settle it.",
        "Վեճը բացված է: Հիմնադիրը կկարգավորի այն:",
    );
    c.add(
        "mkt_dispute_fail",
        "Dispute failed: {message}",
        "Վեճի բացումը ձախողվեց՝ {message}",
    );
    c.add(
        "mkt_empty_orders",
        "No orders yet. Purchases appear here with their escrow status.",
        "Դեռ պատվերներ չկան: Գնումները հայտնվում են այստեղ իրենց էսքրոուի կարգավիճակով:",
    );
    c.add(
        "mkt_orders_unavailable",
        "Orders unavailable: {message}",
        "Պատվերներն անհասանելի են՝ {message}",
    );

    // --- Founder Control Center -------------------------------------------
    c.add(
        "fnd_hero_title",
        "Economy command",
        "Տնտեսության կառավարում",
    );
    c.add(
        "fnd_hero_sub",
        "Publish official AWENETwork tasks, review submissions personally, set the official ONECOIN price and settle store disputes. Every action is audit-logged.",
        "Հրապարակիր պաշտոնական AWENETwork առաջադրանքներ, անձամբ ստուգիր ներկայացումները, սահմանիր ONECOIN-ի պաշտոնական գինը և կարգավորիր խանութի վեճերը: Ամեն գործողություն գրանցվում է աուդիտում:",
    );
    c.add("fnd_page_title", "Founder Center", "Հիմնադրի կենտրոն");
    c.add("fnd_tab_overview", "Overview", "Ընդհանուր");
    c.add("fnd_tab_publish", "Publish task", "Հրապարակել առաջադրանք");
    c.add("fnd_tab_review", "Review work", "Ստուգել աշխատանքը");
    c.add("fnd_tab_price", "ONECOIN price", "ONECOIN գին");
    c.add("fnd_tab_suggestions", "Suggestions", "Առաջարկներ");
    c.add("fnd_tab_store", "Store orders", "Խանութի պատվերներ");
    c.add("fnd_tab_passkey", "Security", "Անվտանգություն");
    c.add(
        "fnd_loading_overview",
        "Loading economy overview…",
        "Տնտեսության տվյալները բեռնվում են…",
    );
    c.add(
        "fnd_publish_title",
        "Publish official task",
        "Հրապարակել պաշտոնական առաջադրանք",
    );
    c.add(
        "fnd_publish_notice",
        "Only tasks published here are official. Set the reward and acceptance criteria before publishing; phase rewards must sum to the task reward when phases are used.",
        "Միայն այստեղ հրապարակված առաջադրանքներն են պաշտոնական: Սահմանիր պարգևը և ընդունման չափանիշները հրապարակելուց առաջ, փուլեր օգտագործելիս փուլային պարգևները պետք է գումարվեն առաջադրանքի պարգևին:",
    );
    c.add(
        "fnd_reward_ph",
        "Reward (ONECOIN, decimal)",
        "Պարգև (ONECOIN, տասնորդական)",
    );
    c.add(
        "fnd_team_check",
        "Allow team submissions",
        "Թույլատրել թիմային ներկայացումներ",
    );
    c.add(
        "fnd_skills_ph",
        "Required skills, comma separated (optional)",
        "Պահանջվող հմտություններ՝ բաժանված ստորակետով (ըստ ցանկության)",
    );
    c.add(
        "fnd_phases_ph",
        "Phases, one per line: name | acceptance criteria | reward ONECOIN (optional)",
        "Փուլեր՝ մեկական տողում՝ անվանում | ընդունման չափանիշ | պարգև ONECOIN (ըստ ցանկության)",
    );
    c.add("fnd_publish_btn", "Publish task", "Հրապարակել առաջադրանքը");
    c.add(
        "fnd_review_title",
        "Submitted work",
        "Ներկայացված աշխատանքներ",
    );
    c.add(
        "fnd_review_notice",
        "Approve to mint the reward (idempotent, audited). Reject or request revision with a reason; the contributor can resubmit after a revision request.",
        "Հաստատիր՝ պարգևը թողարկելու համար (իդեմպոտենտ, աուդիտավորված): Մերժիր կամ ուղղման պահանջ ներկայացրու՝ պատճառով, մասնակիցը կարող է կրկին ներկայացնել ուղղման պահանջից հետո:",
    );
    c.add(
        "fnd_loading_review",
        "Loading submissions…",
        "Ներկայացումները բեռնվում են…",
    );
    c.add(
        "fnd_price_title",
        "Official ONECOIN price",
        "ONECOIN-ի պաշտոնական գինը",
    );
    c.add(
        "fnd_price_notice",
        "The official price is an internal policy reference only; it does not guarantee external exchange or redemption. Sellers price their own products in ONECOIN separately.",
        "Պաշտոնական գինը միայն ներքին քաղաքականության հղում է, այն երաշխավորում չի տալիս արտաքին փոխանակում կամ մարում: Վաճառողներն իրենց ապրանքները ONECOIN-ով գնակշռում են առանձին:",
    );
    c.add(
        "fnd_price_ph",
        "New official price (USD cents)",
        "Նոր պաշտոնական գին (USD ցենտ)",
    );
    c.add(
        "fnd_effective_ph",
        "Effective at (unix seconds, optional = now)",
        "Ուժի մեջ է մտնում (unix վայրկյան, դատարկ = հիմա)",
    );
    c.add(
        "fnd_price_reason_ph",
        "Public reason (required)",
        "Հրապարակային պատճառ (պարտադիր)",
    );
    c.add(
        "fnd_set_price",
        "Set official price",
        "Սահմանել պաշտոնական գինը",
    );
    c.add(
        "fnd_price_required",
        "A positive price and a public reason are required.",
        "Դրական գինը և հրապարակային պատճառը պարտադիր են:",
    );
    c.add(
        "fnd_price_ok",
        "Official price set: sequence #{id}. Recorded and audit-logged.",
        "Պաշտոնական գինը սահմանված է՝ հաջորդականություն #{id}: Գրանցված և աուդիտավորված է:",
    );
    c.add(
        "fnd_price_fail",
        "Price change failed: {message}",
        "Գնի փոփոխումը ձախողվեց՝ {message}",
    );
    c.add("fnd_price_history", "Price history", "Գների պատմություն");
    c.add(
        "fnd_sug_title",
        "Community suggestions",
        "Համայնքի առաջարկներ",
    );
    c.add(
        "fnd_sug_notice",
        "Suggestions become official only when you approve and publish them.",
        "Առաջարկները պաշտոնական են դառնում միայն քո հաստատմամբ ու հրապարակմամբ:",
    );
    c.add(
        "fnd_loading_sug",
        "Loading suggestions…",
        "Առաջարկները բեռնվում են…",
    );
    c.add(
        "fnd_store_title",
        "Store orders and disputes",
        "Խանութի պատվերներ և վեճեր",
    );
    c.add(
        "fnd_store_notice",
        "Escrowed orders are released to the seller only after the buyer confirms delivery. Disputes are settled by the founder; refunds return the escrow to the buyer.",
        "Էսքրոուի պատվերները վաճառողին են փոխանցվում միայն գնորդի առաքման հաստատումից հետո: Վեճերը կարգավորում է հիմնադիրը, վերադարձները էսքրուն վերադարձնում են գնորդին:",
    );
    c.add(
        "fnd_loading_store",
        "Loading store orders…",
        "Խանութի պատվերները բեռնվում են…",
    );
    c.add(
        "fnd_passkey_title",
        "Founder security (WebAuthn passkeys)",
        "Հիմնադրի անվտանգություն (WebAuthn բանալիներ)",
    );
    c.add(
        "fnd_passkey_notice",
        "Enroll a browser passkey (Ed25519) from this UI origin (localhost:41800). Once a passkey is enrolled, every founder mutation requires a fresh MFA step-up session in addition to the founder key.",
        "Գրանցիր բրաուզերային բանալին (Ed25519) այս UI-ի ծագումից (localhost:41800): Բանալի գրանցվելուց հետո հիմնադրի ամեն փոփոխություն պահանջում է թարմ MFA ստուգման նիստ՝ հիմնադրի բանլուն զուգահեռ:",
    );
    c.add(
        "fnd_loading_passkey",
        "Loading security state…",
        "Անվտանգության վիճակը բեռնվում է…",
    );
    c.add(
        "fnd_passkey_label_ph",
        "Label (optional, e.g. laptop key)",
        "Անվանում (ըստ ցանկության, օրինակ՝ լափթոփի բանալի)",
    );
    c.add("fnd_enroll", "Enroll passkey", "Գրանցել բանալին");
    c.add("fnd_stepup", "Start MFA step-up", "MFA ստուգում");
    c.add(
        "fnd_no_passkey",
        "No passkey enrolled yet. The founder key alone currently authorizes founder mutations.",
        "Դեռ բանալի գրանցված չէ: Ներկայումս միայն հիմնադրի բանալին է լիազորում հիմնադրի փոփոխությունները:",
    );
    c.add("fnd_enrolled_at", "enrolled {date}", "գրանցված {date}");
    c.add(
        "fnd_mfa_active",
        "MFA step-up session active until {date}.",
        "MFA ստուգման նիստն ակտիվ է մինչև {date}:",
    );
    c.add(
        "fnd_mfa_none",
        "No active MFA session.",
        "Ակտիվ MFA նիստ չկա:",
    );
    c.add(
        "fnd_mfa_required_note",
        "Founder mutations require a step-up first.",
        "Հիմնադրի փոփոխությունները նախ պահանջում են MFA ստուգում:",
    );
    c.add(
        "fnd_passkey_state_fail",
        "Security state unavailable: {message}",
        "Անվտանգության վիճակն անհասանելի է՝ {message}",
    );
    c.add(
        "fnd_webauthn_missing",
        "This browser does not expose WebAuthn; enroll from a WebAuthn-capable browser on the node UI origin.",
        "Այս բրաուզերը WebAuthn չունի, գրանցիր WebAuthn-ով բրաուզերից՝ հանգույցի UI-ի ծագումից:",
    );
    c.add(
        "fnd_passkey_ok",
        "Passkey enrolled: {label}. Founder mutations now require a fresh MFA step-up.",
        "Բանալին գրանցված է՝ {label}: Հիմնադրի փոփոխություններն այժմ պահանջում են թարմ MFA ստուգում:",
    );
    c.add(
        "fnd_enroll_fail",
        "Enrollment failed: {message}",
        "Գրանցումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_webauthn_missing_short",
        "This browser does not expose WebAuthn.",
        "Այս բրաուզերը WebAuthn չունի:",
    );
    c.add(
        "fnd_stepup_ok",
        "MFA step-up complete. Session active until {date}.",
        "MFA ստուգումն ավարտված է: Նիստն ակտիվ է մինչև {date}:",
    );
    c.add(
        "fnd_stepup_fail",
        "Step-up failed: {message}",
        "Ստուգումը ձախողվեց՝ {message}",
    );
    c.add("fnd_overview_price", "Official price", "Պաշտոնական գին");
    c.add(
        "fnd_overview_internal",
        "Internal reference",
        "Ներքին հղում",
    );
    c.add("fnd_overview_issued", "Issued", "Թողարկված");
    c.add(
        "fnd_overview_matches",
        "Matches ledger exactly",
        "Ճիշտ համընկնում է գրքի հետ",
    );
    c.add(
        "fnd_overview_circulating",
        "Circulating",
        "Շրջանառության մեջ",
    );
    c.add(
        "fnd_overview_escrow",
        "Escrow held: {amount}",
        "Էսքրոուում՝ {amount}",
    );
    c.add(
        "fnd_overview_task_rewards",
        "Task rewards",
        "Առաջադրանքների պարգևներ",
    );
    c.add(
        "fnd_overview_resource",
        "Resource: {amount}",
        "Ռեսուրս՝ {amount}",
    );
    c.add("fnd_overview_store_volume", "Store volume", "Խանութի ծավալ");
    c.add(
        "fnd_overview_seller_income",
        "Seller income: {amount}",
        "Վաճառողի եկամուտ՝ {amount}",
    );
    c.add(
        "fnd_overview_inflation",
        "Inflation risk (30d)",
        "Գնաճային ռիսկ (30 օր)",
    );
    c.add(
        "fnd_overview_treasury_entries",
        "Treasury entries: {count}",
        "Գանձապետական գրառումներ՝ {count}",
    );
    c.add("fnd_overview_tasks", "Tasks", "Առաջադրանքներ");
    c.add(
        "fnd_overview_submissions",
        "Submissions: {count}",
        "Ներկայացումներ՝ {count}",
    );
    c.add(
        "fnd_overview_suggestions",
        "Suggestions: {count}",
        "Առաջարկներ՝ {count}",
    );
    c.add("fnd_overview_store", "Store", "Խանութ");
    c.add(
        "fnd_overview_orders",
        "Orders: {count}",
        "Պատվերներ՝ {count}",
    );
    c.add(
        "fnd_overview_fee",
        "Fee: {bps} bps",
        "Միջնորդավճար՝ {bps} բազային կետ",
    );
    c.add(
        "fnd_audit_log",
        "Founder audit log (latest first)",
        "Հիմնադրի աուդիտի մատյան (նորերը սկզբում)",
    );
    c.add(
        "fnd_overview_unavailable",
        "Overview unavailable: {message}",
        "Ընդհանուր տվյալներն անհասանելի են՝ {message}",
    );
    c.add("fnd_contributor", "contributor {id}", "մասնակից {id}");
    c.add("fnd_note", "Note:", "Գրառում՝");
    c.add(
        "fnd_reason_ph",
        "Reason (required for reject/revise)",
        "Պատճառ (պարտադիր մերժման/ուղղման համար)",
    );
    c.add("fnd_approve_pay", "Approve & pay", "Հաստատել և վճարել");
    c.add("fnd_team_payout", "Team payout", "Թիմային վճարում");
    c.add("fnd_reject", "Reject", "Մերժել");
    c.add("fnd_revise", "Request revision", "Պահանջել ուղղում");
    c.add(
        "fnd_split_ph",
        "Team split, one per line: AWE_ID (64 hex) | ONECOIN amount",
        "Թիմի բաշխում՝ մեկական տողում՝ AWE_ID (64 hex) | ONECOIN գումար",
    );
    c.add(
        "fnd_no_submissions",
        "No submissions awaiting review.",
        "Ստուգման սպասող ներկայացումներ չկան:",
    );
    c.add(
        "fnd_approved_paid",
        "Approved. Paid: {amount}",
        "Հաստատված է: Վճարված՝ {amount}",
    );
    c.add(
        "fnd_approve_fail",
        "Approve failed: {message}",
        "Հաստատումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_reason_required",
        "A reason is required.",
        "Պատճառը պարտադիր է:",
    );
    c.add(
        "fnd_reject_fail",
        "Reject failed: {message}",
        "Մերժումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_revise_fail",
        "Revise failed: {message}",
        "Ուղղման պահանջը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_team_ok",
        "Team payout done: {amount}",
        "Թիմային վճարումն ավարտված է՝ {amount}",
    );
    c.add(
        "fnd_team_fail",
        "Team payout failed: {message}",
        "Թիմային վճարումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_review_unavailable",
        "Review queue unavailable: {message}",
        "Ստուգման հերթն անհասանելի է՝ {message}",
    );
    c.add("fnd_by", "by {id}", "{id} կողմից");
    c.add(
        "fnd_approve_publish",
        "Approve & publish as official task",
        "Հաստատել և հրապարակել որպես պաշտոնական առաջադրանք",
    );
    c.add(
        "fnd_no_suggestions",
        "No pending suggestions.",
        "Սպասող առաջարկներ չկան:",
    );
    c.add(
        "fnd_published_task",
        "Published as task #{id}",
        "Հրապարակված է որպես առաջադրանք #{id}",
    );
    c.add(
        "fnd_suggestions_unavailable",
        "Suggestions unavailable: {message}",
        "Առաջարկներն անհասանելի են՝ {message}",
    );
    c.add(
        "fnd_rule_refund",
        "Rule: refund buyer",
        "Որոշում՝ վերադարձ գնորդին",
    );
    c.add(
        "fnd_rule_release",
        "Rule: release to seller",
        "Որոշում՝ փոխանցում վաճառողին",
    );
    c.add("fnd_refund_btn", "Founder refund", "Հիմնադրի վերադարձ");
    c.add(
        "fnd_no_orders",
        "No store orders yet.",
        "Դեռ խանութի պատվերներ չկան:",
    );
    c.add(
        "fnd_settled",
        "Dispute settled: {amount}",
        "Վեճը կարգավորված է՝ {amount}",
    );
    c.add(
        "fnd_resolve_fail",
        "Resolve failed: {message}",
        "Կարգավորումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_refunded",
        "Refunded: {amount}",
        "Վերադարձված է՝ {amount}",
    );
    c.add(
        "fnd_refund_fail",
        "Refund failed: {message}",
        "Վերադարձը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_store_unavailable",
        "Store orders unavailable: {message}",
        "Խանութի պատվերներն անհասանելի են՝ {message}",
    );
    c.add(
        "fnd_publish_ok",
        "Published as official task #{id}. It is now visible on the AWENETwork board.",
        "Հրապարակված է որպես պաշտոնական առաջադրանք #{id}: Այժմ երևում է AWENETwork վահանակում:",
    );
    c.add(
        "fnd_publish_fail",
        "Publish failed: {message}",
        "Հրապարակումը ձախողվեց՝ {message}",
    );
    c.add(
        "fnd_not_founder",
        "This AWE ID is not the AWENET founder. The Founder Control Center is available only to the founder identity installed at economy genesis.",
        "Այս AWE ID-ն AWENET-ի հիմնադիրը չէ: Հիմնադրի կառավարման կենտրոնը հասանելի է միայն տնտեսության գենեզիսում տեղադրված հիմնադրի ինքնությանը:",
    );

    // --- ONECOIN wallet ----------------------------------------------------
    c.add("oc_hero_title", "Network economy", "Ցանցի տնտեսություն");
    c.add(
        "oc_hero_sub",
        "Signed ONECOIN wallet transfers, resource rewards and P2P exchange offers.",
        "Ստորագրված ONECOIN դրամապանակի փոխանցումներ, ռեսուրսային պարգևներ և P2P փոխանակման առաջարկներ:",
    );
    c.add("oc_page_title", "ONECOIN", "ONECOIN");
    c.add("oc_wallet", "Wallet", "Դրամապանակ");
    c.add("oc_connecting", "Connecting…", "Միանում է…");
    c.add("oc_loading_aweid", "Loading AWE ID", "AWE ID-ն բեռնվում է");
    c.add("oc_tier", "Tier", "Մակարդակ");
    c.add("oc_loading", "Loading…", "Բեռնվում է…");
    c.add(
        "oc_contribution_verified",
        "Verified resource contribution",
        "Ստուգված ռեսուրսային ներդրում",
    );
    c.add("oc_fee", "ONEBANK fee", "ONEBANK միջնորդավճար");
    c.add("oc_fee_config", "Configurable 0–5%", "Կարգավորվող 0–5%");
    c.add("oc_exchange", "Exchange", "Փոխանակում");
    c.add(
        "oc_fiat_rail",
        "External fiat settlement rail",
        "Արտաքին ֆիատ կարգավորման ուղի",
    );
    c.add("oc_official_price", "Official price", "Պաշտոնական գին");
    c.add(
        "oc_price_reference",
        "Founder-set internal reference",
        "Հիմնադրի սահմանած ներքին հղում",
    );
    c.add("oc_tab_exchange", "Exchange", "Փոխանակում");
    c.add("oc_tab_rewards", "Rewards", "Պարգևներ");
    c.add("oc_tab_tiers", "Tiers", "Մակարդակներ");
    c.add(
        "oc_recipient_ph",
        "Recipient AWE ID (64 hex)",
        "Ստացողի AWE ID (64 hex)",
    );
    c.add("oc_amount_ph", "ONECOIN amount", "ONECOIN գումար");
    c.add("oc_memo_ph", "Memo", "Գրառում");
    c.add("oc_send", "Send ONECOIN", "Ուղարկել ONECOIN");
    c.add("oc_receive", "Receive (QR)", "Ստանալ (QR)");
    c.add("oc_scan", "Scan to pay", "Սկանավորել վճարելու համար");
    c.add(
        "oc_wallet_notice",
        "Private identity material remains in the existing AWENET vault. QR receiving shares your AWE ID; Scan to pay reads a recipient AWE ID QR with the camera where the browser supports it.",
        "Անձնական ինքնության նյութը մնում է AWENET պահոցում: QR ստացումը կիսվում է քո AWE ID-ով, իսկ Scan to pay-ը տեսախցիկով կարդում է ստացողի AWE ID QR-ը՝ այն բրաուզերներում, որտեղ դա սատարվում է:",
    );
    c.add(
        "oc_send_required",
        "Recipient and positive amount are required.",
        "Ստացողը և դրական գումարը պարտադիր են:",
    );
    c.add(
        "oc_send_ok",
        "Transfer accepted: {id}",
        "Փոխանցումն ընդունված է՝ {id}",
    );
    c.add(
        "oc_send_fail",
        "Transfer rejected/backend unavailable: {message}",
        "Փոխանցումը մերժվեց/հանգույցն անհասանելի է՝ {message}",
    );
    c.add("oc_create_offer", "Create P2P offer", "Ստեղծել P2P առաջարկ");
    c.add("oc_sell_onecoin", "Sell ONECOIN", "Վաճառել ONECOIN");
    c.add("oc_buy_onecoin", "Buy ONECOIN", "Գնել ONECOIN");
    c.add(
        "oc_fiat_price_ph",
        "Fiat price / coin",
        "Ֆիատ գին / մետաղադրամ",
    );
    c.add("oc_publish_offer", "Publish offer", "Հրապարակել առաջարկը");
    c.add(
        "oc_exchange_notice",
        "ONEBANK records the ONECOIN order; actual fiat transfer must use the selected external payment rail.",
        "ONEBANK-ը գրանցում է ONECOIN պատվերը, իսկ իրական ֆիատ փոխանցումը պետք է կատարվի ընտրված արտաքին վճարման ուղիով:",
    );
    c.add("oc_offers", "Offers", "Առաջարկներ");
    c.add("oc_offer_ok", "Published: {id}", "Հրապարակված է՝ {id}");
    c.add(
        "oc_offer_positive",
        "Amount and price must be positive.",
        "Գումարը և գինը պետք է դրական լինեն:",
    );
    c.add(
        "oc_offer_offline",
        "Exchange node unavailable; saved locally until the node reconnects.",
        "Փոխանակման հանգույցն անհասանելի է, առաջարկը պահպանված է տեղայնորեն մինչև հանգույցի վերամիացումը:",
    );
    c.add(
        "oc_rewards_title",
        "Contribution rewards",
        "Ներդրման պարգևներ",
    );
    c.add(
        "oc_rewards_notice",
        "Reward = verified resource contribution + availability + actual utilization. BASIC minimum: 10 ONECOIN/month. NET+ minimum: 40–50 ONECOIN/month.",
        "Պարգև = ստուգված ռեսուրսային ներդրում + հասանելիություն + իրական օգտագործում: BASIC նվազագույնը՝ 10 ONECOIN/ամիս: NET+ նվազագույնը՝ 40–50 ONECOIN/ամիս:",
    );
    c.add("oc_storage_ph", "Storage GB", "Պահեստ GB");
    c.add("oc_cpu_ph", "CPU cores", "CPU միջուկներ");
    c.add("oc_ram_ph", "RAM GB", "RAM GB");
    c.add("oc_gpu_ph", "GPU units", "GPU միավորներ");
    c.add("oc_bw_ph", "Bandwidth TB/day", "Թողունակություն TB/օր");
    c.add("oc_usage_ph", "Utilization %", "Օգտագործում %");
    c.add("oc_calculate", "Calculate estimate", "Հաշվել գնահատականը");
    c.add(
        "oc_calc_state",
        "Informational estimate until a signed contribution receipt is accepted.",
        "Տեղեկատվական գնահատական է մինչև ստորագրված ներդրման անդորագրի ընդունումը:",
    );
    c.add(
        "oc_calc_result",
        "Estimated tier: {tier}. Illustrative reward: {amount} ONECOIN/month. Final reward requires signed usage receipts.",
        "Գնահատված մակարդակ՝ {tier}: Օրինակելի պարգև՝ {amount} ONECOIN/ամիս: Վերջնական պարգևը պահանջում է ստորագրված օգտագործման անդորագրեր:",
    );
    c.add(
        "oc_tiers_title",
        "Contribution hierarchy",
        "Ներդրման հիերարխիա",
    );
    c.add("tier_free_brief", "0 contribution", "0 ներդրում");
    c.add(
        "tier_free_res",
        "20 GB · 1 CPU · 1 GB RAM VPS",
        "20 GB · 1 CPU · 1 GB RAM VPS",
    );
    c.add(
        "tier_free_reward",
        "No contribution reward",
        "Ներդրման պարգև չկա",
    );
    c.add(
        "tier_basic_brief",
        "Any verified resource",
        "Ցանկացած ստուգված ռեսուրս",
    );
    c.add(
        "tier_basic_res",
        "Your resource + 20 GB · 2 CPU · 4 GB RAM · 24/7",
        "Քո ռեսուրսը + 20 GB · 2 CPU · 4 GB RAM · 24/7",
    );
    c.add(
        "tier_basic_reward",
        "10 ONECOIN/month minimum",
        "նվազագույնը 10 ONECOIN/ամիս",
    );
    c.add(
        "tier_netplus_brief",
        "100 GB · 4 CPU · 8 GB RAM · 1 TB+",
        "100 GB · 4 CPU · 8 GB RAM · 1 TB+",
    );
    c.add(
        "tier_netplus_res",
        "Your resource + 30 GB · 2 CPU · 8 GB RAM · 24/7",
        "Քո ռեսուրսը + 30 GB · 2 CPU · 8 GB RAM · 24/7",
    );
    c.add(
        "tier_netplus_reward",
        "40–50 ONECOIN/month minimum",
        "նվազագույնը 40–50 ONECOIN/ամիս",
    );
    c.add(
        "tier_netpro_brief",
        "1 TB+ · 8 CPU · GPU · 16 GB+ RAM",
        "1 TB+ · 8 CPU · GPU · 16 GB+ RAM",
    );
    c.add(
        "tier_netpro_res",
        "Your resource + NET-equivalent resource",
        "Քո ռեսուրսը + NET-համարժեք ռեսուրս",
    );
    c.add(
        "tier_netpro_reward",
        "Usage-based reward",
        "Օգտագործման վրա հիմնված պարգև",
    );
    c.add(
        "tier_netultra_brief",
        "Multiple nodes/servers",
        "Բազմաթիվ հանգույցներ/սերվերներ",
    );
    c.add(
        "tier_netultra_res",
        "Data-centre capabilities",
        "Տվյալների կենտրոնի հնարավորություններ",
    );
    c.add(
        "tier_netultra_reward",
        "Usage + contribution",
        "Օգտագործում + ներդրում",
    );
    c.add("tier_pro_brief", "100+ servers", "100+ սերվեր");
    c.add(
        "tier_pro_res",
        "Large infrastructure governance",
        "Խոշոր ենթակառուցվածքի կառավարում",
    );
    c.add(
        "tier_pro_reward",
        "Usage + contribution",
        "Օգտագործում + ներդրում",
    );
    c.add(
        "tier_datagroup_brief",
        "100,000+ resource units",
        "100,000+ ռեսուրսային միավոր",
    );
    c.add(
        "tier_datagroup_res",
        "Full Data Group management",
        "Ամբողջական Data Group կառավարում",
    );
    c.add(
        "tier_datagroup_reward",
        "Usage + contribution",
        "Օգտագործում + ներդրում",
    );
    c.add(
        "tier_centregroup_brief",
        "1,000,000+ resource units",
        "1,000,000+ ռեսուրսային միավոր",
    );
    c.add(
        "tier_centregroup_res",
        "Full Centre Group management",
        "Ամբողջական Centre Group կառավարում",
    );
    c.add(
        "tier_centregroup_reward",
        "Usage + contribution",
        "Օգտագործում + ներդրում",
    );
    c.add(
        "tier_awenetuser_brief",
        "Network governance",
        "Ցանցի կառավարում",
    );
    c.add(
        "tier_awenetuser_res",
        "Protocol governance; no infinite balance/resources",
        "Պրոտոկոլի կառավարում, անսահմանափակ մնացորդ/ռեսուրս չկա",
    );
    c.add(
        "tier_awenetuser_reward",
        "Governance-controlled issuance",
        "Կառավարման վերահսկվող թողարկում",
    );
    c.add(
        "oc_aweid_unavailable",
        "AWE ID unavailable",
        "AWE ID-ն անհասանելի է",
    );
    c.add(
        "oc_score",
        "Resource score · {score}",
        "Ռեսուրսի միավոր · {score}",
    );
    c.add(
        "oc_contribution_unavailable",
        "Contribution unavailable",
        "Ներդրումն անհասանելի է",
    );
    c.add("oc_ledger_offline", "Ledger offline", "Գիրքն անցանց է");
    c.add(
        "oc_start_node",
        "Start an ONEBANK-enabled node",
        "Գործարկիր ONEBANK-ով հանգույց",
    );
    c.add("oc_price_unavailable", "unavailable", "անհասանելի է");
    c.add("oc_receive_title", "Receive ONECOIN", "ONECOIN ստանալ");
    c.add(
        "oc_receive_hint",
        "Share this AWE ID to receive ONECOIN transfers. The sender can read it with Scan to pay.",
        "Կիսվիր այս AWE ID-ով ONECOIN փոխանցումներ ստանալու համար: Ուղարկողը կարող է այն կարդալ Scan to pay-ով:",
    );
    c.add("oc_close", "Close", "Փակել");
    c.add(
        "oc_wallet_unavailable",
        "Wallet unavailable: start the node backend to show your receive QR.",
        "Դրամապանակն անհասանելի է՝ գործարկիր հանգույցի backend-ը՝ QR-ը տեսնելու համար:",
    );
    c.add(
        "oc_scan_title",
        "Scan recipient QR",
        "Սկանավորել ստացողի QR-ը",
    );
    c.add(
        "oc_scan_hint",
        "Point the camera at an AWE ID QR code. Scanning stops automatically after 30 seconds.",
        "Ուղղիր տեսախցիկը AWE ID QR կոդի վրա: Սկանավորումն ավտոմատ կանգնում է 30 վայրկյանից:",
    );
    c.add("oc_cancel", "Cancel", "Չեղարկել");
    c.add(
        "oc_scan_unsupported",
        "Camera QR scanning needs a browser with BarcodeDetector and camera access; paste the recipient AWE ID instead.",
        "Տեսախցիկով QR սկանավորումը պահանջում է BarcodeDetector-ով բրաուզեր և տեսախցիկի հասանելիություն, փոխարենը կպցրուր ստացողի AWE ID-ն:",
    );
    c.add(
        "oc_scan_filled",
        "Recipient filled from QR. Review the amount and send.",
        "Ստացողը լրացված է QR-ից: Ստուգիր գումարը և ուղարկիր:",
    );
    c.add(
        "oc_scan_timeout",
        "No AWE ID QR found in 30 seconds; paste the recipient AWE ID instead.",
        "30 վայրկյանում AWE ID QR չգտնվեց, փոխարենը կպցրուր ստացողի AWE ID-ն:",
    );
    c.add(
        "oc_scan_fail",
        "Camera scanning failed: {message}; paste the recipient AWE ID instead.",
        "Տեսախցիկով սկանավորումը ձախողվեց՝ {message}, փոխարենը կպցրուր ստացողի AWE ID-ն:",
    );
    c.add("oc_local_draft", "local draft", "տեղային սևագիր");
    c.add(
        "oc_no_offers",
        "No local offers. Connected ONEBANK offers appear here.",
        "Տեղային առաջարկներ չկան: Միացված ONEBANK-ի առաջարկները հայտնվում են այստեղ:",
    );
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
        let en = c.format(Locale::En, "health_score", &args);
        assert_eq!(en, "Health score: 87");
        let hy = c.format(Locale::Hy, "health_score", &args);
        assert_eq!(hy, "Առողջության միավոր՝ 87");
        assert_eq!(c.format(Locale::En, "missing", &args), "missing");
    }

    #[test]
    fn serializes_to_json() {
        let c = product_catalog();
        let json = serde_json::to_string(&c).unwrap();
        let back: Catalog = serde_json::from_str(&json).unwrap();
        assert_eq!(back.key_count(), c.key_count());
    }

    #[test]
    fn parses_locale_codes() {
        assert_eq!(Locale::from_code("en"), Some(Locale::En));
        assert_eq!(Locale::from_code(" hy "), Some(Locale::Hy));
        assert_eq!(Locale::from_code("fr"), None);
        assert_eq!(Locale::from_code(""), None);
    }

    #[test]
    fn economy_keys_exist_in_both_locales() {
        let c = product_catalog();
        for key in [
            "lang_toggle",
            "status_published",
            "cat_engineering",
            "cx_expert",
            "kind_digital",
            "awk_hero_title",
            "mkt_confirm",
            "fnd_publish_btn",
            "oc_send",
            "tier_basic_reward",
        ] {
            assert_ne!(
                c.get(Locale::En, key),
                key,
                "missing English text for economy key {key}"
            );
            assert_ne!(
                c.get(Locale::Hy, key),
                key,
                "missing Armenian text for economy key {key}"
            );
        }
        assert!(c.key_count() >= 250);
    }

    #[test]
    fn every_catalog_entry_has_english_and_armenian_text() {
        let c = product_catalog();
        let en = c.messages(Locale::En);
        let hy = c.messages(Locale::Hy);
        assert_eq!(en.len(), c.key_count());
        assert_eq!(hy.len(), c.key_count());
        assert!(en.values().all(|text| !text.is_empty()));
        assert!(hy.values().all(|text| !text.is_empty()));
        assert_eq!(en.get("nav_home").map(String::as_str), Some("Home"));
        assert_eq!(hy.get("nav_home").map(String::as_str), Some("Գլխավոր"));
    }

    #[test]
    fn messages_map_omits_unknown_keys() {
        let c = product_catalog();
        let en = c.messages(Locale::En);
        assert!(!en.contains_key("definitely_not_a_key"));
        assert!(en.contains_key("awk_hero_sub"));
    }
}
