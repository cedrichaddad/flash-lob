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
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use std::thread;
use flash_lob::{Engine, Command, PlaceOrder, Side, OrderType};

// Shared statistics for the Snapshot Pattern
struct SharedStats {
    ops_count: AtomicU64,
    p99_latency_ns: AtomicU64,
    arena_used: AtomicU64,
    arena_capacity: AtomicU64,
    bid_depth: AtomicU64,
    ask_depth: AtomicU64,
    last_trade_price: AtomicU64,
    last_trade_qty: AtomicU64,
}

impl SharedStats {
    fn new(capacity: u64) -> Self {
        Self {
            ops_count: AtomicU64::new(0),
            p99_latency_ns: AtomicU64::new(0),
            arena_used: AtomicU64::new(0),
            arena_capacity: AtomicU64::new(capacity),
            bid_depth: AtomicU64::new(0),
            ask_depth: AtomicU64::new(0),
            last_trade_price: AtomicU64::new(0),
            last_trade_qty: AtomicU64::new(0),
        }
    }
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
        
        loop {
            // Batch processing to reduce atomic contention overhead
            const BATCH_SIZE: u64 = 1000;
            let start_batch = std::time::Instant::now();
            
            for _ in 0..BATCH_SIZE {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                order_id = order_id.wrapping_add(1);
                
                let side = if rng % 2 == 0 { Side::Bid } else { Side::Ask };
                let price = 10000 + (rng % 1000) as u64; // 100.00 - 110.00
                let qty = 1 + (rng % 100) as u32;
                
                // 50% Limit, 50% IOC/Market-like match
                let is_taker = (rng >> 10) % 10 == 0; 
                let final_price = if is_taker {
                    if side == Side::Bid { price + 50 } else { price - 50 }
                } else {
                    price
                };

                let cmd = Command::Place(PlaceOrder {
                    order_id,
                    user_id: 1,
                    side,
                    price: final_price,
                    qty,
                    order_type: OrderType::Limit,
                });
                
                engine.process_command(cmd);
            }
            
            // Update stats
            stats_clone.ops_count.fetch_add(BATCH_SIZE, Ordering::Relaxed);
            
            // Approximate latency per op
            let elapsed = start_batch.elapsed();
            let ns_per_op = elapsed.as_nanos() as u64 / BATCH_SIZE;
            stats_clone.p99_latency_ns.store(ns_per_op, Ordering::Relaxed); // Actually Avg, but good for demo
            
            // Snapshot simple metrics
            stats_clone.arena_used.store(engine.order_count() as u64, Ordering::Relaxed);
            stats_clone.bid_depth.store(engine.matcher.book.bids.len() as u64, Ordering::Relaxed);
            stats_clone.ask_depth.store(engine.matcher.book.asks.len() as u64, Ordering::Relaxed);
            
            // Reset if full
            if engine.order_count() > (capacity as usize) * 9 / 10 {
                engine = Engine::new(capacity); // Hard reset for demo loop
            }
            
            // Yield slightly to let UI thread breathe if single core (unlikely)
            // thread::yield_now(); 
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
            let header = Block::default().borders(Borders::ALL).title("FLASH-LOB Crypto Demo");
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
                
            let bid_depth = stats.bid_depth.load(Ordering::Relaxed);
            let ask_depth = stats.ask_depth.load(Ordering::Relaxed);

            let bids = Paragraph::new(format!("BIDS\n\nActive Levels: {}\n\n(Synthetic Visualization)", bid_depth))
                .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::Green)));
                
            let asks = Paragraph::new(format!("ASKS\n\nActive Levels: {}\n\n(Synthetic Visualization)", ask_depth))
                .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::Red)));

            f.render_widget(bids, book_chunks[0]);
            f.render_widget(asks, book_chunks[1]);

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
