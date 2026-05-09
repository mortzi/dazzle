use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use dazzle::{
    deribit::models::{BookLevel, BookUpdateType, OrderBookUpdate},
    order_book::book::{Book, Price, Quantity, Side},
};

fn make_snapshot(depth: usize) -> OrderBookUpdate {
    OrderBookUpdate {
        instrument_name: "BTC-PERPETUAL".into(),
        timestamp: 0,
        change_id: 1,
        prev_change_id: None,
        update_type: BookUpdateType::Snapshot,
        asks: (0..depth)
            .map(|i| BookLevel { action: "new".into(), price: 80000.0 + i as f64 * 0.5, size: 1.0 })
            .collect(),
        bids: (0..depth)
            .map(|i| BookLevel { action: "new".into(), price: 79999.5 - i as f64 * 0.5, size: 1.0 })
            .collect(),
    }
}

fn bench_price_from_f64(c: &mut Criterion) {
    c.bench_function("price_from_f64", |b| {
        b.iter(|| Price::from_f64(black_box(80258.5)))
    });
}

fn bench_from_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("from_snapshot");
    for depth in [25, 100, 500] {
        let snap = make_snapshot(depth);
        group.bench_with_input(BenchmarkId::from_parameter(depth), &snap, |b, s| {
            b.iter(|| Book::from_snapshot(black_box(s)))
        });
    }
    group.finish();
}

fn bench_walk_book(c: &mut Criterion) {
    let mut group = c.benchmark_group("walk_book");
    for depth in [25, 100, 500] {
        let book = Book::from_snapshot(&make_snapshot(depth));
        let qty = Quantity::from_f64(depth as f64 / 2.0);
        group.bench_with_input(BenchmarkId::from_parameter(depth), &(book, qty), |b, (bk, q)| {
            b.iter(|| bk.walk_book(black_box(Side::Buy), black_box(*q)))
        });
    }
    group.finish();
}

fn bench_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("book_serialize");
    for depth in [25, 100, 500] {
        let book = Book::from_snapshot(&make_snapshot(depth));
        group.bench_with_input(BenchmarkId::from_parameter(depth), &book, |b, bk| {
            b.iter(|| serde_json::to_string(black_box(bk)))
        });
    }
    group.finish();
}

fn bench_update_mix(c: &mut Criterion) {
    // Benchmarks a realistic stream of book updates: 80% insert/change, 20% delete.
    // Uses 20 cycling price levels so the book stays populated despite deletes.
    let mut group = c.benchmark_group("update_mix");
    for depth in [25, 100, 500] {
        let levels: Vec<Price> = (0..20)
            .map(|i| Price::from_f64(80000.0 + i as f64 * 0.5))
            .collect();
        let qty = Quantity::from_f64(2.5);
        let mut book = Book::from_snapshot(&make_snapshot(depth));
        let mut i = 0usize;
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, _| {
            b.iter(|| {
                let price = levels[i % levels.len()];
                if i % 5 == 0 {
                    book.asks.remove(black_box(&price));
                } else {
                    book.asks.insert(black_box(price), black_box(qty));
                }
                i += 1;
            })
        });
    }
    group.finish();
}

fn bench_analytics(c: &mut Criterion) {
    let book = Book::from_snapshot(&make_snapshot(100));
    let mut group = c.benchmark_group("analytics");
    group.bench_function("best_bid", |b| b.iter(|| book.best_bid()));
    group.bench_function("best_ask", |b| b.iter(|| book.best_ask()));
    group.bench_function("spread",   |b| b.iter(|| book.spread()));
    group.bench_function("mid_price", |b| b.iter(|| book.mid_price()));
    group.finish();
}

criterion_group!(benches, bench_price_from_f64, bench_from_snapshot, bench_update_mix, bench_walk_book, bench_serialize, bench_analytics);
criterion_main!(benches);
