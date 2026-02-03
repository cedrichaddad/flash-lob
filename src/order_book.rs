//! Order Book - The central limit order book data structure.
//!
//! Maintains bid and ask price levels with O(1) best-price access
//! and O(1) order lookup for cancellation.

use rustc_hash::FxHashMap;
use crate::arena::{Arena, ArenaIndex};
use crate::command::Side;
use crate::price_level::PriceLevel;

/// Mapping from OrderId to ArenaIndex for O(1) cancel lookup
pub type OrderMap = FxHashMap<u64, ArenaIndex>;

/// Order metadata stored alongside the arena index
#[derive(Clone, Copy, Debug)]
pub struct OrderInfo {
    /// Index in the arena
    pub arena_index: ArenaIndex,
    /// Order side (needed for cancel to find correct book side)
    pub side: Side,
    /// Price level (needed for cancel to find the PriceLevel)
    pub price: u64,
    /// User ID (needed for modify order)
    pub user_id: u64,
}

/// Sparse Order Book using HashMaps for price levels.
///
/// Best for assets with large price ranges (crypto, FX).
/// Uses FxHashMap for fast non-cryptographic hashing.
pub struct OrderBook {
    /// Bid price levels (buy orders)
    pub bids: FxHashMap<u64, PriceLevel>,
    /// Ask price levels (sell orders)
    pub asks: FxHashMap<u64, PriceLevel>,
    /// Cached best bid price (highest buy price)
    best_bid: Option<u64>,
    /// Cached best ask price (lowest sell price)
    best_ask: Option<u64>,
    /// Order lookup map: OrderId -> OrderInfo
    order_map: FxHashMap<u64, OrderInfo>,
}

impl OrderBook {
    /// Create a new empty order book
    pub fn new() -> Self {
        Self {
            bids: FxHashMap::default(),
            asks: FxHashMap::default(),
            best_bid: None,
            best_ask: None,
            order_map: FxHashMap::default(),
        }
    }
    
    /// Create a new order book with pre-allocated capacity
    pub fn with_capacity(levels: usize, orders: usize) -> Self {
        Self {
            bids: FxHashMap::with_capacity_and_hasher(levels, Default::default()),
            asks: FxHashMap::with_capacity_and_hasher(levels, Default::default()),
            best_bid: None,
            best_ask: None,
            order_map: FxHashMap::with_capacity_and_hasher(orders, Default::default()),
        }
    }
    
    // ========================================================================
    // Best Price Access
    // ========================================================================
    
    /// Get the best bid price (highest buy price)
    #[inline]
    pub fn best_bid(&self) -> Option<u64> {
        self.best_bid
    }
    
    /// Get the best ask price (lowest sell price)
    #[inline]
    pub fn best_ask(&self) -> Option<u64> {
        self.best_ask
    }
    
    /// Get the best price on a given side
    #[inline]
    pub fn best_price(&self, side: Side) -> Option<u64> {
        match side {
            Side::Bid => self.best_bid,
            Side::Ask => self.best_ask,
        }
    }
    
    /// Get the best opposite price (for matching)
    #[inline]
    pub fn best_opposite_price(&self, side: Side) -> Option<u64> {
        match side {
            Side::Bid => self.best_ask,  // Buyer matches with lowest ask
            Side::Ask => self.best_bid,  // Seller matches with highest bid
        }
    }
    
    // ========================================================================
    // Level Access
    // ========================================================================
    
    /// Get a price level (immutable)
    #[inline]
    pub fn get_level(&self, side: Side, price: u64) -> Option<&PriceLevel> {
        match side {
            Side::Bid => self.bids.get(&price),
            Side::Ask => self.asks.get(&price),
        }
    }
    
    /// Get a price level (mutable)
    #[inline]
    pub fn get_level_mut(&mut self, side: Side, price: u64) -> Option<&mut PriceLevel> {
        match side {
            Side::Bid => self.bids.get_mut(&price),
            Side::Ask => self.asks.get_mut(&price),
        }
    }
    
