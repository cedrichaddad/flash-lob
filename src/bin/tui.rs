use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use std::{io, time::Duration};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use flash_lob::{Engine, Command, PlaceOrder, Side, OrderType};

// [NEW] A Snapshot of the top levels to share with the UI
#[derive(Default, Clone)]
struct BookSnapshot {
    bids: Vec<(u64, u32)>, // (Price, Qty)
    asks: Vec<(u64, u32)>,
}

struct SharedStats {
    ops_count: AtomicU64,
    p99_latency_ns: AtomicU64,
    arena_used: AtomicU64,
    arena_capacity: AtomicU64,
    // [NEW] The actual book data (protected by a lock)
    book_snapshot: RwLock<BookSnapshot>,
}

impl SharedStats {
    fn new(capacity: u64) -> Self {
        Self {
            ops_count: AtomicU64::new(0),
            p99_latency_ns: AtomicU64::new(0),
            arena_used: AtomicU64::new(0),
            arena_capacity: AtomicU64::new(capacity),
            // Initialize empty
            book_snapshot: RwLock::new(BookSnapshot::default()),
        }
    }
}

// Helper to generate the ASCII Bar string
fn render_level_bars(levels: &[(u64, u32)], side: Side, _max_width: usize) -> String {
    let mut out = String::new();
    let max_qty = levels.iter().map(|(_, q)| *q).max().unwrap_or(1) as f32;

    for (price, qty) in levels.iter().take(15) { // Show top 15
        let price_fmt = format!("{:.2}", *price as f64 / 100.0); // Assuming $100.00 fixed point
        
        // Calculate bar length (e.g., 20 chars max)
        let bar_len = ((*qty as f32 / max_qty) * 20.0) as usize;
        let bar = "█".repeat(bar_len);
        // let _space = " ".repeat(20 - bar_len);
        
        let line = if side == Side::Bid {
            // Bid: Price | Bar | Qty
            format!("{:>8} {} {:<5}\n", price_fmt, bar, qty)
        } else {
            // Ask: Price | Bar | Qty
            format!("{:>8} {} {:<5}\n", price_fmt, bar, qty)
        };
        out.push_str(&line);
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Shared state
    let capacity = 1_000_000;
    let stats = Arc::new(SharedStats::new(capacity as u64));
    let stats_clone = stats.clone();

    // Spawn Engine Thread (Synthetic Load)
    thread::spawn(move || {
        let mut engine = Engine::new(capacity);
        engine.warm_up();
        
        let mut order_id = 1u64;
        let mut rng = 12345u64; // Simple LCG for speed
        let mut loop_count = 0u64; // Deterministic counter for snapshots
        
        // [NEW] Start at $3,000.00 (Fixed point: 300,000)
        let mut current_mid_price = 300_000u64; 

        loop {
            // Batch processing to reduce atomic contention overhead
            const BATCH_SIZE: u64 = 1000;
            let start_batch = std::time::Instant::now();
            
            for _ in 0..BATCH_SIZE {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                order_id = order_id.wrapping_add(1);
                
                // Use high 32 bits for better randomness (LCG low bits are poor)
                let r = rng >> 32;

                // [NEW] Random Walk Logic (Simulate Volatility)
                // Apply drift to the mid-price (Brownian Motion)
                if r % 100 == 0 { // 1% chance to drift per order for smoother action
                    let drift = (r % 5) as i64 - 2; // -2, -1, 0, 1, 2
                    // Add multiplier to drift to make it more visible? No, small steps are fine.
                    // Let's allow slightly larger steps: -5 to +5
                    let drift_mag = (r % 11) as i64 - 5; 
                    current_mid_price = (current_mid_price as i64 + drift_mag).max(1000) as u64;
                }

                // Determine Side (50/50 bid/ask)
                let side = if r % 2 == 0 { Side::Bid } else { Side::Ask };
                
                // Place orders AROUND the mid-price (Spread generation)
                // Spread can be tight or wide.
                // 100 to 500 spread ($1.00 to $5.00)
                let spread_dist = 100 + (r % 400); 
                let spread_offset = spread_dist / 2;
                
                // Add some noise to specific order price
                let noise = (r % 20) as i64 - 10;
                
                let base_price = if side == Side::Bid {
                   current_mid_price.saturating_sub(spread_offset)
                } else {
                   current_mid_price.saturating_add(spread_offset)
                };
                
                let price = (base_price as i64 + noise).max(1) as u64;

                let qty = 1 + (rng % 100) as u32; // 0.01 to 1.00 ETH size

                let cmd = Command::Place(PlaceOrder {
                    order_id,
                    user_id: 1,
                    side,
                    price,
                    qty,
                    order_type: OrderType::Limit,
                });
                
                engine.process_command(cmd);
            }
            
            loop_count += 1;

            // Update stats
            stats_clone.ops_count.fetch_add(BATCH_SIZE, Ordering::Relaxed);
            
            // Approximate latency per op
            let elapsed = start_batch.elapsed();
            let ns_per_op = elapsed.as_nanos() as u64 / BATCH_SIZE;
            stats_clone.p99_latency_ns.store(ns_per_op, Ordering::Relaxed); // Actually Avg, but good for demo
            stats_clone.arena_used.store(engine.order_count() as u64, Ordering::Relaxed);

            // [NEW] Publish Snapshot (Only once per batch/loop iteration)
            // Use loop_count to guarantee updates every 50 batches (approx 5ms at 10M ops/sec)
            if loop_count % 50 == 0 { 
                if let Ok(mut write_guard) = stats_clone.book_snapshot.write() {
                    // Extract Top 15 Bids/Asks manually
                    write_guard.bids = engine.matcher.book.bids.iter()
                        .rev().take(15).map(|(p, l)| (*p, l.total_qty as u32)).collect();
                    write_guard.asks = engine.matcher.book.asks.iter()
                        .take(15).map(|(p, l)| (*p, l.total_qty as u32)).collect();
                }
            }
            
            // Reset if full
            if engine.order_count() > (capacity as usize) * 9 / 10 {
                engine = Engine::new(capacity); // Hard reset for demo loop
            }
        }
    });

    // Run TUI Loop
    let mut last_ops = 0;
    let mut last_time = std::time::Instant::now();
    let mut throughput = 0.0;

    loop {
        // Handle input
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }

        // Calculate throughput
        let now = std::time::Instant::now();
        if now.duration_since(last_time).as_secs_f64() >= 1.0 {
            let current_ops = stats.ops_count.load(Ordering::Relaxed);
            throughput = (current_ops - last_ops) as f64;
            last_ops = current_ops;
            last_time = now;
        }

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Length(3), // Header
                        Constraint::Min(10),   // Book
                        Constraint::Length(10), // Stats
                    ]
                    .as_ref(),
                )
                .split(f.size());

            // 1. Header
            let header = Block::default().borders(Borders::ALL).title("FLASH-LOB Crypto Demo (Brownian)");
            let title = Paragraph::new("ETH-USD | Press 'q' to quit")
                .block(header)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(title, chunks[0]);

            // 2. Book
            let book_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);
                
            // [NEW] Render the Bars
            let snapshot = stats.book_snapshot.read().unwrap();
            
            let bids_text = render_level_bars(&snapshot.bids, Side::Bid, 30);
            let asks_text = render_level_bars(&snapshot.asks, Side::Ask, 30);

            let bids_widget = Paragraph::new(bids_text)
                .block(Block::default().borders(Borders::ALL).title("BIDS").style(Style::default().fg(Color::Green)));
            
            let asks_widget = Paragraph::new(asks_text)
                .block(Block::default().borders(Borders::ALL).title("ASKS").style(Style::default().fg(Color::Red)));

            f.render_widget(bids_widget, book_chunks[0]);
            f.render_widget(asks_widget, book_chunks[1]);

            // 3. Stats
            let ops_fmt = if throughput > 1_000_000.0 {
                format!("{:.2} M", throughput / 1_000_000.0)
            } else {
                format!("{:.0} k", throughput / 1_000.0)
            };
            
            let arena_used = stats.arena_used.load(Ordering::Relaxed);
            let arena_cap = stats.arena_capacity.load(Ordering::Relaxed);
            let arena_pct = (arena_used as f64 / arena_cap as f64) * 100.0;
            let latency = stats.p99_latency_ns.load(Ordering::Relaxed);

            let stats_text = format!(
                "Throughput: {} ops/sec\nLatency (Avg Batch): {} ns\nArena Usage: {} / {} ({:.1}%)",
                ops_fmt, latency, arena_used, arena_cap, arena_pct
            );

            let stats_block = Paragraph::new(stats_text)
                .block(Block::default().borders(Borders::ALL).title("Engine Telemetry"))
                .style(Style::default().fg(Color::Yellow));
            f.render_widget(stats_block, chunks[2]);
        })?;
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
