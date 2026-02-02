//! Command and Event types for the matching engine.
//!
//! Commands are inputs from the network thread.
//! Events are outputs to market data consumers.

/// Order side (bid = buy, ask = sell)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Side {
    /// Buy side (bids)
    Bid = 0,
    /// Sell side (asks)
    Ask = 1,
}

impl Side {
    /// Returns the opposite side
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }
}

// ============================================================================
// Input Commands
// ============================================================================

/// Place a new limit order
#[derive(Clone, Copy, Debug)]
pub struct PlaceOrder {
    /// External order ID (client-assigned)
    pub order_id: u64,
    /// Trader/user ID
    pub user_id: u64,
    /// Order side (bid/ask)
    pub side: Side,
    /// Fixed-point price (e.g., $100.50 -> 10050000)
    pub price: u64,
    /// Order quantity
    pub qty: u32,
}

/// Cancel an existing order
#[derive(Clone, Copy, Debug)]
pub struct CancelOrder {
    /// Order ID to cancel
    pub order_id: u64,
}

/// Modify an existing order (cancel + replace)
#[derive(Clone, Copy, Debug)]
pub struct ModifyOrder {
    /// Original order ID
    pub order_id: u64,
    /// New order ID (optional: can be same as original)
    pub new_order_id: u64,
    /// New price
    pub new_price: u64,
    /// New quantity
    pub new_qty: u32,
}

/// Input commands from the network thread
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Place a new limit order
    Place(PlaceOrder),
    /// Cancel an existing order
    Cancel(CancelOrder),
    /// Modify an existing order
    Modify(ModifyOrder),
}

// ============================================================================
// Output Events
// ============================================================================

/// A trade was executed
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradeEvent {
    /// Execution price
    pub price: u64,
    /// Executed quantity
    pub qty: u32,
    /// Maker (passive) order ID
    pub maker_order_id: u64,
    /// Taker (aggressive) order ID
    pub taker_order_id: u64,
    /// Maker user ID
    pub maker_user_id: u64,
    /// Taker user ID
    pub taker_user_id: u64,
    /// Side of the taker order
    pub taker_side: Side,
}

/// Order book level update (Level 2 market data)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BookUpdate {
    /// Which side changed
    pub side: Side,
    /// Price level that changed
    pub price: u64,
    /// New total quantity at this price (0 = level removed)
    pub new_qty: u64,
    /// New order count at this price
    pub new_count: u32,
}

/// Order was accepted and resting in the book
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderAccepted {
    pub order_id: u64,
    pub price: u64,
    pub qty: u32,
    pub side: Side,
}

/// Order was canceled
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderCanceled {
    pub order_id: u64,
    /// Remaining quantity that was canceled
    pub canceled_qty: u32,
}

/// Order was rejected
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderRejected {
    pub order_id: u64,
    pub reason: RejectReason,
}

/// Reasons for order rejection
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RejectReason {
    /// Order ID already exists
    DuplicateOrderId = 0,
    /// Order not found (for cancel/modify)
    OrderNotFound = 1,
    /// Arena is full
    ArenaFull = 2,
    /// Invalid price
    InvalidPrice = 3,
    /// Invalid quantity
    InvalidQuantity = 4,
}

/// Output events from the matching engine
#[derive(Clone, Copy, Debug)]
pub enum OutputEvent {
    /// Trade executed
    Trade(TradeEvent),
    /// Book level changed
    BookDelta(BookUpdate),
    /// Order accepted and resting
    Accepted(OrderAccepted),
    /// Order canceled
    Canceled(OrderCanceled),
    /// Order rejected
    Rejected(OrderRejected),
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Bid.opposite(), Side::Ask);
        assert_eq!(Side::Ask.opposite(), Side::Bid);
    }
    
    #[test]
    fn test_place_order() {
        let order = PlaceOrder {
            order_id: 1,
            user_id: 100,
            side: Side::Bid,
            price: 10050000,
            qty: 100,
        };
        assert_eq!(order.order_id, 1);
        assert_eq!(order.side, Side::Bid);
    }
    
    #[test]
    fn test_command_variants() {
        let place = Command::Place(PlaceOrder {
            order_id: 1,
            user_id: 1,
            side: Side::Bid,
            price: 100,
            qty: 10,
        });
        
        let cancel = Command::Cancel(CancelOrder { order_id: 1 });
        
        match place {
            Command::Place(o) => assert_eq!(o.order_id, 1),
            _ => panic!("Expected Place"),
        }
        
        match cancel {
            Command::Cancel(c) => assert_eq!(c.order_id, 1),
            _ => panic!("Expected Cancel"),
        }
    }
}
