//! Price Level - A FIFO queue of orders at a single price point.
//!
//! Implements a doubly-linked list using arena indices for O(1)
//! insertion, removal from head, and removal from arbitrary position.

use crate::arena::{Arena, ArenaIndex, NULL_INDEX};

/// A queue of orders at a specific price level.
///
/// Orders are processed in FIFO order (price-time priority).
/// The doubly-linked structure enables O(1) cancel from any position.
#[derive(Clone, Copy, Debug, Default)]
pub struct PriceLevel {
    /// Index of the oldest order (highest priority, first to match)
    pub head: ArenaIndex,
    /// Index of the newest order (last to match)
    pub tail: ArenaIndex,
    /// Total quantity across all orders at this level
    pub total_qty: u64,
    /// Number of orders at this level
    pub count: u32,
}

impl PriceLevel {
    /// Create a new empty price level
    #[inline]
    pub const fn new() -> Self {
        Self {
            head: NULL_INDEX,
            tail: NULL_INDEX,
            total_qty: 0,
            count: 0,
        }
    }
    
    /// Returns true if there are no orders at this level
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
    
    /// Append an order to the tail of the queue (newest order).
    ///
    /// # Arguments
    /// * `arena` - The arena containing order nodes
    /// * `index` - Index of the order to add
    ///
    /// # Complexity
    /// O(1)
    #[inline]
    pub fn push_back(&mut self, arena: &mut Arena, index: ArenaIndex) {
        let qty = arena.get(index).qty;
        
        if self.tail == NULL_INDEX {
            // Empty list: new node becomes both head and tail
            debug_assert!(self.head == NULL_INDEX);
            self.head = index;
            self.tail = index;
            arena.get_mut(index).prev = NULL_INDEX;
            arena.get_mut(index).next = NULL_INDEX;
        } else {
            // Append to existing tail
            arena.get_mut(self.tail).next = index;
            arena.get_mut(index).prev = self.tail;
            arena.get_mut(index).next = NULL_INDEX;
            self.tail = index;
        }
        
        self.count += 1;
        self.total_qty += qty as u64;
    }
    
    /// Remove and return the head order (oldest/highest priority).
    ///
    /// # Returns
    /// The index of the removed order, or `None` if empty.
    /// The order is NOT freed from the arena; caller must do that.
    ///
    /// # Complexity
    /// O(1)
    #[inline]
    pub fn pop_front(&mut self, arena: &mut Arena) -> Option<ArenaIndex> {
        if self.head == NULL_INDEX {
            return None;
        }
        
        let index = self.head;
        let node = arena.get(index);
        let next_idx = node.next;
        let qty = node.qty;
        
        if next_idx == NULL_INDEX {
            // Was the only node
            self.head = NULL_INDEX;
            self.tail = NULL_INDEX;
        } else {
            // Update new head
            self.head = next_idx;
            arena.get_mut(next_idx).prev = NULL_INDEX;
        }
        
        self.count -= 1;
        self.total_qty -= qty as u64;
        
        // Clear the removed node's linkage
        arena.get_mut(index).prev = NULL_INDEX;
        arena.get_mut(index).next = NULL_INDEX;
        
        Some(index)
    }
    
    /// Remove an order from anywhere in the queue (for cancel).
    ///
    /// Handles all edge cases:
    /// - Only node in level (head == tail)
    /// - Removing head
    /// - Removing tail
    /// - Removing from middle
    ///
    /// # Arguments
    /// * `arena` - The arena containing order nodes
    /// * `index` - Index of the order to remove
    ///
    /// # Returns
    /// `true` if the level is now empty, `false` otherwise.
    /// The order is NOT freed from the arena; caller must do that.
    ///
    /// # Complexity
    /// O(1)
    #[inline]
    pub fn remove(&mut self, arena: &mut Arena, index: ArenaIndex) -> bool {
        let node = arena.get(index);
        let prev_idx = node.prev;
        let next_idx = node.next;
        let qty = node.qty;
        
        // Case 1: Only node in level (head == tail == index)
        if prev_idx == NULL_INDEX && next_idx == NULL_INDEX {
            debug_assert!(self.head == index && self.tail == index);
            self.head = NULL_INDEX;
            self.tail = NULL_INDEX;
        }
        // Case 2: Removing head (prev is NULL, next exists)
        else if prev_idx == NULL_INDEX {
            debug_assert!(self.head == index);
            self.head = next_idx;
            arena.get_mut(next_idx).prev = NULL_INDEX;
        }
        // Case 3: Removing tail (next is NULL, prev exists)
        else if next_idx == NULL_INDEX {
            debug_assert!(self.tail == index);
            self.tail = prev_idx;
            arena.get_mut(prev_idx).next = NULL_INDEX;
        }
        // Case 4: Removing from middle (both prev and next exist)
        else {
            arena.get_mut(prev_idx).next = next_idx;
            arena.get_mut(next_idx).prev = prev_idx;
        }
        
        self.count -= 1;
        self.total_qty -= qty as u64;
        
        // Clear the removed node's linkage
        arena.get_mut(index).prev = NULL_INDEX;
        arena.get_mut(index).next = NULL_INDEX;
        
        self.count == 0
    }
    
    /// Peek at the head order without removing it.
    ///
    /// # Returns
    /// Index of the head order, or `NULL_INDEX` if empty.
    #[inline]
    pub const fn peek_head(&self) -> ArenaIndex {
        self.head
    }
    
