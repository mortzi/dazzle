use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use dazzle::{
    deribit::models::{BookLevel, BookUpdateType, OrderBookUpdate},
    order_book::book::{Book, Price, Quantity, Side},
};
use hdrhistogram::Histogram;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() {
    scenario_update_throughput().await;
    scenario_read_under_contention().await;
    scenario_walk_scaling().await;
}

fn make_snapshot(depth: usize) -> OrderBookUpdate {
    OrderBookUpdate {
        instrument_name: "BTC-PERPETUAL".into(),
        timestamp: 0,
        change_id: 1,
        prev_change_id: None,
        update_type: BookUpdateType::Snapshot,
        asks: (0..depth)
            .map(|i| BookLevel {
                action: "new".into(),
                price: 80000.0 + i as f64 * 0.5,
                size: 1.0,
            })
            .collect(),
        bids: (0..depth)
            .map(|i| BookLevel {
                action: "new".into(),
                price: 79999.5 - i as f64 * 0.5,
                size: 1.0,
            })
            .collect(),
    }
}

fn new_hist() -> Histogram<u64> {
    // track up to 10 seconds in nanoseconds, 3 significant figures
    Histogram::<u64>::new_with_max(10_000_000_000, 3).unwrap()
}

fn print_hist(label: &str, hist: &Histogram<u64>) {
    println!(
        "  {:<12}  p50={:>7}ns  p95={:>7}ns  p99={:>7}ns  p999={:>8}ns  max={:>8}ns  samples={}",
        label,
        hist.value_at_quantile(0.50),
        hist.value_at_quantile(0.95),
        hist.value_at_quantile(0.99),
        hist.value_at_quantile(0.999),
        hist.max(),
        hist.len(),
    );
}

// ─── Scenario 1: single-threaded update throughput at different book depths ───

async fn scenario_update_throughput() {
    println!("=== Scenario 1: Update throughput at different book depths (100k updates each) ===");

    for depth in [25, 100, 500] {
        let book = Arc::new(RwLock::new(Book::from_snapshot(&make_snapshot(depth))));
        let mut hist = new_hist();

        // Realistic mix: 40% new/change, 40% change, 20% delete
        // Use 20 levels so deletes don't immediately empty the book
        let levels: Vec<Price> = (0..20)
            .map(|i| Price::from_f64(80000.0 + i as f64 * 0.5))
            .collect();
        let qty = Quantity::from_f64(2.5);

        let n = 100_000usize;
        let wall_start = Instant::now();

        for i in 0..n {
            let price = levels[i % levels.len()];
            let t = Instant::now();
            {
                let mut b = book.write().await;
                if i % 5 == 0 {
                    b.asks.remove(&price); // 20% deletes
                } else {
                    b.asks.insert(price, qty); // 80% new/change
                }
                b.change_id += 1;
            }
            hist.record(t.elapsed().as_nanos() as u64).unwrap_or(());
        }

        let throughput = n as f64 / wall_start.elapsed().as_secs_f64();
        println!("depth={depth:>3}  throughput={throughput:>9.0} updates/s");
        print_hist("write_lock", &hist);
    }
}

// ─── Scenario 2: read latency under write contention ─────────────────────────

async fn scenario_read_under_contention() {
    println!(
        "\n=== Scenario 2: Read latency under write contention (depth=100, 10 readers, 2s) ==="
    );

    let book = Arc::new(RwLock::new(Book::from_snapshot(&make_snapshot(100))));
    let stop = Arc::new(AtomicBool::new(false));
    let levels: Vec<Price> = (0..20)
        .map(|i| Price::from_f64(80000.0 + i as f64 * 0.5))
        .collect();
    let qty = Quantity::from_f64(2.5);

    // One writer applying a realistic mix (80% insert/change, 20% delete)
    let book_w = Arc::clone(&book);
    let stop_w = Arc::clone(&stop);
    let writer = tokio::spawn(async move {
        let mut i = 0usize;
        while !stop_w.load(Ordering::Relaxed) {
            let price = levels[i % levels.len()];
            {
                let mut b = book_w.write().await;
                if i % 5 == 0 {
                    b.asks.remove(&price);
                } else {
                    b.asks.insert(price, qty);
                }
                b.change_id += 1;
            }
            i += 1;
            tokio::task::yield_now().await;
        }
        i
    });

    // Ten concurrent readers cloning the book on each read (same as get_book endpoint)
    let mut reader_handles = vec![];
    for _ in 0..10 {
        let book_r = Arc::clone(&book);
        let stop_r = Arc::clone(&stop);
        reader_handles.push(tokio::spawn(async move {
            let mut hist = new_hist();
            while !stop_r.load(Ordering::Relaxed) {
                let t = Instant::now();
                let _snapshot = book_r.read().await.clone();
                hist.record(t.elapsed().as_nanos() as u64).unwrap_or(());
                tokio::task::yield_now().await;
            }
            hist
        }));
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    stop.store(true, Ordering::Relaxed);

    let write_count = writer.await.unwrap();
    println!("  writer applied {write_count} updates");

    let mut merged = new_hist();
    for h in reader_handles {
        merged.add(&h.await.unwrap()).unwrap();
    }
    print_hist("read+clone", &merged);
}

// ─── Scenario 3: walk_book scaling with book depth ───────────────────────────

async fn scenario_walk_scaling() {
    println!("\n=== Scenario 3: walk_book latency scaling with book depth (50k calls each) ===");

    for depth in [25, 100, 500] {
        let book = Book::from_snapshot(&make_snapshot(depth));
        // Buy half the book depth — this exercises the most levels
        let qty = Quantity::from_f64(depth as f64 / 2.0);
        let mut hist = new_hist();

        for _ in 0..50_000 {
            let t = Instant::now();
            let _ = book.walk_book(Side::Buy, qty);
            hist.record(t.elapsed().as_nanos().max(1) as u64)
                .unwrap_or(());
        }

        println!("depth={depth:>3}");
        print_hist("walk_book", &hist);
    }
}
