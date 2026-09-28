use cosmic_text::{Attrs, Family, FontSystem, Metrics};
use std::time::Instant;
use twitty::color::{Palette, Rgba};
use twitty::shaping::{hash_cells, shape_row};
use twitty::terminal::Terminal;

#[test]
fn test_pty_ingestion_throughput_mb_per_sec() {
    let mut term = Terminal::new(120, 40, |_| {});

    // Generate 5MB of realistic mixed terminal stream (ANSI color codes, English, Arabic, numbers)
    let mut stream = Vec::with_capacity(5 * 1024 * 1024);
    let sample_lines = [
        "\x1b[32m[INFO]\x1b[0m Starting build sequence on branch v2-native\r\n",
        "\x1b[34m[CARGO]\x1b[0m Compiling twitty v2.0.0 (/home/omar/projects/twitty)\r\n",
        "\x1b[33m[BIDI]\x1b[0m معالجة النصوص ثنائية الاتجاه وتشكيل الخطوط بسلاسة وسرعة فائقة\r\n",
        "OpenCode Zen 1.18.30 · Context: 44,865 tokens (4%) · $0.00 spent · Build: Muse Spark 1.3 Free\r\n",
    ];

    while stream.len() < 5 * 1024 * 1024 {
        for line in &sample_lines {
            stream.extend_from_slice(line.as_bytes());
        }
    }

    let total_bytes = stream.len();
    let start = Instant::now();

    // Feed the 5MB stream through VTE in 64KB chunks (standard PTY read buffer size)
    for chunk in stream.chunks(64 * 1024) {
        term.process_bytes(chunk);
    }

    let duration = start.elapsed();
    let mb = total_bytes as f64 / (1024.0 * 1024.0);
    let throughput_mb_s = mb / duration.as_secs_f64();

    println!(
        "\n=== PTY THROUGHPUT BENCHMARK ===\nProcessed: {:.2} MB in {:?} -> Throughput: {:.2} MB/s\n",
        mb, duration, throughput_mb_s
    );

    // Assert high throughput: must exceed at least 5.0 MB/s in unoptimized debug mode on shared CI runners (~100 MB/s in release)
    assert!(
        throughput_mb_s >= 5.0,
        "PTY throughput must exceed 5.0 MB/s in debug mode, achieved: {:.2} MB/s",
        throughput_mb_s
    );
}

#[test]
fn test_differential_cache_hit_latency() {
    let mut term = Terminal::new(120, 40, |_| {});
    term.process_bytes(
        b"OpenCode: \xd8\xaa\xd8\xb6\xd8\xa7\xd8\xb1\xd8\xa8 \xd8\xaa\xd8\xb1\xd8\xae\xd9\x8a\xd8\xb5 \xd9\x81\xd9\x8a Cargo.toml:7       Context: 44,865 tokens\r\n",
    );

    let (lines, cursor) = term.snapshot();
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let default_attrs = Attrs::new().family(Family::Monospace);
    let palette = Palette::default();
    let default_bg = Rgba::from_rgb8(21, 22, 30);

    // 1. Initial dirty row shaping
    let start_shaping = Instant::now();
    let cached = shape_row(
        &lines[0],
        0,
        &cursor,
        9.0,
        23.0,
        12.0,
        metrics,
        &default_attrs,
        &palette,
        &mut font_system,
        default_bg,
    );
    let shape_duration = start_shaping.elapsed();

    // 2. Cache hit validation: compute hash of unchanged row
    let start_hit = Instant::now();
    let hash = hash_cells(&lines[0].cells);
    let is_hit = hash == cached.hash;
    let hit_duration = start_hit.elapsed();

    println!(
        "\n=== CACHE LATENCY BENCHMARK ===\nInitial shaping: {:?}\nCache hit verification: {:?} (hit: {})\n",
        shape_duration, hit_duration, is_hit
    );

    assert!(is_hit, "Unchanged line must produce identical hash");
    assert!(
        hit_duration.as_micros() < 200,
        "Cache hit verification must complete in under 200 microseconds"
    );
}

#[test]
fn test_multi_column_bidi_shaping_latency() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let default_attrs = Attrs::new().family(Family::Monospace);
    let palette = Palette::default();
    let default_bg = Rgba::from_rgb8(21, 22, 30);

    let mut term = Terminal::new(140, 24, |_| {});
    // OpenCode dense split line
    let line_str = "Robustness: app.rs:349 فيه terminal.lock().unwrap() جوه render loop - لو poisoned هيكراش. و                    $0.00 spent\r\n";
    term.process_bytes(line_str.as_bytes());
    let (lines, cursor) = term.snapshot();

    let start = Instant::now();
    let cached = shape_row(
        &lines[0],
        0,
        &cursor,
        9.0,
        23.0,
        12.0,
        metrics,
        &default_attrs,
        &palette,
        &mut font_system,
        default_bg,
    );
    let duration = start.elapsed();

    println!(
        "\n=== MULTI-COLUMN BIDI SHAPING BENCHMARK ===\nDense row shaping: {:?} (segments: {})\n",
        duration,
        cached.segments.len()
    );

    // Multi-column row must separate into at least 2 distinct segments (chat and sidebar)
    assert!(
        cached.segments.len() >= 2,
        "Must isolate chat and sidebar into independent segments"
    );
    assert!(
        duration.as_millis() < 50,
        "Dense multi-column shaping must complete in under 50ms, took: {:?}",
        duration
    );
}