    /// Update total quantity after a partial fill.
    ///
    /// Call this after modifying an order's qty directly.
    #[inline]
    pub fn subtract_qty(&mut self, qty: u32) {
        debug_assert!(self.total_qty >= qty as u64);
        self.total_qty -= qty as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;
    
    fn setup_arena_with_orders(arena: &mut Arena, count: u32) -> Vec<ArenaIndex> {
        let mut indices = Vec::new();
        for i in 0..count {
            let idx = arena.alloc().unwrap();
            let node = arena.get_mut(idx);
            node.order_id = i as u64;
            node.qty = 100;
            node.price = 10000;
            indices.push(idx);
        }
        indices
    }
    
    #[test]
    fn test_empty_level() {
        let level = PriceLevel::new();
        assert!(level.is_empty());
        assert_eq!(level.count, 0);
        assert_eq!(level.total_qty, 0);
        assert_eq!(level.head, NULL_INDEX);
        assert_eq!(level.tail, NULL_INDEX);
    }
    
    #[test]
    fn test_push_single() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        
        let idx = arena.alloc().unwrap();
        arena.get_mut(idx).qty = 100;
        
        level.push_back(&mut arena, idx);
        
        assert!(!level.is_empty());
        assert_eq!(level.count, 1);
        assert_eq!(level.total_qty, 100);
        assert_eq!(level.head, idx);
        assert_eq!(level.tail, idx);
    }
    
    #[test]
    fn test_push_multiple_fifo() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        let indices = setup_arena_with_orders(&mut arena, 3);
        
        for &idx in &indices {
            level.push_back(&mut arena, idx);
        }
        
        assert_eq!(level.count, 3);
        assert_eq!(level.total_qty, 300);
        assert_eq!(level.head, indices[0]);
        assert_eq!(level.tail, indices[2]);
        
        // Verify linkage
        assert_eq!(arena.get(indices[0]).next, indices[1]);
        assert_eq!(arena.get(indices[1]).prev, indices[0]);
        assert_eq!(arena.get(indices[1]).next, indices[2]);
        assert_eq!(arena.get(indices[2]).prev, indices[1]);
    }
    
    #[test]
    fn test_pop_front() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        let indices = setup_arena_with_orders(&mut arena, 3);
        
        for &idx in &indices {
            level.push_back(&mut arena, idx);
        }
        
        // Pop first
        let popped = level.pop_front(&mut arena);
        assert_eq!(popped, Some(indices[0]));
        assert_eq!(level.count, 2);
        assert_eq!(level.head, indices[1]);
        assert_eq!(arena.get(indices[1]).prev, NULL_INDEX);
        
        // Pop second
        let popped = level.pop_front(&mut arena);
        assert_eq!(popped, Some(indices[1]));
        assert_eq!(level.count, 1);
        
        // Pop third (last)
        let popped = level.pop_front(&mut arena);
        assert_eq!(popped, Some(indices[2]));
        assert!(level.is_empty());
        
        // Pop from empty
        assert!(level.pop_front(&mut arena).is_none());
    }
    
    #[test]
    fn test_remove_only_node() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        
        let idx = arena.alloc().unwrap();
        arena.get_mut(idx).qty = 100;
        level.push_back(&mut arena, idx);
        
        let is_empty = level.remove(&mut arena, idx);
        
        assert!(is_empty);
        assert!(level.is_empty());
        assert_eq!(level.head, NULL_INDEX);
        assert_eq!(level.tail, NULL_INDEX);
    }
    
    #[test]
    fn test_remove_head() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        let indices = setup_arena_with_orders(&mut arena, 3);
        
        for &idx in &indices {
            level.push_back(&mut arena, idx);
        }
        
        let is_empty = level.remove(&mut arena, indices[0]);
        
        assert!(!is_empty);
        assert_eq!(level.count, 2);
        assert_eq!(level.head, indices[1]);
        assert_eq!(arena.get(indices[1]).prev, NULL_INDEX);
    }
    
    #[test]
    fn test_remove_tail() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        let indices = setup_arena_with_orders(&mut arena, 3);
        
        for &idx in &indices {
            level.push_back(&mut arena, idx);
        }
        
        let is_empty = level.remove(&mut arena, indices[2]);
        
        assert!(!is_empty);
        assert_eq!(level.count, 2);
        assert_eq!(level.tail, indices[1]);
        assert_eq!(arena.get(indices[1]).next, NULL_INDEX);
    }
    
    #[test]
    fn test_remove_middle() {
        let mut arena = Arena::new(10);
        let mut level = PriceLevel::new();
        let indices = setup_arena_with_orders(&mut arena, 3);
        
        for &idx in &indices {
            level.push_back(&mut arena, idx);
        }
        
        let is_empty = level.remove(&mut arena, indices[1]);
        
        assert!(!is_empty);
        assert_eq!(level.count, 2);
        assert_eq!(arena.get(indices[0]).next, indices[2]);
        assert_eq!(arena.get(indices[2]).prev, indices[0]);
    }
    
    #[test]
    fn test_subtract_qty() {
        let mut level = PriceLevel::new();
        level.total_qty = 500;
        
        level.subtract_qty(100);
        assert_eq!(level.total_qty, 400);
        
        level.subtract_qty(400);
        assert_eq!(level.total_qty, 0);
    }
}
