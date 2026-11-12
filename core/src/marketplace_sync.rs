//! Network-wide AWENET Store catalog sync between live nodes.
//!
//! `storefront.rs` owns a node's own catalog and `store_escrow.rs`
//! settles its orders on the local ONECOIN ledger. Until now that
//! market was an island: a buyer could only search products listed on
//! the node they were connected to, and a seller's storefront was
//! invisible to every other node.
//!
//! This module is the wire between seller nodes and their peers:
//!
//! - `ListingAnnounce`: a seller node announces one of its own
//!   storefront products, or the delisting of one. The announcement is
//!   signed by the seller's identity and only restates state the
//!   storefront already holds; a peer accepts it only when the
//!   announcing peer really is the seller, the signature verifies
//!   against the seller's public key (which is bound to the seller
//!   AWE-ID) and the revision is strictly newer than what the peer
//!   already holds.
//! - `RemoteCatalog`: the receiving side of a node. A bounded,
//!   last-writer-wins merge of the listings announced by its peers,
//!   with delisting tombstones so a reordered stale announcement
//!   cannot resurrect a delisted product, and a retention prune so a
//!   silent seller disappears from the catalog instead of lingering
//!   forever.
//!
//! Nothing here mints ONECOIN, changes prices, executes orders or
//! weakens the founder's exclusive rights. Store revenue stays a
//! buyer-to-seller transfer on the seller's ledger; this module only
//! moves honest catalog data between nodes. Cross-node order
//! execution (a buyer's node funding an order on the seller's node)
//! is deliberately not part of this transport: it needs an order
//! handshake and a funding protocol of its own.

use crate::identity::{AweId, Identity};
use crate::storefront::{
    Product, ProductFilter, ProductKind, Storefront, MAX_CATEGORY_CHARS, MAX_DESCRIPTION_CHARS,
    MAX_TITLE_CHARS,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Stream 203 on the authenticated node transport: storage uses 200,
/// ONECOIN transfers use 201, verifier reports use 202 and storefront
/// listings travel here.
pub const MARKETPLACE_SYNC_STREAM: u32 = 203;
pub const MARKETPLACE_SYNC_VERSION: u16 = 1;
/// The transport carries JSON messages bounded by the node policy size.
pub const MAX_SYNC_MESSAGE_BYTES: usize = 64 * 1024;
/// How many listings one seller node may keep in a peer's remote
/// catalog, tombstones included. A seller also announces at most this
/// many products per cycle, so the two bounds agree.
pub const MAX_LISTINGS_PER_SELLER: usize = 128;
/// How many remote listings one node keeps in total.
pub const MAX_REMOTE_LISTINGS: usize = 2_048;
/// A listing that has not been re-announced for this long is pruned
/// from the remote catalog: sellers re-announce every cycle, so a
/// silent seller disappears instead of lingering forever.
pub const LISTING_RETENTION_SECS: u64 = 6 * 3_600;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum MarketplaceSyncError {
    #[error("the marketplace sync message exceeds the maximum size")]
    MessageTooLarge,
    #[error("marketplace sync serialization failed: {0}")]
    Serialization(String),
    #[error("the marketplace sync message is malformed")]
    Malformed,
    #[error("the listing announcement is invalid")]
    InvalidListing,
    #[error("only the seller node itself may announce its listing")]
    ForeignListing,
    #[error("the listing announcement signature is forged")]
    ForgedListing,
    #[error("the remote catalog is full (max {MAX_REMOTE_LISTINGS} listings)")]
    TooManyListings,
}

/// The signed content of a listing announcement. The core is the exact
/// byte range the signature covers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListingAnnounceCore {
    pub version: u16,
    pub product_id: u64,
    pub seller: AweId,
    pub seller_public_key: [u8; 32],
    /// Where the seller node accepts buyers: purchases execute on the
    /// seller's ledger, so a remote listing is discovery data pointing
    /// at the seller's node, not an order route.
    pub seller_node_address: String,
    pub title: String,
    pub description: String,
    pub kind: ProductKind,
    pub category: String,
    /// The product price in ONECOIN atoms, set by the seller and
    /// unrelated to the official ONECOIN price.
    pub price_atoms: u128,
    /// `None` means unlimited availability (typical for digital goods).
    pub stock: Option<u64>,
    /// `false` announces a delisting: the product leaves every peer's
    /// catalog.
    pub listed: bool,
    /// Monotonic last-writer-wins revision. The announcing node stamps
    /// the current time; a peer only accepts strictly newer revisions.
    pub revision_unix: u64,
}