    /// Get or create a price level
    #[inline]
    pub fn get_or_create_level(&mut self, side: Side, price: u64) -> &mut PriceLevel {
        match side {
            Side::Bid => self.bids.entry(price).or_insert_with(PriceLevel::new),
            Side::Ask => self.asks.entry(price).or_insert_with(PriceLevel::new),
        }
    }
    
    // ========================================================================
    // Order Management
    // ========================================================================
    
    /// Add an order to the book.
    ///
    /// # Arguments
    /// * `arena` - The arena containing order nodes
    /// * `order_id` - External order ID
    /// * `side` - Order side
    /// * `price` - Order price
    /// * `arena_index` - Index of the order in the arena
    ///
    /// # Returns
    /// `true` if order was added, `false` if order_id already exists
    pub fn add_order(
        &mut self,
        arena: &mut Arena,
        order_id: u64,
        user_id: u64,
        side: Side,
        price: u64,
        arena_index: ArenaIndex,
    ) -> bool {
        // Check for duplicate order ID
        if self.order_map.contains_key(&order_id) {
            return false;
        }
        
        // Add to order lookup map
        self.order_map.insert(order_id, OrderInfo {
            arena_index,
            side,
            price,
            user_id,
        });
        
        // Add to price level
        let level = self.get_or_create_level(side, price);
        level.push_back(arena, arena_index);
        
        // Update best price cache
        self.update_best_price_on_add(side, price);
        
        true
    }
    
    /// Remove an order from the book (for cancel).
    ///
    /// # Arguments
    /// * `arena` - The arena containing order nodes
    /// * `order_id` - External order ID to remove
    ///
    /// # Returns
    /// The removed order's info if found, or `None` if not found
    pub fn remove_order(&mut self, arena: &mut Arena, order_id: u64) -> Option<OrderInfo> {
        // Look up order
        let info = self.order_map.remove(&order_id)?;
        
        // Remove from price level
        let level = match info.side {
            Side::Bid => self.bids.get_mut(&info.price),
            Side::Ask => self.asks.get_mut(&info.price),
        };
        
        if let Some(level) = level {
            let is_empty = level.remove(arena, info.arena_index);
            
            // Clean up empty level and update best price
            if is_empty {
                self.remove_empty_level(info.side, info.price);
            }
        }
        
        Some(info)
    }
    
    /// Look up an order by ID.
    #[inline]
    pub fn get_order(&self, order_id: u64) -> Option<&OrderInfo> {
        self.order_map.get(&order_id)
    }
    
    /// Check if an order exists.
    #[inline]
    pub fn contains_order(&self, order_id: u64) -> bool {
        self.order_map.contains_key(&order_id)
    }
    
    /// Remove an order from the order map only (after matching).
    /// Call this when an order is fully filled during matching.
    #[inline]
    pub fn remove_order_from_map(&mut self, order_id: u64) {
        self.order_map.remove(&order_id);
    }
    
    // ========================================================================
    // Level Removal
    // ========================================================================
    
    /// Remove an empty price level and update best price if needed.
    pub fn remove_empty_level(&mut self, side: Side, price: u64) {
        match side {
            Side::Bid => {
                self.bids.remove(&price);
                if self.best_bid == Some(price) {
                    self.recalculate_best_bid();
                }
            }
            Side::Ask => {
                self.asks.remove(&price);
                if self.best_ask == Some(price) {
                    self.recalculate_best_ask();
                }
            }
        }
    }
    
    // ========================================================================
    // Best Price Management
    // ========================================================================
    
    /// Update best price cache when adding an order.
    fn update_best_price_on_add(&mut self, side: Side, price: u64) {
        match side {
            Side::Bid => {
                if self.best_bid.map_or(true, |best| price > best) {
                    self.best_bid = Some(price);
                }
            }
            Side::Ask => {
                if self.best_ask.map_or(true, |best| price < best) {
                    self.best_ask = Some(price);
                }
            }
        }
    }
    
