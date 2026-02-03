//! Arena Allocator - O(1) slab allocator with cache-line aligned nodes.
//!
//! The arena pre-allocates a contiguous block of memory at startup,
//! eliminating heap allocation in the hot path. Uses a free list for
//! O(1) allocation and deallocation.

use std::fmt;

/// Sentinel value representing a null/invalid index (like nullptr)
pub const NULL_INDEX: u32 = u32::MAX;

/// Type alias for arena indices - our "compressed pointers"
/// Using u32 instead of 64-bit pointers halves metadata size,
/// doubling cache efficiency.
pub type ArenaIndex = u32;

/// A single order in the book - exactly 64 bytes (one cache line).
///
/// # Memory Layout
///
/// | Field      | Type    | Offset | Size |
/// |------------|---------|--------|------|
/// | price      | u64     | 0      | 8    |
/// | qty        | u32     | 8      | 4    |
/// | (padding)  | -       | 12     | 4    |
/// | order_id   | u64     | 16     | 8    |
/// | user_id    | u64     | 24     | 8    |
/// | next       | u32     | 32     | 4    |
/// | prev       | u32     | 36     | 4    |
/// | _reserved  | [u8;24] | 40     | 24   |
/// | **Total**  |         |        | 64   |
///
/// Note: There's 4 bytes of padding after `qty` due to u64 alignment.
#[repr(C)]
#[repr(align(64))]
#[derive(Clone, Copy)]
pub struct OrderNode {
    // === Hot Data (frequently accessed during matching) ===
    
    /// Fixed-point price (e.g., $100.50 -> 10050000 with 5 decimal places)
    pub price: u64,
    
    /// Remaining quantity to fill
    pub qty: u32,
    
    // 4 bytes implicit padding here for u64 alignment
    
    /// External order ID (for client tracking)
    pub order_id: u64,
    
    /// Trader/user ID (for trade attribution)
    pub user_id: u64,
    
    // === Linkage (FIFO queue pointers within a PriceLevel) ===
    
    /// Index of next order at same price level
    pub next: ArenaIndex,
    
    /// Index of previous order (enables O(1) cancel)
    pub prev: ArenaIndex,
    
    // === Reserved Space (28 bytes) ===
    // Future use: timestamp, side enum, flags, etc.
    // Current layout: 8 + 4 + (4 padding) + 8 + 8 + 4 + 4 = 40 bytes
    // Need: 64 - 40 = 24 bytes padding
    pub _reserved: [u8; 24],
}

// Compile-time assertion: OrderNode must be exactly 64 bytes
const _: () = assert!(
    std::mem::size_of::<OrderNode>() == 64,
    "OrderNode must be exactly 64 bytes (one cache line)"
);

// Compile-time assertion: OrderNode must be 64-byte aligned
const _: () = assert!(
    std::mem::align_of::<OrderNode>() == 64,
    "OrderNode must be 64-byte aligned"
);

impl OrderNode {
    /// Create a new order node with the given data
    #[inline]
    pub fn new(order_id: u64, user_id: u64, price: u64, qty: u32) -> Self {
        Self {
            price,
            qty,
            order_id,
            user_id,
            next: NULL_INDEX,
            prev: NULL_INDEX,
            _reserved: [0u8; 24],
        }
    }
    
    /// Create an empty/uninitialized node (for free list)
    #[inline]
    pub const fn empty() -> Self {
        Self {
            price: 0,
            qty: 0,
            order_id: 0,
            user_id: 0,
            next: NULL_INDEX,
            prev: NULL_INDEX,
            _reserved: [0u8; 24],
        }
    }
    
    /// Reset the node for reuse (when returning to free list)
    #[inline]
    pub fn reset(&mut self) {
        self.price = 0;
        self.qty = 0;
        self.order_id = 0;
        self.user_id = 0;
        self.next = NULL_INDEX;
        self.prev = NULL_INDEX;
    }
}

impl fmt::Debug for OrderNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrderNode")
            .field("order_id", &self.order_id)
            .field("user_id", &self.user_id)
            .field("price", &self.price)
            .field("qty", &self.qty)
            .field("prev", &self.prev)
            .field("next", &self.next)
            .finish()
    }
}

/// Pre-allocated memory pool with O(1) allocation and deallocation.
///
/// Uses a free list threaded through the `next` field of unused nodes.
/// No system calls or locks in the hot path.
pub struct Arena {
    /// Contiguous block of pre-allocated nodes
    nodes: Vec<OrderNode>,
    
    /// Head of the free list (index of first available node)
    free_head: ArenaIndex,
    
    /// Number of currently allocated nodes
    allocated_count: u32,
    
    /// Total capacity
    capacity: u32,
}

impl Arena {
    /// Create a new arena with the specified capacity.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of orders the arena can hold
    ///
    /// # Panics
    /// Panics if capacity exceeds u32::MAX - 1 (we reserve MAX for NULL_INDEX)
    pub fn new(capacity: u32) -> Self {
        assert!(capacity < NULL_INDEX, "Capacity must be less than NULL_INDEX");
        
        // Pre-allocate all nodes
        let mut nodes = vec![OrderNode::empty(); capacity as usize];
        
        // Thread the free list through all nodes
        // Each node's `next` points to the following node
        for i in 0..(capacity - 1) {
            nodes[i as usize].next = i + 1;
        }
        // Last node points to NULL
        if capacity > 0 {
            nodes[(capacity - 1) as usize].next = NULL_INDEX;
        }
        
        Self {
            nodes,
            free_head: if capacity > 0 { 0 } else { NULL_INDEX },
            allocated_count: 0,
            capacity,
        }
    }
    