/// One signed listing announcement on the marketplace sync transport.
/// The signature is produced and checked over the flattened core.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListingAnnounce {
    #[serde(flatten)]
    pub core: ListingAnnounceCore,
    #[serde(with = "crate::serde_bytes_64")]
    pub signature: [u8; 64],
}

impl ListingAnnounce {
    /// Signs one core with the seller's identity. The signer is
    /// expected to be the node whose storefront holds the product.
    pub fn sign(
        core: ListingAnnounceCore,
        seller: &Identity,
    ) -> Result<Self, MarketplaceSyncError> {
        let bytes = serde_json::to_vec(&core)
            .map_err(|error| MarketplaceSyncError::Serialization(error.to_string()))?;
        Ok(Self {
            core,
            signature: seller.sign(&bytes),
        })
    }

    /// Builds the announcement for one storefront product of this node.
    /// The revision is the current cycle time, so every re-announcement
    /// and every change to the product is strictly newer than the
    /// previous one.
    pub fn from_product(
        product: &Product,
        seller: &Identity,
        node_address: &str,
        now_unix: u64,
    ) -> Result<Self, MarketplaceSyncError> {
        let core = ListingAnnounceCore {
            version: MARKETPLACE_SYNC_VERSION,
            product_id: product.id,
            seller: product.seller.clone(),
            seller_public_key: seller.public.public_key,
            seller_node_address: node_address.to_string(),
            title: product.title.clone(),
            description: product.description.clone(),
            kind: product.kind,
            category: product.category.clone(),
            price_atoms: product.price_atoms,
            stock: product.stock,
            listed: product.active,
            revision_unix: now_unix,
        };
        Self::sign(core, seller)
    }

    /// An announcement is only well formed when its version matches,
    /// its text fields respect the storefront bounds, its price is
    /// positive, its stock is unlimited or positive, its revision is
    /// set and its seller node address really is a socket address.
    pub fn is_well_formed(&self) -> bool {
        let core = &self.core;
        core.version == MARKETPLACE_SYNC_VERSION
            && core.product_id > 0
            && core.revision_unix > 0
            && !core.title.trim().is_empty()
            && core.title.chars().count() <= MAX_TITLE_CHARS
            && core.description.chars().count() <= MAX_DESCRIPTION_CHARS
            && core.category.chars().count() <= MAX_CATEGORY_CHARS
            && core.price_atoms > 0
            && !core.stock.is_some_and(|stock| stock == 0)
            && core
                .seller_node_address
                .parse::<std::net::SocketAddr>()
                .is_ok()
    }

    /// The signature must verify under the announced public key, and
    /// that public key must be the one the seller AWE-ID was derived
    /// from: nobody can announce listings under someone else's name.
    pub fn verify_signature(&self) -> bool {
        if AweId::from_public_key(&self.core.seller_public_key) != self.core.seller {
            return false;
        }
        match serde_json::to_vec(&self.core) {
            Ok(bytes) => Identity::verify(&self.core.seller_public_key, &bytes, &self.signature),
            Err(_) => false,
        }
    }
}

/// One message on the marketplace sync transport.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketplaceSyncMessage {
    /// A seller node announcing one of its storefront products, or
    /// the delisting of one.
    Listing(ListingAnnounce),
}

/// Encodes one transport message. The encoded form is bounded by
/// `MAX_SYNC_MESSAGE_BYTES` so a hostile peer cannot push an unbounded
/// payload through the transport.
pub fn encode(message: &MarketplaceSyncMessage) -> Result<Vec<u8>, MarketplaceSyncError> {
    let bytes = serde_json::to_vec(message)
        .map_err(|error| MarketplaceSyncError::Serialization(error.to_string()))?;
    if bytes.len() > MAX_SYNC_MESSAGE_BYTES {
        return Err(MarketplaceSyncError::MessageTooLarge);
    }
    Ok(bytes)
}