    /// Recalculate best bid price by scanning all bid levels.
    /// Called when the current best bid level becomes empty.
    fn recalculate_best_bid(&mut self) {
        self.best_bid = self.bids.keys().copied().max();
    }
    
    /// Recalculate best ask price by scanning all ask levels.
    /// Called when the current best ask level becomes empty.
    fn recalculate_best_ask(&mut self) {
        self.best_ask = self.asks.keys().copied().min();
    }
    
    // ========================================================================
    // Utility Methods
    // ========================================================================
    
    /// Get the total number of orders in the book
    pub fn order_count(&self) -> usize {
        self.order_map.len()
    }
    
    /// Get the number of bid levels
    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }
    
    /// Get the number of ask levels
    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }
    
    /// Check if the book is empty
    pub fn is_empty(&self) -> bool {
        self.order_map.is_empty()
    }
    
    /// Clear all orders from the book
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.best_bid = None;
        self.best_ask = None;
        self.order_map.clear();
    }
    
    /// Calculate spread (best_ask - best_bid)
    pub fn spread(&self) -> Option<u64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) if ask > bid => Some(ask - bid),
            _ => None,
        }
    }
    
    /// Get depth at a price level
    pub fn depth_at(&self, side: Side, price: u64) -> (u64, u32) {
        self.get_level(side, price)
            .map(|l| (l.total_qty, l.count))
            .unwrap_or((0, 0))
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for OrderBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrderBook")
            .field("best_bid", &self.best_bid)
            .field("best_ask", &self.best_ask)
            .field("bid_levels", &self.bids.len())
            .field("ask_levels", &self.asks.len())
            .field("order_count", &self.order_map.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;
    
    fn create_order(arena: &mut Arena, order_id: u64, price: u64, qty: u32) -> ArenaIndex {
        let idx = arena.alloc().unwrap();
        let node = arena.get_mut(idx);
        node.order_id = order_id;
        node.price = price;
        node.qty = qty;
        node.user_id = 1;
        idx
    }
    
    #[test]
    fn test_empty_book() {
        let book = OrderBook::new();
        assert!(book.is_empty());
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.spread(), None);
    }
    
    #[test]
    fn test_add_bid_order() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        let idx = create_order(&mut arena, 1, 10000, 100);
        assert!(book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx));
        
        assert_eq!(book.best_bid(), Some(10000));
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.order_count(), 1);
        assert!(book.contains_order(1));
    }
    
    #[test]
    fn test_add_ask_order() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        let idx = create_order(&mut arena, 1, 10100, 100);
        assert!(book.add_order(&mut arena, 1, 1, Side::Ask, 10100, idx));
        
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), Some(10100));
        assert_eq!(book.order_count(), 1);
    }
    
    #[test]
    fn test_best_price_updates() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        // Add bids at different prices
        let idx1 = create_order(&mut arena, 1, 10000, 100);
        let idx2 = create_order(&mut arena, 2, 10050, 100);
        let idx3 = create_order(&mut arena, 3, 9950, 100);
        
        book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx1);
        assert_eq!(book.best_bid(), Some(10000));
        
        book.add_order(&mut arena, 2, 1, Side::Bid, 10050, idx2);
        assert_eq!(book.best_bid(), Some(10050)); // Higher is better for bids
        
        book.add_order(&mut arena, 3, 1, Side::Bid, 9950, idx3);
        assert_eq!(book.best_bid(), Some(10050)); // Still 10050
        
        // Add asks
        let idx4 = create_order(&mut arena, 4, 10100, 100);
        let idx5 = create_order(&mut arena, 5, 10080, 100);
        
        book.add_order(&mut arena, 4, 1, Side::Ask, 10100, idx4);
        assert_eq!(book.best_ask(), Some(10100));
        
        book.add_order(&mut arena, 5, 1, Side::Ask, 10080, idx5);
        assert_eq!(book.best_ask(), Some(10080)); // Lower is better for asks
    }
    
    #[test]
    fn test_spread() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        let idx1 = create_order(&mut arena, 1, 10000, 100);
        let idx2 = create_order(&mut arena, 2, 10100, 100);
        
        book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx1);
        book.add_order(&mut arena, 2, 1, Side::Ask, 10100, idx2);
        
        assert_eq!(book.spread(), Some(100));
    }
    
    #[test]
    fn test_duplicate_order_id() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        let idx1 = create_order(&mut arena, 1, 10000, 100);
        let idx2 = create_order(&mut arena, 1, 10050, 100); // Same order_id
        
        assert!(book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx1));
        assert!(!book.add_order(&mut arena, 1, 1, Side::Bid, 10050, idx2)); // Should fail
        
        assert_eq!(book.order_count(), 1);
    }
    
    #[test]
    fn test_remove_order() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        let idx = create_order(&mut arena, 1, 10000, 100);
        book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx);
        
        let info = book.remove_order(&mut arena, 1);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.arena_index, idx);
        assert_eq!(info.side, Side::Bid);
        assert_eq!(info.price, 10000);
        
        assert!(book.is_empty());
        assert_eq!(book.best_bid(), None);
    }
    
    #[test]
    fn test_remove_nonexistent_order() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        assert!(book.remove_order(&mut arena, 999).is_none());
    }
    
    #[test]
    fn test_best_price_recalculation() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        // Add multiple bid levels
        let idx1 = create_order(&mut arena, 1, 10050, 100);
        let idx2 = create_order(&mut arena, 2, 10000, 100);
        let idx3 = create_order(&mut arena, 3, 9950, 100);
        
        book.add_order(&mut arena, 1, 1, Side::Bid, 10050, idx1);
        book.add_order(&mut arena, 2, 1, Side::Bid, 10000, idx2);
        book.add_order(&mut arena, 3, 1, Side::Bid, 9950, idx3);
        
        assert_eq!(book.best_bid(), Some(10050));
        
        // Remove best bid
        book.remove_order(&mut arena, 1);
        assert_eq!(book.best_bid(), Some(10000)); // Should recalculate
        
        // Remove next best
        book.remove_order(&mut arena, 2);
        assert_eq!(book.best_bid(), Some(9950));
        
        // Remove last
        book.remove_order(&mut arena, 3);
        assert_eq!(book.best_bid(), None);
    }
    
    #[test]
    fn test_multiple_orders_same_level() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        // Add 3 orders at same price
        let idx1 = create_order(&mut arena, 1, 10000, 100);
        let idx2 = create_order(&mut arena, 2, 10000, 200);
        let idx3 = create_order(&mut arena, 3, 10000, 300);
        
        book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx1);
        book.add_order(&mut arena, 2, 1, Side::Bid, 10000, idx2);
        book.add_order(&mut arena, 3, 1, Side::Bid, 10000, idx3);
        
        assert_eq!(book.order_count(), 3);
        assert_eq!(book.bid_levels(), 1);
        
        let (qty, count) = book.depth_at(Side::Bid, 10000);
        assert_eq!(qty, 600);
        assert_eq!(count, 3);
        
        // Remove middle order
        book.remove_order(&mut arena, 2);
        let (qty, count) = book.depth_at(Side::Bid, 10000);
        assert_eq!(qty, 400);
        assert_eq!(count, 2);
        
        // Level should still exist
        assert_eq!(book.bid_levels(), 1);
        assert_eq!(book.best_bid(), Some(10000));
    }
    
    #[test]
    fn test_depth_at() {
        let mut arena = Arena::new(100);
        let mut book = OrderBook::new();
        
        // Empty level
        assert_eq!(book.depth_at(Side::Bid, 10000), (0, 0));
        
        // Add some orders
        let idx1 = create_order(&mut arena, 1, 10000, 100);
        let idx2 = create_order(&mut arena, 2, 10000, 250);
        
        book.add_order(&mut arena, 1, 1, Side::Bid, 10000, idx1);
        book.add_order(&mut arena, 2, 1, Side::Bid, 10000, idx2);
        
        assert_eq!(book.depth_at(Side::Bid, 10000), (350, 2));
    }
}