    /// Allocate a node from the arena.
    ///
    /// Returns `None` if the arena is full.
    ///
    /// # Complexity
    /// O(1) - pops from head of free list
    #[inline]
    pub fn alloc(&mut self) -> Option<ArenaIndex> {
        if self.free_head == NULL_INDEX {
            return None;
        }
        
        let index = self.free_head;
        self.free_head = self.nodes[index as usize].next;
        self.allocated_count += 1;
        
        // Reset the node for use
        self.nodes[index as usize].next = NULL_INDEX;
        self.nodes[index as usize].prev = NULL_INDEX;
        
        Some(index)
    }
    
    /// Free a node back to the arena.
    ///
    /// # Safety
    /// The caller must ensure the index was previously allocated and
    /// has not already been freed (no double-free protection).
    ///
    /// # Complexity
    /// O(1) - pushes to head of free list
    #[inline]
    pub fn free(&mut self, index: ArenaIndex) {
        debug_assert!(index < self.capacity, "Index out of bounds");
        debug_assert!(self.allocated_count > 0, "Double free detected");
        
        // Reset and push to free list head
        self.nodes[index as usize].reset();
        self.nodes[index as usize].next = self.free_head;
        self.free_head = index;
        self.allocated_count -= 1;
    }
    
    /// Get an immutable reference to a node.
    ///
    /// # Complexity
    /// O(1) - direct array access
    #[inline]
    pub fn get(&self, index: ArenaIndex) -> &OrderNode {
        debug_assert!(index < self.capacity, "Index out of bounds");
        &self.nodes[index as usize]
    }
    
    /// Get a mutable reference to a node.
    ///
    /// # Complexity
    /// O(1) - direct array access
    #[inline]
    pub fn get_mut(&mut self, index: ArenaIndex) -> &mut OrderNode {
        debug_assert!(index < self.capacity, "Index out of bounds");
        &mut self.nodes[index as usize]
    }
    
    /// Returns the number of currently allocated nodes.
    #[inline]
    pub fn allocated(&self) -> u32 {
        self.allocated_count
    }
    
    /// Returns the total capacity of the arena.
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }
    
    /// Returns true if the arena is empty (no allocated nodes).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.allocated_count == 0
    }
    
    /// Returns true if the arena is full (no free nodes).
    #[inline]
    pub fn is_full(&self) -> bool {
        self.free_head == NULL_INDEX
    }
    
    /// Pre-fault all memory pages (warm-up routine).
    ///
    /// Walks through all nodes to force the OS to map virtual pages
    /// to physical RAM, preventing page faults in the hot path.
    pub fn warm_up(&mut self) {
        // Touch every node to fault in pages
        for node in &mut self.nodes {
            // Volatile write to prevent optimization
            unsafe {
                std::ptr::write_volatile(&mut node._reserved[0], 0);
            }
        }
    }
}

impl fmt::Debug for Arena {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arena")
            .field("capacity", &self.capacity)
            .field("allocated", &self.allocated_count)
            .field("free_head", &self.free_head)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_order_node_size() {
        assert_eq!(std::mem::size_of::<OrderNode>(), 64);
        assert_eq!(std::mem::align_of::<OrderNode>(), 64);
    }
    
    #[test]
    fn test_arena_creation() {
        let arena = Arena::new(100);
        assert_eq!(arena.capacity(), 100);
        assert_eq!(arena.allocated(), 0);
        assert!(!arena.is_full());
        assert!(arena.is_empty());
    }
    
    #[test]
    fn test_arena_alloc_free() {
        let mut arena = Arena::new(3);
        
        // Allocate all nodes
        let idx0 = arena.alloc().expect("Should allocate");
        let idx1 = arena.alloc().expect("Should allocate");
        let idx2 = arena.alloc().expect("Should allocate");
        
        assert_eq!(arena.allocated(), 3);
        assert!(arena.is_full());
        assert!(arena.alloc().is_none(), "Should be full");
        
        // Free one
        arena.free(idx1);
        assert_eq!(arena.allocated(), 2);
        assert!(!arena.is_full());
        
        // Allocate again (should reuse idx1's slot)
        let idx3 = arena.alloc().expect("Should allocate");
        assert_eq!(idx3, idx1, "Should reuse freed slot");
        
        // Free all
        arena.free(idx0);
        arena.free(idx2);
        arena.free(idx3);
        assert!(arena.is_empty());
    }
    
    #[test]
    fn test_arena_get_set() {
        let mut arena = Arena::new(10);
        let idx = arena.alloc().unwrap();
        
        // Populate the node
        let node = arena.get_mut(idx);
        node.order_id = 12345;
        node.user_id = 999;
        node.price = 10050000; // $100.50
        node.qty = 100;
        
        // Read back
        let node = arena.get(idx);
        assert_eq!(node.order_id, 12345);
        assert_eq!(node.user_id, 999);
        assert_eq!(node.price, 10050000);
        assert_eq!(node.qty, 100);
    }
    
    #[test]
    fn test_order_node_new() {
        let node = OrderNode::new(123, 456, 10000000, 50);
        assert_eq!(node.order_id, 123);
        assert_eq!(node.user_id, 456);
        assert_eq!(node.price, 10000000);
        assert_eq!(node.qty, 50);
        assert_eq!(node.next, NULL_INDEX);
        assert_eq!(node.prev, NULL_INDEX);
    }
    
    #[test]
    fn test_arena_warm_up() {
        let mut arena = Arena::new(1000);
        arena.warm_up(); // Should not panic
    }
}