/// Decodes one transport message. Malformed or oversized input is
/// rejected without touching any catalog state.
pub fn decode(bytes: &[u8]) -> Result<MarketplaceSyncMessage, MarketplaceSyncError> {
    if bytes.len() > MAX_SYNC_MESSAGE_BYTES {
        return Err(MarketplaceSyncError::MessageTooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| MarketplaceSyncError::Malformed)
}

/// How many announcements one seller node sends per cycle: its own
/// products only, active listings first, bounded by the per-seller
/// catalog bound so sender and receiver agree.
pub const MAX_ANNOUNCES_PER_CYCLE: usize = MAX_LISTINGS_PER_SELLER;

/// The announcements a node's own storefront produces in one cycle:
/// every product the node's identity sells, active ones first (so a
/// full cycle announces everything that is for sale before it spends
/// budget on delisting tombstones), deactivated ones as delistings.
/// Products sold by other accounts on this storefront are not ours to
/// announce and are skipped.
pub fn storefront_announces(
    store: &Storefront,
    seller: &Identity,
    node_address: &str,
    now_unix: u64,
) -> Vec<ListingAnnounce> {
    let mut own: Vec<&Product> = store
        .search(&ProductFilter {
            include_inactive: true,
            ..ProductFilter::default()
        })
        .into_iter()
        .filter(|product| product.seller == seller.public.awe_id)
        .collect();
    own.sort_by_key(|product| (!product.active, product.id));
    own.truncate(MAX_ANNOUNCES_PER_CYCLE);
    own.into_iter()
        .filter_map(|product| {
            ListingAnnounce::from_product(product, seller, node_address, now_unix).ok()
        })
        .collect()
}

/// One remote listing held in a peer's catalog, together with the
/// first and last time its seller announced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteListing {
    pub core: ListingAnnounceCore,
    pub first_seen_unix: u64,
    pub last_seen_unix: u64,
}

impl RemoteListing {
    /// The remote listing as a storefront product view so the
    /// established `ProductFilter` surface applies to network listings
    /// unchanged. `sold_units` is unknown remotely and honestly zero.
    pub fn as_product(&self) -> Product {
        Product {
            id: self.core.product_id,
            seller: self.core.seller.clone(),
            title: self.core.title.clone(),
            description: self.core.description.clone(),
            kind: self.core.kind,
            category: self.core.category.clone(),
            price_atoms: self.core.price_atoms,
            stock: self.core.stock,
            sold_units: 0,
            active: self.core.listed,
            listed_at_unix: self.first_seen_unix,
        }
    }
}

/// The receiving side of one node: the storefront listings its peers
/// announced, merged last-writer-wins per seller and product, bounded
/// per seller and in total, pruned by retention. The catalog lives in
/// memory only: sellers re-announce every cycle, so a restart simply
/// starts collecting again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteCatalog {
    listings: BTreeMap<([u8; 32], u64), RemoteListing>,
}

