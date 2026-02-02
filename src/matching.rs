//! Matching Engine - Core order matching algorithm.
//!
//! Implements the cross/rest algorithm:
//! 1. CROSSING: Match aggressive orders against the opposite side
//! 2. RESTING: Place remaining quantity in the book

use crate::arena::{Arena, ArenaIndex, NULL_INDEX};
use crate::command::{
    BookUpdate, CancelOrder, OutputEvent, PlaceOrder, Side, TradeEvent,
    OrderAccepted, OrderCanceled, OrderRejected, RejectReason,
};
use crate::order_book::OrderBook;

/// Result of processing a place order command
#[derive(Debug)]
pub struct PlaceResult {
    /// Trade events generated (if any matching occurred)
    pub trades: Vec<TradeEvent>,
    /// Book updates generated
    pub book_updates: Vec<BookUpdate>,
    /// Whether the order is resting in the book
    pub is_resting: bool,
    /// Remaining quantity (if resting)
    pub resting_qty: u32,
}

/// The matching engine core
pub struct MatchingEngine {
    /// Memory arena for order nodes
    pub arena: Arena,
    /// The limit order book
    pub book: OrderBook,
}

impl MatchingEngine {
    /// Create a new matching engine with the specified capacity
    pub fn new(capacity: u32) -> Self {
        Self {
            arena: Arena::new(capacity),
            book: OrderBook::with_capacity(1000, capacity as usize),
        }
    }
    
    /// Process a place order command.
    ///
    /// # Algorithm
    /// 1. Check for duplicate order ID
    /// 2. Attempt to cross (match) against opposite side
    /// 3. If quantity remains, rest the order in the book
    ///
    /// # Returns
    /// Vector of output events (trades, book updates, etc.)
    pub fn process_place(&mut self, order: PlaceOrder) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        
        // Validate
        if order.qty == 0 {
            events.push(OutputEvent::Rejected(OrderRejected {
                order_id: order.order_id,
                reason: RejectReason::InvalidQuantity,
            }));
            return events;
        }
        
        // Check for duplicate order ID
        if self.book.contains_order(order.order_id) {
            events.push(OutputEvent::Rejected(OrderRejected {
                order_id: order.order_id,
                reason: RejectReason::DuplicateOrderId,
            }));
            return events;
        }
        
        let mut remaining_qty = order.qty;
        
        // Phase 1: CROSSING (aggressive matching)
        remaining_qty = self.cross_order(&order, remaining_qty, &mut events);
        
        // Phase 2: RESTING (passive posting)
        if remaining_qty > 0 {
            if let Some(_arena_idx) = self.rest_order(&order, remaining_qty, &mut events) {
                // Order is now resting
            } else {
                // Arena is full
                events.push(OutputEvent::Rejected(OrderRejected {
                    order_id: order.order_id,
                    reason: RejectReason::ArenaFull,
                }));
            }
        }
        
