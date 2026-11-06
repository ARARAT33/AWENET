//! AWENET Store catalog: products, seller pages and search.
//!
//! The storefront is the catalog layer of the AWENET Store. It owns what is
//! for sale (digital goods, services, physical items), who sells it and at
//! what ONECOIN price, plus the search and filter surface buyers use. It
//! deliberately does not move money: payments, escrow holds, delivery
//! confirmation, refunds and disputes are handled by `store_escrow.rs` over
//! the ONECOIN ledger, so store revenue is always a buyer-to-seller
//! transfer and never new issuance.
//!
//! Deterministic and persistence-friendly: pure data, no wall clock and no
//! threads; time is supplied by the caller and the whole catalog
//! serializes with serde.

use crate::identity::AweId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub const MAX_PRODUCTS: usize = 10_000;
pub const MAX_TITLE_CHARS: usize = 128;
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_CATEGORY_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum StorefrontError {
    #[error("store catalog is full (max {MAX_PRODUCTS} products)")]
    RegistryFull,
    #[error("product title must be 1-{MAX_TITLE_CHARS} characters")]
    TitleTooLong,
    #[error("product description exceeds {MAX_DESCRIPTION_CHARS} characters")]
    DescriptionTooLong,
    #[error("product category exceeds {MAX_CATEGORY_CHARS} characters")]
    CategoryTooLong,
    #[error("product {0} is unknown")]
    UnknownProduct(u64),
    #[error("only the seller of product {0} may modify it")]
    NotSeller(u64),
    #[error("product price must be positive")]
    ZeroPrice,
    #[error("product {0} is not active")]
    NotActive(u64),
    #[error("product {0} has insufficient stock")]
    InsufficientStock(u64),
    #[error("purchase units must be positive")]
    ZeroUnits,
    #[error("product {0} is your own listing; buyer and seller must differ")]
    SelfPurchase(u64),
}

/// What kind of item a product is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductKind {
    Digital,
    Service,
    Physical,
}

impl ProductKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Digital => "digital",
            Self::Service => "service",
            Self::Physical => "physical",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "digital" => Some(Self::Digital),
            "service" => Some(Self::Service),
            "physical" => Some(Self::Physical),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Product {
    pub id: u64,
    pub seller: AweId,
    pub title: String,
    pub description: String,
    pub kind: ProductKind,
    pub category: String,
    pub price_atoms: u128,
    /// `None` means unlimited availability (typical for digital goods).
    pub stock: Option<u64>,
    pub sold_units: u64,
    pub active: bool,
    pub listed_at_unix: u64,
}

/// Case-insensitive search and filter surface over the catalog. The default
/// filter matches every active product.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProductFilter {
    /// Case-insensitive substring match over title, description and category.
    pub query: String,
    pub kind: Option<ProductKind>,
    pub category: Option<String>,
    pub seller: Option<AweId>,
    pub min_price_atoms: u128,
    pub max_price_atoms: Option<u128>,
    pub include_inactive: bool,
}