impl RemoteCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of listings currently held, tombstones included.
    pub fn len(&self) -> usize {
        self.listings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.listings.is_empty()
    }

    /// Every listing currently held, tombstones included, in stable
    /// seller and product order.
    pub fn listings(&self) -> impl Iterator<Item = &RemoteListing> {
        self.listings.values()
    }

    /// Merges one announcement into the catalog. The sender must be
    /// the seller itself, the announcement must be well formed and
    /// correctly signed, and only a strictly newer revision changes
    /// anything. Returns whether the catalog changed. A delisting for
    /// an unknown product is a no-op; a delisting for a known product
    /// becomes a tombstone so a reordered stale listing cannot
    /// resurrect the product.
    pub fn merge(
        &mut self,
        sender: &[u8; 32],
        announce: &ListingAnnounce,
        now_unix: u64,
    ) -> Result<bool, MarketplaceSyncError> {
        if announce.core.seller.as_bytes() != sender {
            return Err(MarketplaceSyncError::ForeignListing);
        }
        if !announce.is_well_formed() {
            return Err(MarketplaceSyncError::InvalidListing);
        }
        if !announce.verify_signature() {
            return Err(MarketplaceSyncError::ForgedListing);
        }
        let key = (*sender, announce.core.product_id);
        if let Some(existing) = self.listings.get_mut(&key) {
            if announce.core.revision_unix <= existing.core.revision_unix {
                return Ok(false);
            }
            existing.core = announce.core.clone();
            existing.last_seen_unix = now_unix;
            return Ok(true);
        }
        // a delisting for a product this catalog never held has
        // nothing to delist and nothing to resurrect
        if !announce.core.listed {
            return Ok(false);
        }
        if self.listings.len() >= MAX_REMOTE_LISTINGS {
            let _ = self.prune(now_unix);
        }
        if self.listings.len() >= MAX_REMOTE_LISTINGS {
            return Err(MarketplaceSyncError::TooManyListings);
        }
        if self.seller_listing_count(sender) >= MAX_LISTINGS_PER_SELLER {
            return Err(MarketplaceSyncError::TooManyListings);
        }
        self.listings.insert(
            key,
            RemoteListing {
                core: announce.core.clone(),
                first_seen_unix: now_unix,
                last_seen_unix: now_unix,
            },
        );
        Ok(true)
    }

    /// Searches the live remote listings with the established
    /// storefront filter surface. Delisting tombstones never match:
    /// they are not products.
    pub fn search(&self, filter: &ProductFilter) -> Vec<&RemoteListing> {
        self.listings
            .values()
            .filter(|listing| listing.core.listed)
            .filter(|listing| filter.matches(&listing.as_product()))
            .collect()
    }

    /// Drops listings whose seller has not re-announced them within
    /// the retention horizon. Returns how many listings were pruned.
    pub fn prune(&mut self, now_unix: u64) -> usize {
        let expired: Vec<([u8; 32], u64)> = self
            .listings
            .iter()
            .filter(|(_, listing)| {
                now_unix
                    > listing
                        .last_seen_unix
                        .saturating_add(LISTING_RETENTION_SECS)
            })
            .map(|(key, _)| *key)
            .collect();
        let removed = expired.len();
        for key in expired {
            self.listings.remove(&key);
        }
        removed
    }

    fn seller_listing_count(&self, seller: &[u8; 32]) -> usize {
        self.listings
            .values()
            .filter(|listing| listing.core.seller.as_bytes() == seller)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Username;

    fn identity(name: &str) -> Identity {
        Identity::generate(Username::new(name).unwrap())
    }

    fn announce(
        seller: &Identity,
        product_id: u64,
        revision: u64,
        listed: bool,
    ) -> ListingAnnounce {
        ListingAnnounce::sign(
            ListingAnnounceCore {
                version: MARKETPLACE_SYNC_VERSION,
                product_id,
                seller: seller.public.awe_id.clone(),
                seller_public_key: seller.public.public_key,
                seller_node_address: "127.0.0.1:46102".into(),
                title: "AWE Theme Pack".into(),
                description: "Dark and light themes".into(),
                kind: ProductKind::Digital,
                category: "themes".into(),
                price_atoms: 250,
                stock: None,
                listed,
                revision_unix: revision,
            },
            seller,
        )
        .unwrap()
    }

    #[test]
    fn messages_roundtrip_and_reject_malformed_input() {
        let seller = identity("encode-seller");
        let message = MarketplaceSyncMessage::Listing(announce(&seller, 1, 1_000, true));
        let bytes = encode(&message).unwrap();
        assert_eq!(decode(&bytes).unwrap(), message);
        assert!(matches!(
            decode(b"not marketplace sync json"),
            Err(MarketplaceSyncError::Malformed)
        ));
        let oversized = vec![b'x'; MAX_SYNC_MESSAGE_BYTES + 1];
        assert!(matches!(
            decode(&oversized),
            Err(MarketplaceSyncError::MessageTooLarge)
        ));
    }

    #[test]
    fn listings_must_be_well_formed() {
        let seller = identity("wellformed-seller");
        let valid = announce(&seller, 1, 1_000, true);
        assert!(valid.is_well_formed());
        let mut stale_version = valid.clone();
        stale_version.core.version = MARKETPLACE_SYNC_VERSION + 1;
        assert!(!stale_version.is_well_formed());
        let mut no_title = valid.clone();
        no_title.core.title = "   ".into();
        assert!(!no_title.is_well_formed());
        let mut long_description = valid.clone();
        long_description.core.description = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert!(!long_description.is_well_formed());
        let mut zero_price = valid.clone();
        zero_price.core.price_atoms = 0;
        assert!(!zero_price.is_well_formed());
        let mut zero_stock = valid.clone();
        zero_stock.core.stock = Some(0);
        assert!(!zero_stock.is_well_formed());
        let mut zero_revision = valid.clone();
        zero_revision.core.revision_unix = 0;
        assert!(!zero_revision.is_well_formed());
        let mut bad_address = valid;
        bad_address.core.seller_node_address = "not a socket address".into();
        assert!(!bad_address.is_well_formed());
    }

    #[test]
    fn forged_and_foreign_listings_are_rejected() {
        let seller = identity("honest-seller");
        let forger = identity("forging-seller");
        let mut catalog = RemoteCatalog::new();
        let honest = announce(&seller, 1, 1_000, true);
        // a peer cannot announce a listing for someone else
        assert!(matches!(
            catalog.merge(forger.public.awe_id.as_bytes(), &honest, 1_001),
            Err(MarketplaceSyncError::ForeignListing)
        ));
        // a tampered announcement no longer matches its signature
        let mut tampered = honest.clone();
        tampered.core.price_atoms = 1;
        assert!(!tampered.verify_signature());
        assert!(matches!(
            catalog.merge(seller.public.awe_id.as_bytes(), &tampered, 1_001),
            Err(MarketplaceSyncError::ForgedListing)
        ));
        // a stolen public key cannot sign under the seller's AWE-ID
        let mut stolen = honest;
        stolen.core.seller_public_key = forger.public.public_key;
        assert!(!stolen.verify_signature());
        assert!(matches!(
            catalog.merge(seller.public.awe_id.as_bytes(), &stolen, 1_001),
            Err(MarketplaceSyncError::ForgedListing)
        ));
        // a malformed announcement never reaches the catalog
        let mut malformed = announce(&seller, 1, 1_000, true);
        malformed.core.revision_unix = 0;
        assert!(matches!(
            catalog.merge(seller.public.awe_id.as_bytes(), &malformed, 1_001),
            Err(MarketplaceSyncError::InvalidListing)
        ));
        assert!(catalog.is_empty());
    }

    #[test]
    fn catalog_merges_listings_last_writer_wins() {
        let seller = identity("merge-seller");
        let mut catalog = RemoteCatalog::new();
        let sender = seller.public.awe_id.as_bytes();
        let first = announce(&seller, 1, 1_000, true);
        assert!(catalog.merge(sender, &first, 1_005).unwrap());
        // an equal revision changes nothing
        assert!(!catalog.merge(sender, &first, 1_010).unwrap());
        // an older revision changes nothing
        let older = announce(&seller, 1, 900, true);
        assert!(!catalog.merge(sender, &older, 1_011).unwrap());
        // a newer revision updates the listing
        let mut updated = announce(&seller, 1, 1_100, true);
        updated.core.price_atoms = 300;
        updated.core.title = "AWE Theme Pack v2".into();
        assert!(catalog.merge(sender, &updated, 1_105).unwrap());
        let listings = catalog.search(&ProductFilter::default());
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0].core.title, "AWE Theme Pack v2");
        assert_eq!(listings[0].core.price_atoms, 300);
        assert_eq!(listings[0].first_seen_unix, 1_005);
        assert_eq!(listings[0].last_seen_unix, 1_105);
    }

    #[test]
    fn delisting_tombstones_prevent_resurrection() {
        let seller = identity("delist-seller");
        let mut catalog = RemoteCatalog::new();
        let sender = seller.public.awe_id.as_bytes();
        assert!(catalog
            .merge(sender, &announce(&seller, 1, 1_000, true), 1_000)
            .unwrap());
        assert_eq!(catalog.search(&ProductFilter::default()).len(), 1);
        // the seller delists the product: it leaves the visible catalog
        assert!(catalog
            .merge(sender, &announce(&seller, 1, 2_000, false), 2_000)
            .unwrap());
        assert_eq!(catalog.search(&ProductFilter::default()).len(), 0);
        // a reordered stale listing cannot resurrect the product: the
        // tombstone holds a newer revision
        assert!(!catalog
            .merge(sender, &announce(&seller, 1, 1_500, true), 2_010)
            .unwrap());
        assert_eq!(catalog.search(&ProductFilter::default()).len(), 0);
        // the tombstone itself still occupies the bounded catalog
        assert_eq!(catalog.len(), 1);
        // a delisting for an unknown product is a no-op
        assert!(!catalog
            .merge(sender, &announce(&seller, 42, 2_500, false), 2_500)
            .unwrap());
        assert_eq!(catalog.len(), 1);
    }

    #[test]
    fn catalog_is_bounded_per_seller_and_in_total() {
        let seller = identity("bound-seller");
        let mut catalog = RemoteCatalog::new();
        let sender = seller.public.awe_id.as_bytes();
        for index in 1..=MAX_LISTINGS_PER_SELLER {
            assert!(catalog
                .merge(sender, &announce(&seller, index as u64, 1_000, true), 1_000)
                .unwrap());
        }
        // one more listing from the same seller exceeds the per-seller bound
        assert!(matches!(
            catalog.merge(
                sender,
                &announce(&seller, MAX_LISTINGS_PER_SELLER as u64 + 1, 1_000, true),
                1_000
            ),
            Err(MarketplaceSyncError::TooManyListings)
        ));
        assert_eq!(catalog.len(), MAX_LISTINGS_PER_SELLER);
        // the total bound covers all sellers together
        let others =
            1..=((MAX_REMOTE_LISTINGS - MAX_LISTINGS_PER_SELLER) / MAX_LISTINGS_PER_SELLER + 1);
        for round in others {
            let other = identity(&format!("bound-other-{round}"));
            let other_sender = other.public.awe_id.as_bytes();
            for index in 1..=MAX_LISTINGS_PER_SELLER {
                let result = catalog.merge(
                    other_sender,
                    &announce(&other, index as u64, 1_000, true),
                    1_000,
                );
                if catalog.len() >= MAX_REMOTE_LISTINGS {
                    assert!(matches!(result, Err(MarketplaceSyncError::TooManyListings)));
                } else {
                    assert!(result.unwrap());
                }
            }
        }
        assert_eq!(catalog.len(), MAX_REMOTE_LISTINGS);
    }

    #[test]
    fn stale_listings_are_pruned_by_retention() {
        let seller = identity("prune-seller");
        let mut catalog = RemoteCatalog::new();
        let sender = seller.public.awe_id.as_bytes();
        assert!(catalog
            .merge(sender, &announce(&seller, 1, 100, true), 100)
            .unwrap());
        // a listing is kept while its seller re-announces within the
        // retention horizon
        assert_eq!(catalog.prune(100 + LISTING_RETENTION_SECS - 1), 0);
        assert_eq!(catalog.len(), 1);
        // and dropped once the horizon passes without a re-announce
        assert_eq!(catalog.prune(100 + LISTING_RETENTION_SECS + 1), 1);
        assert!(catalog.is_empty());
    }

    #[test]
    fn search_filters_remote_listings_like_local_products() {
        let seller_a = identity("search-seller-a");
        let seller_b = identity("search-seller-b");
        let mut catalog = RemoteCatalog::new();
        assert!(catalog
            .merge(
                seller_a.public.awe_id.as_bytes(),
                &announce(&seller_a, 1, 1_000, true),
                1_000
            )
            .unwrap());
        let mut service = announce(&seller_b, 1, 1_000, true);
        service.core.kind = ProductKind::Service;
        service.core.title = "Node repair service".into();
        service.core.category = "support".into();
        service.core.price_atoms = 900;
        assert!(catalog
            .merge(seller_b.public.awe_id.as_bytes(), &service, 1_000)
            .unwrap());
        let all = catalog.search(&ProductFilter::default());
        assert_eq!(all.len(), 2);
        let by_query = catalog.search(&ProductFilter {
            query: "THEME".into(),
            ..ProductFilter::default()
        });
        assert_eq!(by_query.len(), 1);
        assert_eq!(by_query[0].core.seller, seller_a.public.awe_id);
        assert_eq!(by_query[0].core.seller_node_address, "127.0.0.1:46102");
        let by_kind = catalog.search(&ProductFilter {
            kind: Some(ProductKind::Service),
            ..ProductFilter::default()
        });
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].core.title, "Node repair service");
        let by_price = catalog.search(&ProductFilter {
            min_price_atoms: 500,
            ..ProductFilter::default()
        });
        assert_eq!(by_price.len(), 1);
        assert_eq!(by_price[0].core.price_atoms, 900);
        let by_seller = catalog.search(&ProductFilter {
            seller: Some(seller_b.public.awe_id.clone()),
            ..ProductFilter::default()
        });
        assert_eq!(by_seller.len(), 1);
        assert_eq!(by_seller[0].core.seller, seller_b.public.awe_id);
    }

    #[test]
    fn storefront_announces_own_products_active_first_and_bounded() {
        let seller = identity("announce-seller");
        let stranger = identity("announce-stranger");
        let mut store = Storefront::new();
        let theme = store
            .list_product(
                &seller.public.awe_id,
                "AWE Theme Pack",
                "Themes",
                ProductKind::Digital,
                "themes",
                250,
                None,
                1,
            )
            .unwrap();
        store
            .list_product(
                &seller.public.awe_id,
                "Node repair service",
                "Repairs",
                ProductKind::Service,
                "support",
                900,
                Some(3),
                2,
            )
            .unwrap();
        let delisted = store
            .list_product(
                &seller.public.awe_id,
                "Old poster",
                "Retired",
                ProductKind::Physical,
                "prints",
                5,
                None,
                3,
            )
            .unwrap();
        store
            .deactivate_product(&seller.public.awe_id, delisted)
            .unwrap();
        // a product sold by another account is not ours to announce
        store
            .list_product(
                &stranger.public.awe_id,
                "Stranger item",
                "",
                ProductKind::Digital,
                "",
                5,
                None,
                4,
            )
            .unwrap();
        let announces = storefront_announces(&store, &seller, "127.0.0.1:46102", 10_000);
        assert_eq!(announces.len(), 3);
        assert!(announces.iter().all(|announce| announce.verify_signature()));
        // active listings come first, the delisting comes last
        assert!(announces[0].core.listed);
        assert!(announces[1].core.listed);
        assert!(!announces[2].core.listed);
        assert_eq!(announces[0].core.product_id, theme);
        assert_eq!(announces[2].core.product_id, delisted);
        // every announcement is a faithful copy of the storefront state
        assert_eq!(announces[0].core.seller, seller.public.awe_id);
        assert_eq!(announces[0].core.price_atoms, 250);
        // the per-cycle bound keeps a huge storefront bounded: active
        // listings are announced first
        let mut big = Storefront::new();
        for index in 0..(MAX_ANNOUNCES_PER_CYCLE + 5) {
            big.list_product(
                &seller.public.awe_id,
                &format!("Item {index}"),
                "",
                ProductKind::Digital,
                "",
                1,
                None,
                index as u64,
            )
            .unwrap();
        }
        let bounded = storefront_announces(&big, &seller, "127.0.0.1:46102", 20_000);
        assert_eq!(bounded.len(), MAX_ANNOUNCES_PER_CYCLE);
        assert!(bounded.iter().all(|announce| announce.core.listed));
    }

    #[test]
    fn a_live_marketplace_flow_syncs_a_listing_across_nodes() {
        // The full wire flow: the seller node lists a product, its
        // cycle announces it, the peer's catalog merges it and a
        // buyer-side search finds it with the seller's node address;
        // the seller then delists it and the next cycle removes it
        // from the peer's catalog.
        let seller = identity("flow-seller");
        let peer = identity("flow-peer");
        let mut store = Storefront::new();
        let product_id = store
            .list_product(
                &seller.public.awe_id,
                "AWE Theme Pack",
                "Themes",
                ProductKind::Digital,
                "themes",
                250,
                None,
                1,
            )
            .unwrap();
        let announces = storefront_announces(&store, &seller, "127.0.0.1:46102", 10_000);
        assert_eq!(announces.len(), 1);
        let wire = encode(&MarketplaceSyncMessage::Listing(announces[0].clone())).unwrap();
        let decoded = decode(&wire).unwrap();
        let MarketplaceSyncMessage::Listing(announce) = decoded;
        let mut catalog = RemoteCatalog::new();
        assert!(catalog
            .merge(seller.public.awe_id.as_bytes(), &announce, 10_001)
            .unwrap());
        let found = catalog.search(&ProductFilter {
            seller: Some(seller.public.awe_id.clone()),
            ..ProductFilter::default()
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].core.product_id, product_id);
        assert_eq!(found[0].core.seller_node_address, "127.0.0.1:46102");
        assert_ne!(found[0].core.seller, peer.public.awe_id);
        // the seller delists the product and the next cycle clears it
        store
            .deactivate_product(&seller.public.awe_id, product_id)
            .unwrap();
        let delistings = storefront_announces(&store, &seller, "127.0.0.1:46102", 20_000);
        assert_eq!(delistings.len(), 1);
        assert!(!delistings[0].core.listed);
        let wire = encode(&MarketplaceSyncMessage::Listing(delistings[0].clone())).unwrap();
        let MarketplaceSyncMessage::Listing(announce) = decode(&wire).unwrap();
        assert!(catalog
            .merge(seller.public.awe_id.as_bytes(), &announce, 20_001)
            .unwrap());
        assert_eq!(catalog.search(&ProductFilter::default()).len(), 0);
    }
}
