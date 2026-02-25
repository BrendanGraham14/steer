use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use ratatui::{Terminal, backend::TestBackend};
use steer_grpc::client_api::{AssistantContent, Message, MessageData, UserContent};
use steer_tui::tui::ChatViewport;
use steer_tui::tui::model::ChatItem;
use steer_tui::tui::state::ChatStore;
use steer_tui::tui::theme::Theme;
use steer_tui::tui::widgets::chat_widgets::{MessageWidget, RowWidget};
use steer_tui::tui::widgets::{ChatRenderable, ViewMode};

const VIEWPORT_WIDTH: u16 = 120;
const VIEWPORT_HEIGHT: u16 = 30;
const SCROLL_STEPS: usize = 200;
const SCROLL_DELTA: usize = 3;

const FLAT_CHAT_SIZES: [usize; 3] = [1_000, 5_000, 20_000];
const RICH_CHAT_SIZES: [usize; 2] = [20, 80];
const VARIABLE_CHAT_SIZES: [usize; 2] = [1_000, 5_000];
const RICH_MARKDOWN_BLOCKS: usize = 40;

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

fn build_chat_items_with<F>(count: usize, mut body_for_index: F) -> (Vec<ChatItem>, ChatStore)
where
    F: FnMut(usize) -> String,
{
    let mut store = ChatStore::default();
    let mut parent: Option<String> = None;

    for i in 0..count {
        let id = format!("msg-{i}");
        let text = body_for_index(i);
        let data = if i % 2 == 0 {
            MessageData::User {
                content: vec![UserContent::Text { text }],
            }
        } else {
            MessageData::Assistant {
                content: vec![AssistantContent::Text { text }],
            }
        };

        let msg = Message {
            data,
            id: id.clone(),
            parent_message_id: parent.clone(),
            timestamp: i as u64,
        };
        store.add_message(msg);
        parent = Some(id);
    }

    let items = store
        .as_vec()
        .into_iter()
        .cloned()
        .collect::<Vec<ChatItem>>();

    (items, store)
}

fn build_flat_chat_items(count: usize) -> (Vec<ChatItem>, ChatStore) {
    build_chat_items_with(count, |i| {
        if i % 2 == 0 {
            format!("User message {i}: lorem ipsum dolor sit amet, consectetur adipiscing elit.")
        } else {
            format!(
                "Assistant message {i}: sed do eiusmod tempor incididunt ut labore et dolore magna aliqua."
            )
        }
    })
}

fn rich_markdown_body(message_idx: usize, blocks: usize) -> String {
    use std::fmt::Write as _;

    let mut body = String::new();
    for block in 0..blocks {
        let _ = writeln!(body, "## Section {message_idx}.{block}");
        let _ = writeln!(body);
        let _ = writeln!(
            body,
            "> Constraint {block}: keep output deterministic and idempotent."
        );
        let _ = writeln!(body);
        let _ = writeln!(body, "- [ ] investigate parser branch {}", block % 7);
        let _ = writeln!(body, "- [x] cache rendered chunk {}", block % 5);
        let _ = writeln!(
            body,
            "- note: `fn render_markdown_chunk(input: &str) -> usize`"
        );
        let _ = writeln!(body);
        let _ = writeln!(body, "```rust");
        let _ = writeln!(body, "fn chunk_{block}(value: usize) -> usize {{");
        let _ = writeln!(body, "    value.saturating_mul(2).saturating_add({block})");
        let _ = writeln!(body, "}}");
        let _ = writeln!(body, "```");
        let _ = writeln!(body);
        let _ = writeln!(body, "| metric | value |");
        let _ = writeln!(body, "| --- | --- |");
        let _ = writeln!(body, "| rows | {} |", block + 1);
        let _ = writeln!(body, "| hash | `{message_idx:04x}-{block:04x}` |");
        let _ = writeln!(body);
        let _ = writeln!(
            body,
            "Paragraph {message_idx}.{block} lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua."
        );
        let _ = writeln!(body);
    }
    body
}

fn build_rich_markdown_chat_items(
    count: usize,
    blocks_per_message: usize,
) -> (Vec<ChatItem>, ChatStore) {
    build_chat_items_with(count, |i| rich_markdown_body(i, blocks_per_message))
}