impl ProductFilter {
    pub fn matches(&self, product: &Product) -> bool {
        if !self.query.trim().is_empty() {
            let needle = self.query.trim().to_ascii_lowercase();
            let haystack = format!(
                "{} {} {}",
                product.title,
                product.description,
                product.category
            )
            .to_ascii_lowercase();
            if !haystack.contains(&needle) {
                return false;
            }
        }
        if self.kind.is_some_and(|kind| product.kind != kind) {
            return false;
        }
        if let Some(category) = &self.category {
            if !category.trim().is_empty()
                && !product
                    .category
                    .eq_ignore_ascii_case(category.trim())
            {
                return false;
            }
        }
        if self
            .seller
            .as_ref()
            .is_some_and(|seller| product.seller != *seller)
        {
            return false;
        }
        if product.price_atoms < self.min_price_atoms {
            return false;
        }
        if self
            .max_price_atoms
            .is_some_and(|max| product.price_atoms > max)
        {
            return false;
        }
        if !self.include_inactive && !product.active {
            return false;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SellerStats {
    pub products: usize,
    pub active_products: usize,
    pub sold_units: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Storefront {
    next_product_id: u64,
    products: BTreeMap<u64, Product>,
}

impl Storefront {
    pub fn new() -> Self {
        Self {
            next_product_id: 1,
            products: BTreeMap::new(),
        }
    }

    pub fn product(&self, id: u64) -> Option<&Product> {
        self.products.get(&id)
    }

    pub fn product_count(&self) -> usize {
        self.products.len()
    }

    pub fn search(&self, filter: &ProductFilter) -> Vec<&Product> {
        self.products
            .values()
            .filter(|product| filter.matches(product))
            .collect()
    }

    pub fn seller_products(&self, seller: &AweId) -> Vec<&Product> {
        self.products
            .values()
            .filter(|product| product.seller == *seller)
            .collect()
    }

    pub fn seller_stats(&self, seller: &AweId) -> SellerStats {
        let mut stats = SellerStats::default();
        for product in self.products.values() {
            if product.seller == *seller {
                stats.products += 1;
                stats.active_products += usize::from(product.active);
                stats.sold_units = stats.sold_units.saturating_add(product.sold_units);
            }
        }
        stats
    }

    /// A seller lists a product. The price is a product price in ONECOIN
    /// atoms, set by the seller and unrelated to the official ONECOIN price.
    pub fn list_product(
        &mut self,
        seller: &AweId,
        title: &str,
        description: &str,
        kind: ProductKind,
        category: &str,
        price_atoms: u128,
        stock: Option<u64>,
        now_unix: u64,
    ) -> Result<u64, StorefrontError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
            return Err(StorefrontError::TitleTooLong);
        }
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(StorefrontError::DescriptionTooLong);
        }
        let category = category.trim();
        if category.chars().count() > MAX_CATEGORY_CHARS {
            return Err(StorefrontError::CategoryTooLong);
        }
        if price_atoms == 0 {
            return Err(StorefrontError::ZeroPrice);
        }
        if stock.is_some_and(|stock| stock == 0) {
            return Err(StorefrontError::InsufficientStock(0));
        }
        if self.products.len() >= MAX_PRODUCTS {
            return Err(StorefrontError::RegistryFull);
        }
        let id = self.next_product_id;
        self.next_product_id = self.next_product_id.saturating_add(1);
        self.products.insert(
            id,
            Product {
                id,
                seller: seller.clone(),
                title: title.to_string(),
                description: description.trim().to_string(),
                kind,
                category: category.to_string(),
                price_atoms,
                stock,
                sold_units: 0,
                active: true,
                listed_at_unix: now_unix,
            },
        );
        Ok(id)
    }

    /// The seller of a product is the only account that may update it.
    #[allow(clippy::too_many_arguments)]
    pub fn update_product(
        &mut self,
        seller: &AweId,
        id: u64,
        title: &str,
        description: &str,
        category: &str,
        price_atoms: u128,
        stock: Option<u64>,
        active: bool,
    ) -> Result<(), StorefrontError> {
        let product = self
            .products
            .get_mut(&id)
            .ok_or(StorefrontError::UnknownProduct(id))?;
        if product.seller != *seller {
            return Err(StorefrontError::NotSeller(id));
        }
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
            return Err(StorefrontError::TitleTooLong);
        }
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            return Err(StorefrontError::DescriptionTooLong);
        }
        let category = category.trim();
        if category.chars().count() > MAX_CATEGORY_CHARS {
            return Err(StorefrontError::CategoryTooLong);
        }
        if price_atoms == 0 {
            return Err(StorefrontError::ZeroPrice);
        }
        if stock.is_some_and(|stock| stock == 0) {
            return Err(StorefrontError::InsufficientStock(id));
        }
        product.title = title.to_string();
        product.description = description.trim().to_string();
        product.category = category.to_string();
        product.price_atoms = price_atoms;
        product.stock = stock;
        product.active = active;
        Ok(())
    }

    pub fn deactivate_product(&mut self, seller: &AweId, id: u64) -> Result<(), StorefrontError> {
        let product = self
            .products
            .get_mut(&id)
            .ok_or(StorefrontError::UnknownProduct(id))?;
        if product.seller != *seller {
            return Err(StorefrontError::NotSeller(id));
        }
        product.active = false;
        Ok(())
    }

    /// Records a sale against a listing and returns the total price atoms.
    /// The escrow layer performs the actual payment; this only keeps the
    /// catalog consistent with what was sold.
    pub fn record_sale(
        &mut self,
        buyer: &AweId,
        id: u64,
        units: u64,
    ) -> Result<u128, StorefrontError> {
        if units == 0 {
            return Err(StorefrontError::ZeroUnits);
        }
        let product = self
            .products
            .get_mut(&id)
            .ok_or(StorefrontError::UnknownProduct(id))?;
        if product.seller == *buyer {
            return Err(StorefrontError::SelfPurchase(id));
        }
        if !product.active {
            return Err(StorefrontError::NotActive(id));
        }
        if let Some(stock) = product.stock {
            if stock < units {
                return Err(StorefrontError::InsufficientStock(id));
            }
            product.stock = Some(stock - units);
        }
        product.sold_units = product.sold_units.saturating_add(units);
        Ok(product
            .price_atoms
            .saturating_mul(u128::from(units)))
    }
}

