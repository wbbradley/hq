//! Explicit pure-model invalidation performance regression budget.

#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use hq_tui::{
    UiConversationAuthor, UiConversationEntry, UiConversationEntryPresentation, UiConversationPage,
    UiEffect, UiEvent, UiHumanState, UiMaterializedConversationView, UiMessageState, UiModel,
    UiRenderCache, UiRow, UiRowKind, UiRowState, UiSize, UiSnapshot, UiTheme, render_with_cache,
    update,
};
use ratatui::{Terminal, backend::TestBackend};

const REPRESENTATIVE_ROW_COUNT: usize = 10_000;
const MAXIMUM_TUI_CONVERSATION_PAGE: usize = 100;
const MAXIMUM_MESSAGE_BYTES: usize = 16_384;

fn budget() -> Duration {
    let milliseconds = std::env::var("HQ_QUALIFICATION_INVALIDATION_REDRAW_MAX_MILLISECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(100);
    Duration::from_millis(milliseconds)
}

fn markdown_redraw_budget() -> Duration {
    let milliseconds =
        std::env::var("HQ_QUALIFICATION_MAXIMUM_MARKDOWN_PAGE_REDRAW_MAX_MILLISECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(100);
    Duration::from_millis(milliseconds)
}

#[test]
fn invalidation_of_a_large_ready_model_requests_redraw_within_the_declared_budget() {
    let started = update(
        UiModel::new(UiSize {
            width: 120,
            height: 40,
        }),
        UiEvent::Started,
    )
    .expect("model starts");
    let snapshot_id = started
        .effects
        .iter()
        .find_map(|effect| match effect {
            UiEffect::LoadSnapshot { id, .. } => Some(*id),
            _ => None,
        })
        .expect("startup requests a snapshot");
    let snapshot = UiSnapshot {
        revision: 1,
        human_state: UiHumanState::Ready,
        inbox_rows: (0..REPRESENTATIVE_ROW_COUNT)
            .map(|index| UiRow {
                id: format!("conversation-{index}"),
                title: format!("Conversation {index}"),
                detail: "ready".to_owned(),
                state: UiRowState::Open,
                kind: UiRowKind::Conversation,
                conversation_target: None,
            })
            .collect(),
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    };
    let ready = update(
        started.model,
        UiEvent::SnapshotLoaded {
            effect_id: snapshot_id,
            snapshot,
        },
    )
    .expect("large snapshot loads");

    let measured_at = Instant::now();
    let invalidated =
        update(ready.model, UiEvent::Invalidated { revision: 2 }).expect("invalidation applies");
    let elapsed = measured_at.elapsed();

    assert!(
        invalidated
            .effects
            .iter()
            .any(|effect| matches!(effect, UiEffect::LoadSnapshot { .. }))
    );
    assert!(invalidated.effects.contains(&UiEffect::RequestRedraw));
    let maximum = budget();
    assert!(
        elapsed <= maximum,
        "invalidation-to-redraw took {elapsed:?}, exceeding {maximum:?}"
    );
}

#[test]
fn maximum_markdown_page_renders_within_the_declared_redraw_budget() {
    let size = UiSize {
        width: 120,
        height: 40,
    };
    let body = representative_markdown_message();
    assert_eq!(body.len(), MAXIMUM_MESSAGE_BYTES);
    let model = maximum_markdown_conversation(size, &body);

    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height))
        .expect("test terminal initializes");
    let theme = UiTheme::terminal();
    let mut cache = UiRenderCache::new();
    terminal
        .draw(|frame| {
            let _ = render_with_cache(frame, &model, &theme, &mut cache);
        })
        .expect("initial maximum Markdown page renders");
    let initial = terminal.backend().buffer().clone();
    let measured_at = Instant::now();
    terminal
        .draw(|frame| {
            let _ = render_with_cache(frame, &model, &theme, &mut cache);
        })
        .expect("maximum Markdown page renders");
    let elapsed = measured_at.elapsed();
    assert_eq!(terminal.backend().buffer(), &initial);

    let rendered =
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .fold(String::new(), |mut text, cell| {
                text.push_str(cell.symbol());
                text
            });
    assert!(rendered.contains("Qualification heading"));
    let maximum = markdown_redraw_budget();
    assert!(
        elapsed <= maximum,
        "maximum 100-entry Markdown page redraw took {elapsed:?}, exceeding {maximum:?}"
    );
}

fn maximum_markdown_conversation(size: UiSize, body: &str) -> UiModel {
    let started = update(UiModel::new(size), UiEvent::Started).expect("model starts");
    let snapshot = UiSnapshot {
        revision: 1,
        human_state: UiHumanState::Ready,
        inbox_rows: vec![UiRow {
            id: "markdown-conversation".to_owned(),
            title: "Markdown qualification".to_owned(),
            detail: "maximum page".to_owned(),
            state: UiRowState::Open,
            kind: UiRowKind::Conversation,
            conversation_target: None,
        }],
        sent_rows: Vec::new(),
        archived_rows: Vec::new(),
        agent_rows: Vec::new(),
        project_rows: Vec::new(),
        direct_targets: Vec::new(),
        providers: Vec::new(),
        agents: Vec::new(),
        projects: Vec::new(),
    };

    let entries = (0..MAXIMUM_TUI_CONVERSATION_PAGE)
        .map(|index| UiConversationEntry {
            id: format!("markdown-message-{index:03}"),
            presentation: UiConversationEntryPresentation::Message {
                author: UiConversationAuthor::Participant("Alice".to_owned()),
                body: body.to_owned(),
            },
            message_state: Some(UiMessageState::Open),
            delivery: None,
            message_target: None,
            technical: Vec::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), MAXIMUM_TUI_CONVERSATION_PAGE);
    update(
        started.model,
        UiEvent::MaterializedViewObserved {
            view: UiMaterializedConversationView {
                snapshot,
                conversation: Some(UiConversationPage {
                    row_id: "markdown-conversation".to_owned(),
                    title: "Markdown qualification".to_owned(),
                    context: None,
                    entries,
                    next_cursor: None,
                }),
            },
        },
    )
    .expect("maximum conversation page loads")
    .model
}

fn representative_markdown_message() -> String {
    const SAMPLE: &str = concat!(
        "# Qualification heading\n\n",
        "A **strong** paragraph with *emphasis*, ~~retired text~~, `inline code`, and ",
        "[visible documentation](https://example.test/docs).  \n",
        "The next line contains Unicode: cafe\u{301}, 界, and 👩‍💻.\n\n",
        "> A quoted explanation that wraps across terminal rows.\n\n",
        "1. ordered work\n   - nested work with continuation text\n- [x] completed task\n\n",
        "```rust\nfn qualified() -> bool { true }\n```\n\n",
        "| Item | Result | Notes |\n| --- | --- | --- |\n| renderer | ready | bounded table content |\n\n",
        "![diagram](file:///tmp/qualification.png)\n\n",
    );
    let mut message = String::with_capacity(MAXIMUM_MESSAGE_BYTES);
    while message.len().saturating_add(SAMPLE.len()) <= MAXIMUM_MESSAGE_BYTES {
        message.push_str(SAMPLE);
    }
    message.extend(std::iter::repeat_n(
        'x',
        MAXIMUM_MESSAGE_BYTES - message.len(),
    ));
    message
}