fn variable_length_body(message_idx: usize) -> String {
    match message_idx % 10 {
        0 => rich_markdown_body(message_idx, 24),
        1 | 2 => (0..48)
            .map(|line| {
                format!(
                    "Long plain line {line} for message {message_idx}: suspendisse potenti integer nec odio praesent libero sed cursus ante dapibus diam."
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        3 | 4 | 5 => (0..8)
            .map(|line| format!("Medium line {line} for message {message_idx}"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => format!("Short message {message_idx}: ok."),
    }
}

fn build_variable_length_chat_items(count: usize) -> (Vec<ChatItem>, ChatStore) {
    build_chat_items_with(count, variable_length_body)
}

fn bench_chat_viewport_rebuild_flat(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_rebuild_flat");
    group.sample_size(10);

    let theme = Theme::default();

    for size in FLAT_CHAT_SIZES {
        let (items, store) = build_flat_chat_items(size);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new("rebuild", format!("{size}_messages")),
            |b| {
                b.iter_batched(
                    ChatViewport::new,
                    |mut viewport| {
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );
                        black_box(viewport.state().total_content_height);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_chat_viewport_steady_scroll_flat(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_steady_scroll_flat");
    group.sample_size(10);

    let theme = Theme::default();

    for size in FLAT_CHAT_SIZES {
        let (items, store) = build_flat_chat_items(size);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new("render", format!("{size}_messages")),
            |b| {
                b.iter_batched(
                    || {
                        let mut viewport = ChatViewport::new();
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );

                        let mut terminal =
                            Terminal::new(TestBackend::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT))
                                .expect("create benchmark terminal");
                        terminal
                            .draw(|f| {
                                let area = f.area();
                                viewport.render(f, area, 0, None, &theme);
                            })
                            .expect("prime viewport frame");

                        let max_offset = viewport
                            .state()
                            .total_content_height
                            .saturating_sub(VIEWPORT_HEIGHT as usize);

                        (viewport, terminal, max_offset)
                    },
                    |(mut viewport, mut terminal, max_offset)| {
                        let offset_span = max_offset.saturating_add(1).max(1);
                        for step in 0..SCROLL_STEPS {
                            let offset = (step * SCROLL_DELTA) % offset_span;
                            viewport.state_mut().offset = black_box(offset);
                            terminal
                                .draw(|f| {
                                    let area = f.area();
                                    viewport.render(f, area, 0, None, &theme);
                                })
                                .expect("draw benchmark frame");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_chat_viewport_rebuild_rich_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_rebuild_rich_markdown");
    group.sample_size(10);

    let theme = Theme::default();

    for size in RICH_CHAT_SIZES {
        let (items, store) = build_rich_markdown_chat_items(size, RICH_MARKDOWN_BLOCKS);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new(
                "rebuild",
                format!("{size}_messages_{}_blocks", RICH_MARKDOWN_BLOCKS),
            ),
            |b| {
                b.iter_batched(
                    ChatViewport::new,
                    |mut viewport| {
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );
                        black_box(viewport.state().total_content_height);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_chat_viewport_steady_scroll_rich_markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_steady_scroll_rich_markdown");
    group.sample_size(10);

    let theme = Theme::default();

    for size in RICH_CHAT_SIZES {
        let (items, store) = build_rich_markdown_chat_items(size, RICH_MARKDOWN_BLOCKS);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new(
                "render",
                format!("{size}_messages_{}_blocks", RICH_MARKDOWN_BLOCKS),
            ),
            |b| {
                b.iter_batched(
                    || {
                        let mut viewport = ChatViewport::new();
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );

                        let mut terminal =
                            Terminal::new(TestBackend::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT))
                                .expect("create benchmark terminal");
                        terminal
                            .draw(|f| {
                                let area = f.area();
                                viewport.render(f, area, 0, None, &theme);
                            })
                            .expect("prime viewport frame");

                        let max_offset = viewport
                            .state()
                            .total_content_height
                            .saturating_sub(VIEWPORT_HEIGHT as usize);

                        (viewport, terminal, max_offset)
                    },
                    |(mut viewport, mut terminal, max_offset)| {
                        let offset_span = max_offset.saturating_add(1).max(1);
                        for step in 0..SCROLL_STEPS {
                            let offset = (step * SCROLL_DELTA) % offset_span;
                            viewport.state_mut().offset = black_box(offset);
                            terminal
                                .draw(|f| {
                                    let area = f.area();
                                    viewport.render(f, area, 0, None, &theme);
                                })
                                .expect("draw benchmark frame");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_chat_viewport_rebuild_variable_length(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_rebuild_variable_length");
    group.sample_size(10);

    let theme = Theme::default();

    for size in VARIABLE_CHAT_SIZES {
        let (items, store) = build_variable_length_chat_items(size);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new("rebuild", format!("{size}_messages_mixed")),
            |b| {
                b.iter_batched(
                    ChatViewport::new,
                    |mut viewport| {
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );
                        black_box(viewport.state().total_content_height);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_chat_viewport_steady_scroll_variable_length(c: &mut Criterion) {
    let mut group = c.benchmark_group("chat_viewport_steady_scroll_variable_length");
    group.sample_size(10);

    let theme = Theme::default();

    for size in VARIABLE_CHAT_SIZES {
        let (items, store) = build_variable_length_chat_items(size);
        let refs = items.iter().collect::<Vec<_>>();

        group.bench_function(
            BenchmarkId::new("render", format!("{size}_messages_mixed")),
            |b| {
                b.iter_batched(
                    || {
                        let mut viewport = ChatViewport::new();
                        viewport.rebuild(
                            &refs,
                            VIEWPORT_WIDTH,
                            ViewMode::Compact,
                            &theme,
                            &store,
                            None,
                        );

                        let mut terminal =
                            Terminal::new(TestBackend::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT))
                                .expect("create benchmark terminal");
                        terminal
                            .draw(|f| {
                                let area = f.area();
                                viewport.render(f, area, 0, None, &theme);
                            })
                            .expect("prime viewport frame");

                        let max_offset = viewport
                            .state()
                            .total_content_height
                            .saturating_sub(VIEWPORT_HEIGHT as usize);

                        (viewport, terminal, max_offset)
                    },
                    |(mut viewport, mut terminal, max_offset)| {
                        let offset_span = max_offset.saturating_add(1).max(1);
                        for step in 0..SCROLL_STEPS {
                            let offset = (step * SCROLL_DELTA) % offset_span;
                            viewport.state_mut().offset = black_box(offset);
                            terminal
                                .draw(|f| {
                                    let area = f.area();
                                    viewport.render(f, area, 0, None, &theme);
                                })
                                .expect("draw benchmark frame");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
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

criterion_group!(
    benches,
    bench_rowwidget_cache_hit,
    bench_chat_viewport_rebuild_flat,
    bench_chat_viewport_steady_scroll_flat,
    bench_chat_viewport_rebuild_rich_markdown,
    bench_chat_viewport_steady_scroll_rich_markdown,
    bench_chat_viewport_rebuild_variable_length,
    bench_chat_viewport_steady_scroll_variable_length
);
criterion_main!(benches);