impl Default for Storefront {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Identity, Username};

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn seller() -> AweId {
        identity("store-seller").public.awe_id
    }

    fn buyer() -> AweId {
        identity("store-buyer").public.awe_id
    }

    #[test]
    fn listing_validates_and_persists() {
        let seller = seller();
        let mut store = Storefront::new();
        assert_eq!(
            store
                .list_product(&seller, "  ", "", ProductKind::Digital, "apps", 5, None, 1)
                .unwrap_err(),
            StorefrontError::TitleTooLong
        );
        assert_eq!(
            store
                .list_product(&seller, "App", "", ProductKind::Digital, "apps", 0, None, 1)
                .unwrap_err(),
            StorefrontError::ZeroPrice
        );
        let long = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert_eq!(
            store
                .list_product(&seller, "App", &long, ProductKind::Digital, "apps", 5, None, 1)
                .unwrap_err(),
            StorefrontError::DescriptionTooLong
        );
        assert_eq!(
            store
                .list_product(&seller, "App", "", ProductKind::Digital, "apps", 5, Some(0), 1)
                .unwrap_err(),
            StorefrontError::InsufficientStock(0)
        );
        let id = store
            .list_product(
                &seller,
                "  AWE Theme Pack  ",
                "Dark and light themes",
                ProductKind::Digital,
                "  themes  ",
                250,
                None,
                10,
            )
            .unwrap();
        let product = store.product(id).unwrap();
        assert_eq!(product.title, "AWE Theme Pack");
        assert_eq!(product.category, "themes");
        assert_eq!(product.price_atoms, 250);
        assert!(product.active);
        assert_eq!(store.product_count(), 1);
    }

    #[test]
    fn search_filters_are_case_insensitive_and_composable() {
        let seller = seller();
        let other = identity("store-other").public.awe_id;
        let mut store = Storefront::new();
        let theme = store
            .list_product(
                &seller,
                "AWE Theme Pack",
                "UI themes for the desktop app",
                ProductKind::Digital,
                "Themes",
                250,
                None,
                1,
            )
            .unwrap();
        let repair = store
            .list_product(
                &other,
                "Node repair service",
                "We repair broken replication",
                ProductKind::Service,
                "Support",
                900,
                Some(3),
                1,
            )
            .unwrap();
        store.deactivate_product(&other, repair).unwrap();

        let all = store.search(&ProductFilter::default());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, theme);

        let by_query = store.search(&ProductFilter {
            query: "THEME".into(),
            ..Default::default()
        });
        assert_eq!(by_query.len(), 1);
        assert_eq!(by_query[0].id, theme);

        let by_kind = store.search(&ProductFilter {
            kind: Some(ProductKind::Service),
            include_inactive: true,
            ..Default::default()
        });
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].id, repair);

        let by_seller = store.search(&ProductFilter {
            seller: Some(seller.clone()),
            ..Default::default()
        });
        assert_eq!(by_seller.len(), 1);
        assert_eq!(by_seller[0].id, theme);

        let by_price = store.search(&ProductFilter {
            min_price_atoms: 300,
            include_inactive: true,
            ..Default::default()
        });
        assert_eq!(by_price.len(), 1);
        assert_eq!(by_price[0].id, repair);

        let by_max_price = store.search(&ProductFilter {
            max_price_atoms: Some(300),
            ..Default::default()
        });
        assert_eq!(by_max_price.len(), 1);
        assert_eq!(by_max_price[0].id, theme);
    }

    #[test]
    fn only_the_seller_may_update_or_deactivate() {
        let seller = seller();
        let stranger = buyer();
        let mut store = Storefront::new();
        let id = store
            .list_product(&seller, "App", "desc", ProductKind::Digital, "apps", 5, None, 1)
            .unwrap();
        assert_eq!(
            store
                .update_product(&stranger, id, "App", "desc", "apps", 6, None, true)
                .unwrap_err(),
            StorefrontError::NotSeller(id)
        );
        assert_eq!(
            store.deactivate_product(&stranger, id).unwrap_err(),
            StorefrontError::NotSeller(id)
        );
        store
            .update_product(&seller, id, "App v2", "desc", "apps", 6, None, true)
            .unwrap();
        assert_eq!(store.product(id).unwrap().price_atoms, 6);
        store.deactivate_product(&seller, id).unwrap();
        assert!(!store.product(id).unwrap().active);
        assert_eq!(
            store.record_sale(&buyer(), id, 1).unwrap_err(),
            StorefrontError::NotActive(id)
        );
    }

    #[test]
    fn sales_consume_stock_and_reject_self_purchase() {
        let seller = seller();
        let buyer = buyer();
        let mut store = Storefront::new();
        let limited = store
            .list_product(
                &seller,
                "Cables",
                "Spare cables",
                ProductKind::Physical,
                "hardware",
                7,
                Some(2),
                1,
            )
            .unwrap();
        let unlimited = store
            .list_product(
                &seller,
                "E-book",
                "Guide",
                ProductKind::Digital,
                "books",
                3,
                None,
                1,
            )
            .unwrap();
        assert_eq!(
            store.record_sale(&seller, limited, 1).unwrap_err(),
            StorefrontError::SelfPurchase(limited)
        );
        assert_eq!(store.record_sale(&buyer, limited, 2).unwrap(), 14);
        assert_eq!(store.product(limited).unwrap().stock, Some(0));
        assert_eq!(
            store.record_sale(&buyer, limited, 1).unwrap_err(),
            StorefrontError::InsufficientStock(limited)
        );
        assert_eq!(store.record_sale(&buyer, unlimited, 4).unwrap(), 12);
        assert_eq!(store.product(unlimited).unwrap().stock, None);
        assert_eq!(store.product(unlimited).unwrap().sold_units, 4);
        assert_eq!(
            store.record_sale(&buyer, unlimited, 0).unwrap_err(),
            StorefrontError::ZeroUnits
        );
    }

    #[test]
    fn catalog_is_bounded() {
        let seller = seller();
        let mut store = Storefront::new();
        for _ in 0..MAX_PRODUCTS {
            store
                .list_product(&seller, "Item", "", ProductKind::Digital, "", 1, None, 0)
                .unwrap();
        }
        assert_eq!(
            store
                .list_product(&seller, "Item", "", ProductKind::Digital, "", 1, None, 0)
                .unwrap_err(),
            StorefrontError::RegistryFull
        );
        assert_eq!(store.product_count(), MAX_PRODUCTS);
    }

    #[test]
    fn seller_stats_reflect_listings() {
        let seller = seller();
        let buyer = buyer();
        let mut store = Storefront::new();
        let a = store
            .list_product(&seller, "A", "", ProductKind::Digital, "", 5, None, 1)
            .unwrap();
        store
            .list_product(&seller, "B", "", ProductKind::Digital, "", 5, None, 1)
            .unwrap();
        store
            .list_product(&buyer, "C", "", ProductKind::Digital, "", 5, None, 1)
            .unwrap();
        store.record_sale(&buyer, a, 3).unwrap();
        let stats = store.seller_stats(&seller);
        assert_eq!(stats.products, 2);
        assert_eq!(stats.active_products, 2);
        assert_eq!(stats.sold_units, 3);
        assert_eq!(store.seller_products(&seller).len(), 2);
    }
}