        events
    }
    
    /// Cross (match) an incoming order against the opposite side.
    ///
    /// # Returns
    /// Remaining quantity after matching
    fn cross_order(
        &mut self,
        order: &PlaceOrder,
        mut remaining_qty: u32,
        events: &mut Vec<OutputEvent>,
    ) -> u32 {
        let opposite_side = order.side.opposite();
        
        loop {
            if remaining_qty == 0 {
                break;
            }
            
            // Get best opposite price
            let best_opposite = match self.book.best_opposite_price(order.side) {
                Some(price) => price,
                None => break, // No orders on opposite side
            };
            
            // Check if price crosses
            if !self.prices_cross(order.price, best_opposite, order.side) {
                break;
            }
            
            // Match against orders at this level
            remaining_qty = self.match_at_level(
                order,
                best_opposite,
                opposite_side,
                remaining_qty,
                events,
            );
        }
        
        remaining_qty
    }
    
    /// Check if an incoming order price crosses the opposite best price.
    #[inline]
    fn prices_cross(&self, order_price: u64, opposite_best: u64, order_side: Side) -> bool {
        match order_side {
            // Buyer willing to pay >= lowest ask
            Side::Bid => order_price >= opposite_best,
            // Seller willing to accept <= highest bid
            Side::Ask => order_price <= opposite_best,
        }
    }
    
    /// Match against all orders at a specific price level.
    ///
    /// # Returns
    /// Remaining quantity after matching at this level
    fn match_at_level(
        &mut self,
        taker: &PlaceOrder,
        price: u64,
        maker_side: Side,
        mut remaining_qty: u32,
        events: &mut Vec<OutputEvent>,
    ) -> u32 {
        loop {
            if remaining_qty == 0 {
                break;
            }
            
            // Get the price level
            let level = match self.book.get_level_mut(maker_side, price) {
                Some(l) => l,
                None => break,
            };
            
            if level.is_empty() {
                break;
            }
            
            // Get head order (oldest = highest priority)
            let maker_idx = level.peek_head();
            if maker_idx == NULL_INDEX {
                break;
            }
            
            // Get maker order details
            let maker = self.arena.get(maker_idx);
            let maker_order_id = maker.order_id;
            let maker_user_id = maker.user_id;
            let maker_qty = maker.qty;
            
            // Calculate trade quantity
            let trade_qty = remaining_qty.min(maker_qty);
            
            // Emit trade event
            events.push(OutputEvent::Trade(TradeEvent {
                price,
                qty: trade_qty,
                maker_order_id,
                taker_order_id: taker.order_id,
                maker_user_id,
                taker_user_id: taker.user_id,
                taker_side: taker.side,
            }));
            
            // Update quantities
            remaining_qty -= trade_qty;
            let new_maker_qty = maker_qty - trade_qty;
            
            if new_maker_qty == 0 {
                // Maker fully filled - remove from book
                // Re-borrow level mutably
                let level = self.book.get_level_mut(maker_side, price).unwrap();
                level.pop_front(&mut self.arena);
                self.book.remove_order_from_map(maker_order_id);
                self.arena.free(maker_idx);
                
                // Check if level is now empty
                let level = self.book.get_level(maker_side, price);
                if level.map_or(true, |l| l.is_empty()) {
                    // Emit book update (level removed)
                    events.push(OutputEvent::BookDelta(BookUpdate {
                        side: maker_side,
                        price,
                        new_qty: 0,
                        new_count: 0,
                    }));
                    self.book.remove_empty_level(maker_side, price);
                } else {
                    // Emit book update (level updated)
                    let level = self.book.get_level(maker_side, price).unwrap();
                    events.push(OutputEvent::BookDelta(BookUpdate {
                        side: maker_side,
                        price,
                        new_qty: level.total_qty,
                        new_count: level.count,
                    }));
                }
            } else {
                // Maker partially filled - update quantity
                self.arena.get_mut(maker_idx).qty = new_maker_qty;
                
                // Update level total
                let level = self.book.get_level_mut(maker_side, price).unwrap();
                level.subtract_qty(trade_qty);
                
                // Emit book update
                events.push(OutputEvent::BookDelta(BookUpdate {
                    side: maker_side,
                    price,
                    new_qty: level.total_qty,
                    new_count: level.count,
                }));
            }
        }
        
        remaining_qty
    }
    
    /// Rest an order in the book (passive posting).
    ///
    /// # Returns
    /// Arena index of the new order, or `None` if arena is full
    fn rest_order(
        &mut self,
        order: &PlaceOrder,
        qty: u32,
        events: &mut Vec<OutputEvent>,
    ) -> Option<ArenaIndex> {
        // Allocate node
        let arena_idx = self.arena.alloc()?;
        
        // Populate node
        let node = self.arena.get_mut(arena_idx);
        node.order_id = order.order_id;
        node.user_id = order.user_id;
        node.price = order.price;
        node.qty = qty;
        
        // Add to book
        self.book.add_order(
            &mut self.arena,
            order.order_id,
            order.side,
            order.price,
            arena_idx,
        );
        
        // Emit accepted event
        events.push(OutputEvent::Accepted(OrderAccepted {
            order_id: order.order_id,
            price: order.price,
            qty,
            side: order.side,
        }));
        
        // Emit book update
        let level = self.book.get_level(order.side, order.price).unwrap();
        events.push(OutputEvent::BookDelta(BookUpdate {
            side: order.side,
            price: order.price,
            new_qty: level.total_qty,
            new_count: level.count,
        }));
        
        Some(arena_idx)
    }
    
    /// Process a cancel order command.
    ///
    /// # Returns
    /// Vector of output events
    pub fn process_cancel(&mut self, cancel: CancelOrder) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        
        // Look up order
        let info = match self.book.get_order(cancel.order_id) {
            Some(info) => *info,
            None => {
                events.push(OutputEvent::Rejected(OrderRejected {
                    order_id: cancel.order_id,
                    reason: RejectReason::OrderNotFound,
                }));
                return events;
            }
        };
        
        // Get canceled quantity before removal
        let canceled_qty = self.arena.get(info.arena_index).qty;
        
        // Remove from book
        self.book.remove_order(&mut self.arena, cancel.order_id);
        
        // Free arena slot
        self.arena.free(info.arena_index);
        
        // Emit canceled event
        events.push(OutputEvent::Canceled(OrderCanceled {
            order_id: cancel.order_id,
            canceled_qty,
        }));
        
        // Emit book update
        let (new_qty, new_count) = self.book.depth_at(info.side, info.price);
        events.push(OutputEvent::BookDelta(BookUpdate {
            side: info.side,
            price: info.price,
            new_qty,
            new_count,
        }));
        
        events
    }
    
    // ========================================================================
    // Utility Methods
    // ========================================================================
    
    /// Get the best bid price
    #[inline]
    pub fn best_bid(&self) -> Option<u64> {
        self.book.best_bid()
    }
    
    /// Get the best ask price
    #[inline]
    pub fn best_ask(&self) -> Option<u64> {
        self.book.best_ask()
    }
    
    /// Get the spread
    #[inline]
    pub fn spread(&self) -> Option<u64> {
        self.book.spread()
    }
    
    /// Get total order count
    #[inline]
    pub fn order_count(&self) -> usize {
        self.book.order_count()
    }
    
    /// Warm up the engine (pre-fault memory pages)
    pub fn warm_up(&mut self) {
        self.arena.warm_up();
    }
    
    /// Compute a hash of the current state (for determinism testing)
    pub fn state_hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        
        // Hash best prices
        self.book.best_bid().hash(&mut hasher);
        self.book.best_ask().hash(&mut hasher);
        
        // Hash order count and arena state
        self.book.order_count().hash(&mut hasher);
        self.arena.allocated().hash(&mut hasher);
        
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn place_order(
        order_id: u64,
        user_id: u64,
        side: Side,
        price: u64,
        qty: u32,
    ) -> PlaceOrder {
        PlaceOrder {
            order_id,
            user_id,
            side,
            price,
            qty,
        }
    }
    
    #[test]
    fn test_place_bid_no_match() {
        let mut engine = MatchingEngine::new(1000);
        
        let order = place_order(1, 100, Side::Bid, 10000, 100);
        let events = engine.process_place(order);
        
        // Should get Accepted + BookDelta
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], OutputEvent::Accepted(_)));
        assert!(matches!(events[1], OutputEvent::BookDelta(_)));
        
        assert_eq!(engine.best_bid(), Some(10000));
        assert_eq!(engine.best_ask(), None);
        assert_eq!(engine.order_count(), 1);
    }
    
    #[test]
    fn test_place_ask_no_match() {
        let mut engine = MatchingEngine::new(1000);
        
        let order = place_order(1, 100, Side::Ask, 10100, 100);
        let events = engine.process_place(order);
        
        assert_eq!(events.len(), 2);
        assert_eq!(engine.best_bid(), None);
        assert_eq!(engine.best_ask(), Some(10100));
    }
    
    #[test]
    fn test_full_match() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place resting ask
        let ask = place_order(1, 100, Side::Ask, 10000, 100);
        engine.process_place(ask);
        
        // Place crossing bid
        let bid = place_order(2, 200, Side::Bid, 10000, 100);
        let events = engine.process_place(bid);
        
        // Should get Trade + BookDelta (level removed)
        let trades: Vec<_> = events.iter()
            .filter(|e| matches!(e, OutputEvent::Trade(_)))
            .collect();
        assert_eq!(trades.len(), 1);
        
        if let OutputEvent::Trade(t) = trades[0] {
            assert_eq!(t.price, 10000);
            assert_eq!(t.qty, 100);
            assert_eq!(t.maker_order_id, 1);
            assert_eq!(t.taker_order_id, 2);
            assert_eq!(t.taker_side, Side::Bid);
        }
        
        // Book should be empty
        assert_eq!(engine.order_count(), 0);
        assert_eq!(engine.best_bid(), None);
        assert_eq!(engine.best_ask(), None);
    }
    
    #[test]
    fn test_partial_match_taker_remains() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place small resting ask
        let ask = place_order(1, 100, Side::Ask, 10000, 50);
        engine.process_place(ask);
        
        // Place larger crossing bid
        let bid = place_order(2, 200, Side::Bid, 10000, 100);
        let events = engine.process_place(bid);
        
        // Should trade 50, then rest 50
        let trades: Vec<_> = events.iter()
            .filter(|e| matches!(e, OutputEvent::Trade(_)))
            .collect();
        assert_eq!(trades.len(), 1);
        
        if let OutputEvent::Trade(t) = trades[0] {
            assert_eq!(t.qty, 50);
        }
        
        // Taker should be resting
        let accepted: Vec<_> = events.iter()
            .filter(|e| matches!(e, OutputEvent::Accepted(_)))
            .collect();
        assert_eq!(accepted.len(), 1);
        
        if let OutputEvent::Accepted(a) = accepted[0] {
            assert_eq!(a.order_id, 2);
            assert_eq!(a.qty, 50);
        }
        
        // Book state
        assert_eq!(engine.order_count(), 1);
        assert_eq!(engine.best_bid(), Some(10000));
        assert_eq!(engine.best_ask(), None);
    }
    
    #[test]
    fn test_partial_match_maker_remains() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place large resting ask
        let ask = place_order(1, 100, Side::Ask, 10000, 100);
        engine.process_place(ask);
        
        // Place smaller crossing bid
        let bid = place_order(2, 200, Side::Bid, 10000, 30);
        engine.process_place(bid);
        
        // Maker should have 70 remaining
        assert_eq!(engine.order_count(), 1);
        assert_eq!(engine.best_ask(), Some(10000));
        
        let (qty, count) = engine.book.depth_at(Side::Ask, 10000);
        assert_eq!(qty, 70);
        assert_eq!(count, 1);
    }
    
    #[test]
    fn test_match_multiple_levels() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place asks at multiple levels
        engine.process_place(place_order(1, 100, Side::Ask, 10000, 50));
        engine.process_place(place_order(2, 100, Side::Ask, 10010, 50));
        engine.process_place(place_order(3, 100, Side::Ask, 10020, 50));
        
        // Place large crossing bid
        let bid = place_order(4, 200, Side::Bid, 10020, 120);
        let events = engine.process_place(bid);
        
        // Should match all of level 10000 (50), all of 10010 (50), part of 10020 (20)
        let trades: Vec<_> = events.iter()
            .filter_map(|e| if let OutputEvent::Trade(t) = e { Some(t) } else { None })
            .collect();
        
        assert_eq!(trades.len(), 3);
        assert_eq!(trades[0].price, 10000);
        assert_eq!(trades[0].qty, 50);
        assert_eq!(trades[1].price, 10010);
        assert_eq!(trades[1].qty, 50);
        assert_eq!(trades[2].price, 10020);
        assert_eq!(trades[2].qty, 20);
        
        // 30 remaining at 10020
        assert_eq!(engine.order_count(), 1);
        assert_eq!(engine.best_ask(), Some(10020));
    }
    
    #[test]
    fn test_cancel_order() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place order
        engine.process_place(place_order(1, 100, Side::Bid, 10000, 100));
        assert_eq!(engine.order_count(), 1);
        
        // Cancel it
        let events = engine.process_cancel(CancelOrder { order_id: 1 });
        
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], OutputEvent::Canceled(_)));
        assert!(matches!(events[1], OutputEvent::BookDelta(_)));
        
        if let OutputEvent::Canceled(c) = &events[0] {
            assert_eq!(c.order_id, 1);
            assert_eq!(c.canceled_qty, 100);
        }
        
        assert_eq!(engine.order_count(), 0);
        assert_eq!(engine.best_bid(), None);
    }
    
    #[test]
    fn test_cancel_nonexistent() {
        let mut engine = MatchingEngine::new(1000);
        
        let events = engine.process_cancel(CancelOrder { order_id: 999 });
        
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            OutputEvent::Rejected(OrderRejected {
                reason: RejectReason::OrderNotFound,
                ..
            })
        ));
    }
    
    #[test]
    fn test_duplicate_order_id() {
        let mut engine = MatchingEngine::new(1000);
        
        engine.process_place(place_order(1, 100, Side::Bid, 10000, 100));
        let events = engine.process_place(place_order(1, 200, Side::Ask, 10100, 50));
        
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            OutputEvent::Rejected(OrderRejected {
                reason: RejectReason::DuplicateOrderId,
                ..
            })
        ));
    }
    
    #[test]
    fn test_zero_quantity_rejected() {
        let mut engine = MatchingEngine::new(1000);
        
        let events = engine.process_place(place_order(1, 100, Side::Bid, 10000, 0));
        
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            OutputEvent::Rejected(OrderRejected {
                reason: RejectReason::InvalidQuantity,
                ..
            })
        ));
    }
    
    #[test]
    fn test_fifo_order_priority() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place 3 asks at same price (FIFO order: 1, 2, 3)
        engine.process_place(place_order(1, 100, Side::Ask, 10000, 100));
        engine.process_place(place_order(2, 101, Side::Ask, 10000, 100));
        engine.process_place(place_order(3, 102, Side::Ask, 10000, 100));
        
        // Match against first two
        let events = engine.process_place(place_order(4, 200, Side::Bid, 10000, 200));
        
        let trades: Vec<_> = events.iter()
            .filter_map(|e| if let OutputEvent::Trade(t) = e { Some(t) } else { None })
            .collect();
        
        assert_eq!(trades.len(), 2);
        assert_eq!(trades[0].maker_order_id, 1); // First in
        assert_eq!(trades[1].maker_order_id, 2); // Second in
        
        // Order 3 should still be resting
        assert_eq!(engine.order_count(), 1);
    }
    
    #[test]
    fn test_price_time_priority() {
        let mut engine = MatchingEngine::new(1000);
        
        // Place asks at different prices
        engine.process_place(place_order(1, 100, Side::Ask, 10020, 100)); // Worst
        engine.process_place(place_order(2, 100, Side::Ask, 10000, 100)); // Best
        engine.process_place(place_order(3, 100, Side::Ask, 10010, 100)); // Middle
        
        // Match - should go 10000 -> 10010 -> 10020
        let events = engine.process_place(place_order(4, 200, Side::Bid, 10020, 250));
        
        let trades: Vec<_> = events.iter()
            .filter_map(|e| if let OutputEvent::Trade(t) = e { Some(t) } else { None })
            .collect();
        
        assert_eq!(trades.len(), 3);
        assert_eq!(trades[0].price, 10000);
        assert_eq!(trades[1].price, 10010);
        assert_eq!(trades[2].price, 10020);
    }
}
