use criterion::{Criterion, black_box, criterion_group, criterion_main};
use steer_grpc::client_api::{Message, MessageData, UserContent};
use steer_tui::tui::theme::Theme;
use steer_tui::tui::widgets::chat_widgets::{MessageWidget, RowWidget};
use steer_tui::tui::widgets::{ChatRenderable, ViewMode};

fn long_message(lines: usize) -> Message {
    let text = (0..lines)
        .map(|i| format!("Line {i} Lorem ipsum dolor sit amet, consectetur adipiscing elit."))
        .collect::<Vec<_>>()
        .join("\n");
    Message {
        data: MessageData::User {
            content: vec![UserContent::Text { text }],
        },
        id: "m".into(),
        parent_message_id: None,
        timestamp: 0,
    }
}

fn bench_rowwidget_cache_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("rowwidget");
    group.sample_size(20);

    group.bench_function("cache_hit", |b| {
        b.iter(|| {
            let theme = Theme::default();
            let msg = long_message(5000);
            let mut row = RowWidget::new(Box::new(MessageWidget::new(msg)));

            // Cold render
            let _ = row.lines(100, ViewMode::Compact, &theme);

            // Cache hits
            for _ in 0..50 {
                black_box(row.lines(100, ViewMode::Compact, &theme));
            }
        });
    });

    group.bench_function("resize_invalidate", |b| {
        b.iter(|| {
            let theme = Theme::default();
            let msg = long_message(5000);
            let mut row = RowWidget::new(Box::new(MessageWidget::new(msg)));

            let _ = row.lines(100, ViewMode::Compact, &theme);
            for w in [120u16, 140, 160, 180, 200] {
                black_box(row.lines(w, ViewMode::Compact, &theme));
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_rowwidget_cache_hit);
criterion_main!(benches);
